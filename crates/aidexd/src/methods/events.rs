//! `events.*` methods over the drive's durable `aidex-events` log.
//!
//! - `events.replay {after?, filter?, limit?}` → one [`ReplayPage`] strictly after `after`.
//! - `events.latest {}` → `{latest_cursor, oldest_cursor}`.
//! - `events.subscribe {after?, filter?}` (stream) → `events.event` notifications carrying
//!   [`EventEnvelope`]s and `events.watermark` notifications carrying [`StreamWatermark`],
//!   in order: persisted history first, then live events. A watermark consumes
//!   filtered-out durable events only after preceding matching events. If the stream
//!   cannot continue without a gap it sends one `events.error` and ends.
//!   A slow client is disconnected (`slow_consumer`) and resubscribes after its
//!   last processed durable event or watermark.

use crate::{
    errors::RpcError,
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
    stream::NotificationSink,
};
use aidex_events::{Cursor, EventError, EventFilter, MAX_PAGE_SIZE, Subscription};
/// Result of `events.replay`; `events.event` notifications carry an [`EventEnvelope`].
pub use aidex_events::{EventEnvelope, ReplayPage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, thread, time::Duration};

pub const REPLAY: &str = "events.replay";
pub const LATEST: &str = "events.latest";
pub const SUBSCRIBE: &str = "events.subscribe";
/// Stream notification carrying one envelope.
pub const EVENT_NOTIFICATION: &str = "events.event";
/// Stream notification carrying consumed durable progress, not an event.
pub const WATERMARK_NOTIFICATION: &str = "events.watermark";
/// Final stream notification when the subscription cannot continue.
pub const ERROR_NOTIFICATION: &str = "events.error";

/// Encoded bytes per replay page, leaving headroom under the 1 MiB message limit.
const REPLAY_BYTE_BUDGET: usize = 768 * 1024;
/// How often a quiet stream checks whether its connection is still open.
const STREAM_POLL: Duration = Duration::from_millis(200);

/// Params of `events.replay`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplayParams {
    /// Last acknowledged cursor; `0` replays from the beginning.
    #[serde(default)]
    pub after: Cursor,
    #[serde(default)]
    pub filter: EventFilter,
    /// Page size from 1 to [`MAX_PAGE_SIZE`] (default). Pages are also cut short
    /// to fit the IPC message limit; `has_more` tells the caller to continue.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    MAX_PAGE_SIZE
}

/// Result of `events.latest`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LatestCursor {
    /// Newest durable cursor ever assigned (`0` when none).
    pub latest_cursor: Cursor,
    /// Oldest retained cursor, `null` when nothing is retained.
    pub oldest_cursor: Option<Cursor>,
}

/// Params of `events.subscribe`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubscribeParams {
    #[serde(default)]
    pub after: Cursor,
    #[serde(default)]
    pub filter: EventFilter,
}

/// Initial `data` of an `events.subscribe` response.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subscribed {
    pub after: Cursor,
    /// Newest durable cursor when the subscription was registered.
    pub latest_cursor: Cursor,
}

/// Payload of an ordered `events.watermark` notification.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StreamWatermark {
    /// Every durable event through here matched and was sent earlier, or did not
    /// match this subscription's filter. Acknowledge after preceding deliveries.
    pub cursor: Cursor,
}

/// Payload of an `events.error` notification.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StreamError {
    /// `cursor_expired` or `internal`.
    pub reason: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_available: Option<Cursor>,
}

