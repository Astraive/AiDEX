//! `aidex events tail --follow`: stream, reconnect with backoff, resume exactly.
//!
//! The loop owns the acknowledged cursor: durable events advance it only after
//! their lines were written and flushed; ordered watermarks advance it across
//! filtered-out durable events without printing. Every reconnect resumes strictly
//! after it, and an acknowledged durable event is never printed again. Transient
//! failures reconnect with exponential backoff; all other errors end the command.

use super::event_error;
use crate::{
    commands::daemon::connect_root,
    exit::{CliError, Exit},
};
use aidex_events::{Cursor, EventEnvelope, EventFilter, SubscriptionItem};
use aidexd::{
    ClientError,
    methods::events::{
        ERROR_NOTIFICATION, EVENT_NOTIFICATION, SUBSCRIBE, StreamError, StreamWatermark,
        WATERMARK_NOTIFICATION, decode_event,
    },
};
use serde_json::{Value, json};
use std::{io, path::PathBuf, thread, time::Duration};

const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(500);

/// Why one stream stopped.
pub(crate) enum StreamEnd {
    /// Reconnect after a backoff.
    Transient(String),
    /// Stop following with this error.
    Fatal(CliError),
}

/// A connection-scoped event stream (the daemon, or a fake in tests).
pub(crate) trait EventSource {
    /// Stream ordered events and watermarks strictly after `after` into `deliver`
    /// until the stream ends (`Ok`) or fails. Delivery errors are fatal.
    fn stream(
        &mut self,
        after: Cursor,
        filter: &EventFilter,
        deliver: &mut dyn FnMut(&SubscriptionItem) -> Result<(), CliError>,
    ) -> Result<(), StreamEnd>;
}

pub(crate) trait Clock {
    fn sleep(&mut self, duration: Duration);
}

pub(crate) struct RealClock;

