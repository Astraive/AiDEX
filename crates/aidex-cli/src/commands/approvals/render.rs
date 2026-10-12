//! Human-readable approval cards and lists (`--json` prints the records).

use aidexd::methods::approvals::{ApprovalRecord, ApprovalState};
use serde::Serialize;

/// The wire name of a serializable enum value (`"low"`, `"once"`, ...).
fn name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => "?".into(),
    }
}

pub fn state(record: &ApprovalRecord) -> String {
    match &record.state {
        ApprovalState::Pending => format!("pending until {}", record.card.expires_at.as_str()),
        ApprovalState::Approved { scope, by, at } => {
            let used = if record.consumed_at.is_some() {
                ", executed"
            } else {
                ""
            };
            let grant = match &record.grant {
                Some(grant) if grant.active => {
                    format!(", session grant active until {}", grant.expires_at.as_str())
                }
                Some(_) => ", session grant expired".to_owned(),
                None => String::new(),
            };
            format!("approved {scope} by {by} at {}{used}{grant}", at.as_str())
        }
        ApprovalState::Denied { by, at } => format!("denied by {by} at {}", at.as_str()),
        ApprovalState::Expired { at } => format!("expired at {}", at.as_str()),
        ApprovalState::Revoked { by, at, reason } => match reason {
            Some(reason) => format!("revoked by {by} at {} ({reason})", at.as_str()),
            None => format!("revoked by {by} at {}", at.as_str()),
        },
        ApprovalState::Cancelled { at, reason } => match reason {
            Some(reason) => format!("cancelled at {} ({reason})", at.as_str()),
            None => format!("cancelled at {}", at.as_str()),
        },
    }
}

pub fn list(records: &[ApprovalRecord]) {
    if records.is_empty() {
        println!("No approvals.");
        return;
    }
    for record in records {
        println!(
            "{}  {:<9} {:<10} {}",
            record.approval_id,
            name(&record.status()),
            record.card.tool_id,
            record.card.summary
        );
        println!("    {}", state(record));
    }
}

fn lines(record: &ApprovalRecord) -> Vec<String> {
    let card = &record.card;
    let mut out = vec![
        format!("{} wants to run `{}`:", card.requester, card.tool_id),
        String::new(),
        format!("  {}", card.summary),
        String::new(),
        format!("Effect:     {}", card.effect),
        format!(
            "Workspace:  {} ({})",
            card.workspace_name, card.workspace_id
        ),
        format!("Session:    {}", card.session_id),
        format!(
            "Risk:       {} (runs at least `{}` sandboxed)",
            name(&card.risk),
            card.sandbox
        ),
        format!("Policy:     {} ({})", card.reason, card.rule),
    ];
    if let Some(command) = &card.command {
        out.push(format!("Command:    {command}"));
    }
    if !card.targets.is_empty() {
        out.push(format!("Targets:    {}", card.targets.join(", ")));
    }
    for secret in &card.secrets {
        out.push(format!(
            "Secret:     `{}` for {} (value never shown)",
            secret.secret_name, secret.consumer
        ));
    }
    let scopes: Vec<String> = card.scope_options.iter().map(ToString::to_string).collect();
    out.push(format!("Scope:      {}", scopes.join(" or ")));
    if let Some(grant) = &card.session_grant {
        out.push(format!("  session:  {grant}"));
    }
    out.push(format!("Expires:    {}", card.expires_at.as_str()));
    out.push(format!("State:      {}", state(record)));
    for warning in &card.warnings {
        out.push(format!("Warning:    {warning}"));
    }
    if let Some(diff) = &card.diff_preview {
        out.push(String::new());
        out.push(format!(
            "Diff: {} file(s), +{} −{}{}",
            diff.files.len() as u64 + diff.files_omitted,
            diff.additions,
            diff.deletions,
            if diff.files_omitted > 0 {
                format!(" ({} not shown)", diff.files_omitted)
            } else {
                String::new()
            }
        ));
        for file in &diff.files {
            out.push(format!(
                "  {} {} (+{} −{}){}",
                file.kind,
                file.path,
                file.additions,
                file.deletions,
                if file.binary { " binary" } else { "" }
            ));
            if let Some(unified) = &file.unified {
                out.extend(unified.lines().map(|line| format!("    {line}")));
            }
            if file.truncated {
                out.push("    [diff truncated]".into());
            }
        }
    }
    out.push(String::new());
    out.push(format!(
        "Approve: aidex approve {}{}   Deny: aidex deny {}",
        record.approval_id,
        if card.session_grant.is_some() {
            " [--session]"
        } else {
            ""
        },
        record.approval_id
    ));
    out
}

pub fn card(record: &ApprovalRecord) {
    for line in lines(record) {
        println!("{line}");
    }
}

pub fn card_to_stderr(record: &ApprovalRecord) {
    for line in lines(record) {
        eprintln!("{line}");
    }
}