fn to_rpc(error: EventError) -> RpcError {
    let message = error.to_string();
    match error {
        EventError::CursorExpired {
            requested,
            oldest_available,
        } => RpcError::service(error.code(), message).with_data(json!({
            "reason": "cursor_expired",
            "requested": requested,
            "oldest_available": oldest_available,
        })),
        EventError::CursorAhead { requested, latest } => RpcError::invalid_params(message)
            .with_data(json!({
                "reason": "cursor_ahead",
                "requested": requested,
                "latest": latest,
            })),
        EventError::InvalidCursor
        | EventError::InvalidLimit { .. }
        | EventError::InvalidFilter(_) => RpcError::invalid_params(message),
        other => RpcError::service(other.code(), message),
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(REPLAY, move |_context, params| {
        let params: ReplayParams = parse_params(params)?;
        let page = shared
            .events()
            .replay_within(
                &params.filter,
                params.after,
                params.limit,
                REPLAY_BYTE_BUDGET,
            )
            .map_err(to_rpc)?;
        to_result(&page)
    })?;
    let shared = Arc::clone(services);
    router.register(LATEST, move |_context, params| {
        crate::rpc::expect_no_params(&params)?;
        let events = shared.events();
        to_result(&LatestCursor {
            latest_cursor: events.latest_cursor().map_err(to_rpc)?,
            oldest_cursor: events.oldest_cursor().map_err(to_rpc)?,
        })
    })?;
    let shared = Arc::clone(services);
    router.register_stream(SUBSCRIBE, move |_context, params, sink| {
        let params: SubscribeParams = parse_params(params)?;
        let events = shared.events();
        let subscription = events
            .subscribe(params.filter, params.after)
            .map_err(to_rpc)?;
        let latest_cursor = events.latest_cursor().map_err(to_rpc)?;
        thread::Builder::new()
            .name("aidexd-events-stream".into())
            .spawn(move || pump(subscription, &sink))
            .map_err(|error| RpcError::internal(format!("cannot start event stream: {error}")))?;
        to_result(&Subscribed {
            after: params.after,
            latest_cursor,
        })
    })
}

/// Forward ordered events and watermarks until the client unsubscribes or disconnects. Dropping the
/// sink afterwards ends the stream with `aidex.unsubscribed`.
fn pump(mut subscription: Subscription, sink: &NotificationSink) {
    while !sink.is_closed() {
        match subscription.next_timeout(STREAM_POLL) {
            Ok(Some(item)) => {
                let (method, data) = match item.event {
                    Some(envelope) => (EVENT_NOTIFICATION, serde_json::to_value(envelope)),
                    None => (
                        WATERMARK_NOTIFICATION,
                        serde_json::to_value(StreamWatermark {
                            cursor: item.cursor,
                        }),
                    ),
                };
                let data = match data {
                    Ok(data) => data,
                    Err(error) => {
                        notify_error(sink, "internal", error.to_string(), None);
                        return;
                    }
                };
                if sink.notify(method, data).is_err() {
                    return;
                }
            }
            Ok(None) => {}
            Err(error) => {
                let (reason, oldest) = match &error {
                    EventError::CursorExpired {
                        oldest_available, ..
                    } => ("cursor_expired", Some(*oldest_available)),
                    _ => ("internal", None),
                };
                notify_error(sink, reason, error.to_string(), oldest);
                return;
            }
        }
    }
}

fn notify_error(
    sink: &NotificationSink,
    reason: &str,
    message: String,
    oldest_available: Option<Cursor>,
) {
    let error = StreamError {
        reason: reason.into(),
        message,
        oldest_available,
    };
    if let Ok(data) = serde_json::to_value(error) {
        let _ = sink.notify(ERROR_NOTIFICATION, data);
    }
}

/// Decode an `events.event` notification payload.
pub fn decode_event(data: Value) -> Result<EventEnvelope, EventError> {
    serde_json::from_value(data).map_err(|error| EventError::Decode(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::ErrorCode,
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_core::EntityId;
    use aidex_events::{EventKind, EventScope};

    fn fixture(published: &[&str]) -> (tempfile::TempDir, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let workspace = EntityId::new("ws_main").unwrap();
        for task_id in published {
            services
                .events()
                .append(
                    EventScope::workspace(workspace.clone()),
                    EventKind::TaskStarted {
                        task_id: EntityId::new(*task_id).unwrap(),
                    },
                )
                .unwrap();
        }
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        (root, router, context)
    }

    fn page(router: &Router, context: &CallContext, params: Value) -> ReplayPage {
        serde_json::from_value(router.call(context, REPLAY, params).unwrap()).unwrap()
    }

    #[test]
    fn replay_returns_events_after_the_cursor_in_order() {
        let (_root, router, context) = fixture(&["a", "b", "c"]);
        let replay = page(&router, &context, json!({"after": 1}));
        let cursors: Vec<_> = replay
            .events
            .iter()
            .map(|event| event.cursor.get())
            .collect();
        assert_eq!(cursors, [2, 3]);
        assert!(!replay.has_more);
        let limited = page(&router, &context, json!({"limit": 1}));
        assert_eq!(limited.events.len(), 1);
        assert!(limited.has_more);
        assert_eq!(limited.next_cursor, Cursor::new(1));
        let filtered = page(
            &router,
            &context,
            json!({"filter": {"workspace_id": "ws_other", "kinds": ["task"]}}),
        );
        assert!(filtered.events.is_empty());
        assert_eq!(filtered.next_cursor, Cursor::new(3));
        let latest: LatestCursor =
            serde_json::from_value(router.call(&context, LATEST, Value::Null).unwrap()).unwrap();
        assert_eq!(
            latest,
            LatestCursor {
                latest_cursor: Cursor::new(3),
                oldest_cursor: Some(Cursor::new(1)),
            }
        );
    }

    #[test]
    fn invalid_queries_are_invalid_params() {
        let (_root, router, context) = fixture(&["a"]);
        for params in [
            json!({"limit": 0}),
            json!({"limit": MAX_PAGE_SIZE + 1}),
            json!({"filter": {"kinds": ["nope"]}}),
            json!({"filter": {"workspace_id": "bad id!"}}),
            json!({"filter": {"unknown": 1}}),
            json!({"after": -1}),
            json!({"after": 5}),
            json!({"workspace_id": "workspace", "after": 0}),
        ] {
            let error = router.call(&context, REPLAY, params.clone()).unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams), "{params}");
        }
        let error = router.call(&context, LATEST, json!({"x": 1})).unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
    }
}