impl Clock for RealClock {
    fn sleep(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// Follow forever; returns only on a fatal error (including a closed stdout).
pub(crate) fn follow(
    source: &mut dyn EventSource,
    after: Cursor,
    filter: &EventFilter,
    emit: &mut dyn FnMut(&EventEnvelope) -> io::Result<()>,
    clock: &mut dyn Clock,
) -> Result<Exit, CliError> {
    let mut acknowledged = after;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let mut progressed = false;
        let requested = acknowledged;
        let outcome = source.stream(requested, filter, &mut |item| {
            match item.event.as_ref() {
                Some(event) => {
                    if event.is_durable() && event.cursor <= acknowledged {
                        return Ok(());
                    }
                    emit(event).map_err(|error| {
                        CliError::new(Exit::Failure, format!("cannot write events: {error}"))
                    })?;
                    if event.is_durable() {
                        acknowledged = event.cursor;
                    }
                    progressed = true;
                }
                None => {
                    if item.cursor > acknowledged {
                        acknowledged = item.cursor;
                        progressed = true;
                    }
                }
            }
            Ok(())
        });
        let reason = match outcome {
            Ok(()) => "the daemon ended the stream".to_owned(),
            Err(StreamEnd::Transient(message)) => message,
            Err(StreamEnd::Fatal(error)) => return Err(error),
        };
        if progressed {
            backoff = INITIAL_BACKOFF;
        }
        eprintln!(
            "aidex events: {reason}; reconnecting after cursor {acknowledged} in {:.2} s",
            backoff.as_secs_f64()
        );
        clock.sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// `events.subscribe` on the drive's daemon, one connection per stream.
pub(crate) struct DaemonSource {
    root: PathBuf,
}

impl DaemonSource {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

fn classify(error: CliError) -> StreamEnd {
    if error.exit == Exit::DaemonUnavailable {
        StreamEnd::Transient(error.message)
    } else {
        StreamEnd::Fatal(error)
    }
}

fn classify_client(error: ClientError) -> StreamEnd {
    classify(event_error(error))
}

fn stream_error(params: Value) -> CliError {
    match serde_json::from_value::<StreamError>(params) {
        Ok(error) if error.reason == "cursor_expired" => {
            let cli = CliError::new(Exit::NotFound, error.message);
            match error.oldest_available {
                Some(oldest) => cli.with_hint(format!(
                    "rerun with `--after {}` to continue from the oldest retained event",
                    oldest.get().saturating_sub(1)
                )),
                None => cli,
            }
        }
        Ok(error) => CliError::new(Exit::Failure, error.message),
        Err(error) => CliError::new(Exit::Failure, format!("event stream failed: {error}")),
    }
}

impl EventSource for DaemonSource {
    fn stream(
        &mut self,
        after: Cursor,
        filter: &EventFilter,
        deliver: &mut dyn FnMut(&SubscriptionItem) -> Result<(), CliError>,
    ) -> Result<(), StreamEnd> {
        let mut client = connect_root(&self.root).map_err(classify)?;
        let mut subscription = client
            .subscribe(SUBSCRIBE, json!({ "after": after, "filter": filter }))
            .map_err(classify_client)?;
        loop {
            match subscription.next_timeout(POLL) {
                Ok(Some(notification)) if notification.method == EVENT_NOTIFICATION => {
                    let event = decode_event(notification.params).map_err(|error| {
                        StreamEnd::Fatal(CliError::new(
                            Exit::Failure,
                            format!("daemon sent an invalid event: {error}"),
                        ))
                    })?;
                    deliver(&SubscriptionItem::event(event)).map_err(StreamEnd::Fatal)?;
                }
                Ok(Some(notification)) if notification.method == WATERMARK_NOTIFICATION => {
                    let watermark: StreamWatermark = serde_json::from_value(notification.params)
                        .map_err(|error| {
                            StreamEnd::Fatal(CliError::new(
                                Exit::Failure,
                                format!("daemon sent an invalid event watermark: {error}"),
                            ))
                        })?;
                    deliver(&SubscriptionItem::watermark(watermark.cursor))
                        .map_err(StreamEnd::Fatal)?;
                }
                Ok(Some(notification)) if notification.method == ERROR_NOTIFICATION => {
                    return Err(StreamEnd::Fatal(stream_error(notification.params)));
                }
                Ok(Some(_)) => {}
                Ok(None) if subscription.is_ended() => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(classify_client(error)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    fn envelope(cursor: u64, durable: bool) -> EventEnvelope {
        let event = if durable {
            json!({"type": "task.started", "data": {"task_id": format!("t{cursor}")}})
        } else {
            json!({"type": "agent.delta", "data": {"text": "partial"}})
        };
        serde_json::from_value(json!({
            "version": 2,
            "cursor": cursor,
            "scope": {"workspace_id": "ws"},
            "occurred_at": "2026-10-10T00:00:00.000Z",
            "event": event,
        }))
        .unwrap()
    }

    enum End {
        Transient,
        Ended,
        Fatal,
    }

    /// Serves a scripted log; each stream re-sends the event at `after` (a duplicate)
    /// and then up to `count` newer events before ending as scripted.
    struct FakeSource {
        log: Vec<EventEnvelope>,
        sessions: VecDeque<(usize, End)>,
        requests: Vec<u64>,
    }

    impl EventSource for FakeSource {
        fn stream(
            &mut self,
            after: Cursor,
            _filter: &EventFilter,
            deliver: &mut dyn FnMut(&SubscriptionItem) -> Result<(), CliError>,
        ) -> Result<(), StreamEnd> {
            self.requests.push(after.get());
            let (count, end) = self.sessions.pop_front().expect("scripted session");
            let duplicate = self
                .log
                .iter()
                .filter(|event| event.cursor == after)
                .take(1);
            let newer = self
                .log
                .iter()
                .filter(|event| {
                    event.cursor > after || (!event.is_durable() && event.cursor == after)
                })
                .take(count);
            for event in duplicate.chain(newer) {
                deliver(&SubscriptionItem::event(event.clone())).map_err(StreamEnd::Fatal)?;
            }
            match end {
                End::Transient => Err(StreamEnd::Transient("daemon closed the connection".into())),
                End::Ended => Ok(()),
                End::Fatal => Err(StreamEnd::Fatal(CliError::new(
                    Exit::Cancelled,
                    "script done",
                ))),
            }
        }
    }

    #[derive(Default)]
    struct FakeClock {
        sleeps: Vec<Duration>,
    }

    impl Clock for FakeClock {
        fn sleep(&mut self, duration: Duration) {
            self.sleeps.push(duration);
        }
    }

    fn durable_log(count: u64) -> Vec<EventEnvelope> {
        (1..=count).map(|cursor| envelope(cursor, true)).collect()
    }

    #[test]
    fn reconnects_resume_after_the_last_printed_cursor_without_duplicates() {
        let mut source = FakeSource {
            log: durable_log(10),
            sessions: VecDeque::from([
                (3, End::Transient),
                (0, End::Transient),
                (4, End::Ended),
                (3, End::Fatal),
            ]),
            requests: Vec::new(),
        };
        let mut printed = Vec::new();
        let mut clock = FakeClock::default();
        let error = follow(
            &mut source,
            Cursor::ZERO,
            &EventFilter::all(),
            &mut |event| {
                printed.push(event.cursor.get());
                Ok(())
            },
            &mut clock,
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Cancelled);
        assert_eq!(printed, (1..=10).collect::<Vec<_>>());
        assert_eq!(source.requests, [0, 3, 3, 7]);
        // Backoff doubles while nothing arrives and resets after progress.
        assert_eq!(
            clock.sleeps,
            [
                Duration::from_millis(250),
                Duration::from_millis(500),
                Duration::from_millis(250)
            ]
        );
    }

    #[test]
    fn backoff_is_capped_while_the_daemon_stays_down() {
        let mut sessions: VecDeque<_> = (0..8).map(|_| (0, End::Transient)).collect();
        sessions.push_back((0, End::Fatal));
        let mut source = FakeSource {
            log: Vec::new(),
            sessions,
            requests: Vec::new(),
        };
        let mut clock = FakeClock::default();
        follow(
            &mut source,
            Cursor::new(5),
            &EventFilter::all(),
            &mut |_| Ok(()),
            &mut clock,
        )
        .unwrap_err();
        assert!(source.requests.iter().all(|after| *after == 5));
        assert_eq!(clock.sleeps.last(), Some(&MAX_BACKOFF));
        assert!(clock.sleeps.iter().all(|sleep| *sleep <= MAX_BACKOFF));
    }

    #[test]
    fn ephemeral_events_print_but_never_move_the_cursor() {
        let mut log = durable_log(2);
        log.insert(1, envelope(1, false));
        let mut source = FakeSource {
            log,
            sessions: VecDeque::from([(3, End::Transient), (5, End::Fatal)]),
            requests: Vec::new(),
        };
        let mut printed = Vec::new();
        follow(
            &mut source,
            Cursor::ZERO,
            &EventFilter::all(),
            &mut |event| {
                printed.push((event.cursor.get(), event.is_durable()));
                Ok(())
            },
            &mut FakeClock::default(),
        )
        .unwrap_err();
        assert_eq!(printed[..3], [(1, true), (1, false), (2, true)]);
        assert_eq!(source.requests, [0, 2]);
    }

    #[test]
    fn a_failed_write_stops_without_acknowledging_the_event() {
        let mut source = FakeSource {
            log: durable_log(3),
            sessions: VecDeque::from([(3, End::Ended)]),
            requests: Vec::new(),
        };
        let mut written = 0;
        let error = follow(
            &mut source,
            Cursor::ZERO,
            &EventFilter::all(),
            &mut |_| {
                written += 1;
                if written == 2 {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
                } else {
                    Ok(())
                }
            },
            &mut FakeClock::default(),
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Failure);
        assert!(
            error.message.contains("cannot write events"),
            "{}",
            error.message
        );
    }

    struct ScriptedSource {
        sessions: VecDeque<(Vec<SubscriptionItem>, End)>,
        requests: Vec<u64>,
        delivered: usize,
    }

    impl EventSource for ScriptedSource {
        fn stream(
            &mut self,
            after: Cursor,
            _filter: &EventFilter,
            deliver: &mut dyn FnMut(&SubscriptionItem) -> Result<(), CliError>,
        ) -> Result<(), StreamEnd> {
            self.requests.push(after.get());
            let (items, end) = self.sessions.pop_front().expect("scripted session");
            for item in items {
                deliver(&item).map_err(StreamEnd::Fatal)?;
                self.delivered += 1;
            }
            match end {
                End::Transient => Err(StreamEnd::Transient("daemon restarted".into())),
                End::Ended => Ok(()),
                End::Fatal => Err(StreamEnd::Fatal(CliError::new(
                    Exit::Cancelled,
                    "script done",
                ))),
            }
        }
    }

    #[test]
    fn ephemeral_cursors_and_stale_watermarks_never_move_the_resume_cursor() {
        let mut source = ScriptedSource {
            sessions: VecDeque::from([
                (
                    vec![
                        SubscriptionItem::watermark(Cursor::new(5)),
                        SubscriptionItem::event(envelope(99, false)),
                    ],
                    End::Transient,
                ),
                (
                    vec![SubscriptionItem::watermark(Cursor::new(4))],
                    End::Transient,
                ),
                (Vec::new(), End::Fatal),
            ]),
            requests: Vec::new(),
            delivered: 0,
        };
        let mut printed = Vec::new();
        let mut clock = FakeClock::default();
        let error = follow(
            &mut source,
            Cursor::new(5),
            &EventFilter::all(),
            &mut |event| {
                printed.push(event.cursor.get());
                Ok(())
            },
            &mut clock,
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Cancelled);
        assert_eq!(source.requests, [5, 5, 5]);
        assert_eq!(printed, [99]);
        assert_eq!(clock.sleeps, [INITIAL_BACKOFF, INITIAL_BACKOFF * 2]);
    }

    #[test]
    fn watermarks_resume_without_printing_or_regressing_and_keep_ephemeral_events() {
        let mut source = ScriptedSource {
            sessions: VecDeque::from([
                (
                    vec![
                        SubscriptionItem::event(envelope(1, true)),
                        SubscriptionItem::watermark(Cursor::new(4)),
                        SubscriptionItem::watermark(Cursor::new(3)),
                        SubscriptionItem::event(envelope(4, false)),
                    ],
                    End::Transient,
                ),
                (
                    vec![
                        SubscriptionItem::event(envelope(5, true)),
                        SubscriptionItem::watermark(Cursor::new(8)),
                    ],
                    End::Fatal,
                ),
            ]),
            requests: Vec::new(),
            delivered: 0,
        };
        let mut printed = Vec::new();
        let mut clock = FakeClock::default();
        let error = follow(
            &mut source,
            Cursor::ZERO,
            &EventFilter::all()
                .with_kind("task")
                .with_kind("agent.delta"),
            &mut |event| {
                printed.push((event.cursor.get(), event.is_durable()));
                Ok(())
            },
            &mut clock,
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Cancelled);
        assert_eq!(printed, [(1, true), (4, false), (5, true)]);
        assert_eq!(source.requests, [0, 4]);
        assert_eq!(clock.sleeps, [INITIAL_BACKOFF]);
    }

    #[test]
    fn a_failed_matching_write_prevents_acknowledging_a_later_watermark() {
        let mut source = ScriptedSource {
            sessions: VecDeque::from([(
                vec![
                    SubscriptionItem::event(envelope(1, true)),
                    SubscriptionItem::watermark(Cursor::new(8)),
                ],
                End::Transient,
            )]),
            requests: Vec::new(),
            delivered: 0,
        };
        let mut clock = FakeClock::default();
        let error = follow(
            &mut source,
            Cursor::ZERO,
            &EventFilter::all(),
            &mut |_| Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")),
            &mut clock,
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Failure);
        assert_eq!(source.delivered, 0);
        assert_eq!(source.requests, [0]);
        assert!(clock.sleeps.is_empty());
    }

    #[test]
    fn filtered_follow_reconnects_after_consumed_retained_progress() {
        struct LogSource {
            log: aidex_events::EventLog,
            requests: Vec<u64>,
        }
        impl EventSource for LogSource {
            fn stream(
                &mut self,
                after: Cursor,
                filter: &EventFilter,
                deliver: &mut dyn FnMut(&SubscriptionItem) -> Result<(), CliError>,
            ) -> Result<(), StreamEnd> {
                self.requests.push(after.get());
                let mut subscription =
                    self.log.subscribe(filter.clone(), after).map_err(|error| {
                        StreamEnd::Fatal(CliError::new(Exit::NotFound, error.to_string()))
                    })?;
                let range = if self.requests.len() == 1 {
                    1..=10
                } else {
                    11..=11
                };
                for cursor in range {
                    let event = envelope(cursor, true);
                    let scope = if cursor == 1 || cursor == 11 {
                        event.scope
                    } else {
                        aidex_events::EventScope::workspace(
                            aidex_core::EntityId::new("other").unwrap(),
                        )
                    };
                    self.log.append(scope, event.event).unwrap();
                    let item = subscription
                        .try_next()
                        .unwrap()
                        .expect("event or watermark");
                    deliver(&item).map_err(StreamEnd::Fatal)?;
                }
                assert_eq!(subscription.try_next().unwrap(), None);
                if self.requests.len() == 1 {
                    Err(StreamEnd::Transient("daemon restarted".into()))
                } else {
                    Err(StreamEnd::Fatal(CliError::new(
                        Exit::Cancelled,
                        "script done",
                    )))
                }
            }
        }
        let root = tempfile::tempdir().unwrap();
        let mut source = LogSource {
            log: aidex_events::EventLog::open(
                root.path().join("events.sqlite3"),
                aidex_events::RetentionPolicy::max_events(2),
            )
            .unwrap(),
            requests: Vec::new(),
        };
        let filter = EventFilter::for_workspace(aidex_core::EntityId::new("ws").unwrap());
        let mut printed = Vec::new();
        let error = follow(
            &mut source,
            Cursor::ZERO,
            &filter,
            &mut |event| {
                printed.push(event.cursor.get());
                Ok(())
            },
            &mut FakeClock::default(),
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::Cancelled);
        assert_eq!(source.requests, [0, 10]);
        assert_eq!(printed, [1, 11]);
        assert!(matches!(
            source.log.subscribe(filter, Cursor::new(1)),
            Err(aidex_events::EventError::CursorExpired { .. })
        ));
    }

    #[test]
    fn stream_errors_map_to_exit_codes_with_hints() {
        let expired = stream_error(json!({
            "reason": "cursor_expired",
            "message": "event cursor 0 expired",
            "oldest_available": 7
        }));
        assert_eq!(expired.exit, Exit::NotFound);
        assert!(expired.hint.unwrap().contains("--after 6"));
        assert_eq!(
            stream_error(json!({"reason": "internal", "message": "boom"})).exit,
            Exit::Failure
        );
        assert!(matches!(
            classify(CliError::new(Exit::DaemonUnavailable, "down")),
            StreamEnd::Transient(_)
        ));
        assert!(matches!(
            classify(CliError::new(Exit::Incompatible, "v1")),
            StreamEnd::Fatal(_)
        ));
    }
}
