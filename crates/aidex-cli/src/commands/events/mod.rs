//! `aidex events tail|show`: print the drive's durable event log.
//!
//! `show` prints one replay page (`events.replay`). `tail` prints everything after a
//! cursor and exits, or with `--follow` streams `events.subscribe`, reconnecting with
//! backoff after transient disconnects and resuming after the last processed
//! durable event or consumed watermark.
//! Human output is one line per event; `--json` prints envelopes as JSON lines
//! (`show --json` prints the page as one JSON document).

mod follow;
mod render;

use crate::{
    commands::daemon::connect_root,
    exit::{CliError, Exit},
    output::Output,
};
use aidex_core::EntityId;
use aidex_drive::resolve_root;
use aidex_events::{Cursor, EventFilter, MAX_PAGE_SIZE};
use aidexd::{
    Client, ClientError,
    methods::events::{REPLAY, ReplayPage},
};
use clap::{Args, Subcommand};
use serde_json::json;
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    /// Print events after a cursor; with --follow keep streaming and reconnect.
    Tail(TailArgs),
    /// Print one page of events after a cursor.
    Show(ShowArgs),
}

/// Event selection shared by `tail` and `show`.
#[derive(Debug, Args)]
pub struct FilterArgs {
    /// Only events scoped to this workspace.
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Only events scoped to this session.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Only events whose name starts with this dotted prefix (repeatable), e.g. `tool`.
    #[arg(long = "kind", value_name = "PREFIX")]
    pub kinds: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TailArgs {
    /// Print events strictly after this cursor (0 = from the oldest retained).
    #[arg(long, default_value_t = 0, value_name = "CURSOR")]
    pub after: u64,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// Keep streaming live events; reconnect after daemon restarts.
    #[arg(long)]
    pub follow: bool,
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Show events strictly after this cursor.
    #[arg(long, default_value_t = 0, value_name = "CURSOR")]
    pub after: u64,
    /// Events per page (1-256).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

impl EventsCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Tail(_) => "aidex events tail",
            Self::Show(_) => "aidex events show",
        }
    }
}

pub fn run(command: EventsCommand, output: Output) -> Result<Exit, CliError> {
    match command {
        EventsCommand::Tail(args) => tail(args, output),
        EventsCommand::Show(args) => show(args, output),
    }
}

fn entity(flag: &str, value: Option<String>) -> Result<Option<EntityId>, CliError> {
    value
        .map(|value| {
            EntityId::new(value)
                .map_err(|error| CliError::new(Exit::Usage, format!("invalid --{flag}: {error}")))
        })
        .transpose()
}

impl FilterArgs {
    fn into_filter(self) -> Result<EventFilter, CliError> {
        let filter = EventFilter {
            workspace_id: entity("workspace", self.workspace)?,
            session_id: entity("session", self.session)?,
            kinds: self.kinds,
        };
        filter
            .validate()
            .map_err(|error| CliError::new(Exit::Usage, error.to_string()))?;
        Ok(filter)
    }
}

/// Map a client error, adding the resume hint for an expired cursor.
pub(crate) fn event_error(error: ClientError) -> CliError {
    let oldest = match &error {
        ClientError::Rpc {
            data: Some(data), ..
        } if data["reason"] == "cursor_expired" => data["oldest_available"].as_u64(),
        _ => None,
    };
    let cli = CliError::from(error);
    match oldest {
        Some(oldest) => cli.with_hint(format!(
            "history before cursor {oldest} was removed by retention; rerun with `--after {}` to continue from the oldest retained event",
            oldest.saturating_sub(1)
        )),
        None => cli,
    }
}

fn replay(
    client: &mut Client,
    filter: &EventFilter,
    after: Cursor,
    limit: usize,
) -> Result<ReplayPage, CliError> {
    client
        .call_as(
            REPLAY,
            json!({ "after": after, "filter": filter, "limit": limit }),
        )
        .map_err(event_error)
}

fn write_error(error: io::Error) -> CliError {
    CliError::new(Exit::Failure, format!("cannot write events: {error}"))
}

fn show(args: ShowArgs, output: Output) -> Result<Exit, CliError> {
    if args.limit == 0 || args.limit > MAX_PAGE_SIZE {
        return Err(CliError::new(
            Exit::Usage,
            format!("--limit must be between 1 and {MAX_PAGE_SIZE}"),
        ));
    }
    let filter = args.filter.into_filter()?;
    let root = resolve_root(args.drive.as_deref())?;
    let mut client = connect_root(&root)?;
    let page = replay(&mut client, &filter, Cursor::new(args.after), args.limit)?;
    if output.is_json() {
        output.emit(&page, || {})?;
    } else {
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        for event in &page.events {
            render::write_event(&mut stdout, event, false).map_err(write_error)?;
        }
        let more = if page.has_more {
            format!(" (more: aidex events show --after {})", page.next_cursor)
        } else {
            String::new()
        };
        writeln!(stdout, "next cursor: {}{more}", page.next_cursor).map_err(write_error)?;
        stdout.flush().map_err(write_error)?;
    }
    Ok(Exit::Success)
}

fn tail(args: TailArgs, output: Output) -> Result<Exit, CliError> {
    let filter = args.filter.into_filter()?;
    let root = resolve_root(args.drive.as_deref())?;
    let after = Cursor::new(args.after);
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let json = output.is_json();
    if args.follow {
        let mut source = follow::DaemonSource::new(root);
        return follow::follow(
            &mut source,
            after,
            &filter,
            &mut |event| render::write_event(&mut stdout, event, json),
            &mut follow::RealClock,
        );
    }
    let mut client = connect_root(&root)?;
    let mut after = after;
    loop {
        let page = replay(&mut client, &filter, after, MAX_PAGE_SIZE)?;
        for event in &page.events {
            render::write_event(&mut stdout, event, json).map_err(write_error)?;
        }
        after = page.next_cursor;
        if !page.has_more {
            return Ok(Exit::Success);
        }
    }
}
