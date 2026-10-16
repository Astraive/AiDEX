//! `aidex vault ...`: create, unlock, lock, and inspect the drive vault; store,
//! list, and delete secrets; change the passphrase.
//!
//! Passphrases, recovery keys, and secret values are read from the terminal without
//! echo, or from stdin when `--passphrase-stdin` / `--from-stdin` is given. They are
//! never accepted from argv or environment variables, and are never printed (except
//! the newly created recovery key, shown exactly once by `aidex vault init`).

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::{
    Client, ClientError,
    methods::vault::{
        DeleteResult, INIT, InitResult, LOCK, LockResult, PASSPHRASE_CHANGE, PassphraseChanged,
        SECRET_DELETE, SECRET_LIST, SECRET_SET, STATUS, SecretList, SecretMeta, UNLOCK,
        UnlockMethod, VaultState, VaultStatus,
    },
};
use clap::Subcommand;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::{
    io::{self, BufRead, IsTerminal, Read},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Debug, Subcommand)]
pub enum VaultCommand {
    /// Create the drive's vault and unlock it (prompts for a new passphrase).
    Init {
        /// Also create a recovery key; it is printed once and never stored in plaintext.
        #[arg(long)]
        recovery_key: bool,
        /// Read the passphrase from the first line of stdin instead of the terminal.
        #[arg(long)]
        passphrase_stdin: bool,
        /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Unlock the vault for the running daemon.
    Unlock {
        /// Unlock with the recovery key instead of the passphrase.
        #[arg(long)]
        recovery_key: bool,
        /// Read the passphrase (or recovery key) from the first line of stdin.
        #[arg(long)]
        passphrase_stdin: bool,
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Lock the vault, zeroize its keys, and revoke every issued secret handle.
    Lock {
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Report whether the vault is missing, locked, unlocked, or unavailable.
    Status {
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Store or replace a secret (prompts for the value without echo).
    Set {
        /// Secret name: letters, digits, `.`, `_`, `-` (up to 128 characters).
        name: String,
        /// Consumer that may receive the secret through a broker handle:
        /// `provider:<id>`, `mcp:<server-id>`, `process:<tool-id>`, or `git:<host>`.
        /// Repeat for several. Model prompts and shell environments are never allowed.
        #[arg(long = "consumer", value_name = "SCOPE", required = true)]
        consumers: Vec<String>,
        /// api_key, token, password, ssh_key, private_key, or generic.
        #[arg(long, default_value = "generic")]
        kind: String,
        /// Read the value from stdin (all input; one trailing newline is removed).
        #[arg(long)]
        from_stdin: bool,
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// List secret names, kinds, and consumers (values are never shown).
    List {
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Delete a secret; its handles stop resolving immediately.
    Delete {
        name: String,
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Manage the vault passphrase.
    Passphrase {
        #[command(subcommand)]
        command: PassphraseCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum PassphraseCommand {
    /// Change the passphrase (after a recovery-key unlock the old one is not needed).
    Change {
        /// Read the old and new passphrases from the first two lines of stdin (only the
        /// new one after a recovery-key unlock).
        #[arg(long)]
        passphrase_stdin: bool,
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
}

impl VaultCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Init { .. } => "aidex vault init",
            Self::Unlock { .. } => "aidex vault unlock",
            Self::Lock { .. } => "aidex vault lock",
            Self::Status { .. } => "aidex vault status",
            Self::Set { .. } => "aidex vault set",
            Self::List { .. } => "aidex vault list",
            Self::Delete { .. } => "aidex vault delete",
            Self::Passphrase { .. } => "aidex vault passphrase change",
        }
    }
}

pub fn run(command: VaultCommand, output: Output) -> Result<Exit, CliError> {
    match command {
        VaultCommand::Init {
            recovery_key,
            passphrase_stdin,
            drive,
        } => init(recovery_key, passphrase_stdin, drive.as_deref(), output),
        VaultCommand::Unlock {
            recovery_key,
            passphrase_stdin,
            drive,
        } => unlock(recovery_key, passphrase_stdin, drive.as_deref(), output),
        VaultCommand::Lock { drive } => lock(drive.as_deref(), output),
        VaultCommand::Status { drive } => status(drive.as_deref(), output),
        VaultCommand::Set {
            name,
            consumers,
            kind,
            from_stdin,
            drive,
        } => set(name, consumers, kind, from_stdin, drive.as_deref(), output),
        VaultCommand::List { drive } => list(drive.as_deref(), output),
        VaultCommand::Delete { name, drive } => delete(name, drive.as_deref(), output),
        VaultCommand::Passphrase {
            command:
                PassphraseCommand::Change {
                    passphrase_stdin,
                    drive,
                },
        } => change_passphrase(passphrase_stdin, drive.as_deref(), output),
    }
}

type Secret = Zeroizing<String>;

/// Where secret input comes from. Decided before connecting so usage errors are immediate.
enum Source {
    Terminal,
    Stdin,
}

impl Source {
    fn choose(from_stdin: bool, flag: &str) -> Result<Self, CliError> {
        if from_stdin {
            Ok(Self::Stdin)
        } else if io::stdin().is_terminal() {
            Ok(Self::Terminal)
        } else {
            Err(CliError::new(
                Exit::Usage,
                "stdin is not a terminal, so secrets cannot be prompted for",
            )
            .with_hint(format!(
                "pipe the secret on stdin and pass `{flag}`; secrets are never accepted as arguments or environment variables"
            )))
        }
    }
}

/// Reads secret lines from one source, keeping stdin buffered across reads.
struct SecretReader {
    source: Source,
    stdin: Option<io::StdinLock<'static>>,
}

impl SecretReader {
    fn new(source: Source) -> Self {
        Self {
            source,
            stdin: None,
        }
    }

    /// One secret line: a no-echo terminal prompt, or the next stdin line.
    fn line(&mut self, prompt: &str) -> Result<Secret, CliError> {
        match self.source {
            Source::Terminal => rpassword::prompt_password(prompt)
                .map(Zeroizing::new)
                .map_err(|error| {
                    CliError::new(
                        Exit::Failure,
                        format!("cannot read from the terminal: {error}"),
                    )
                }),
            Source::Stdin => {
                let stdin = self.stdin.get_or_insert_with(|| io::stdin().lock());
                let mut line = Zeroizing::new(String::new());
                let read = stdin.read_line(&mut line).map_err(|error| {
                    CliError::new(
                        Exit::Usage,
                        format!("cannot read stdin as UTF-8 text: {error}"),
                    )
                })?;
                if read == 0 {
                    return Err(CliError::new(
                        Exit::Usage,
                        format!(
                            "stdin ended before the expected input ({})",
                            prompt.trim_end_matches([':', ' '])
                        ),
                    ));
                }
                strip_newline(&mut line);
                Ok(line)
            }
        }
    }

    /// A new passphrase; on a terminal it is typed twice.
    fn new_passphrase(&mut self, prompt: &str) -> Result<Secret, CliError> {
        let first = self.line(prompt)?;
        if matches!(self.source, Source::Terminal) {
            let second = self.line("Repeat the passphrase: ")?;
            if *first != *second {
                return Err(CliError::new(Exit::Usage, "the passphrases do not match"));
            }
        }
        Ok(first)
    }

    /// A secret value: a no-echo prompt, or all of stdin.
    fn value(&mut self, prompt: &str) -> Result<Secret, CliError> {
        match self.source {
            Source::Terminal => self.line(prompt),
            Source::Stdin => {
                let mut value = Zeroizing::new(String::new());
                io::stdin()
                    .lock()
                    .read_to_string(&mut value)
                    .map_err(|error| {
                        CliError::new(
                            Exit::Usage,
                            format!("cannot read the secret from stdin as UTF-8 text: {error}"),
                        )
                    })?;
                strip_newline(&mut value);
                Ok(value)
            }
        }
    }
}

/// Remove one trailing `\n` or `\r\n`.
fn strip_newline(text: &mut String) {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
}

/// Build params whose string fields hold secrets. The JSON copy handed to the client
/// is not zeroized (IPC buffers are best-effort; see crates/vault/FORMAT.md).
fn params(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<Map<_, _>>(),
    )
}

/// Call a vault method, adding next-step hints for vault states.
fn call<T: DeserializeOwned>(
    client: &mut Client,
    method: &str,
    params: Value,
) -> Result<T, CliError> {
    client.call_as(method, params).map_err(vault_cli_error)
}

fn vault_cli_error(error: ClientError) -> CliError {
    let reason = match &error {
        ClientError::Rpc { data, .. } => data
            .as_ref()
            .and_then(|data| data.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    };
    let cli = CliError::from(error);
    match reason.as_deref() {
        Some("locked") => cli.with_hint("unlock it with `aidex vault unlock`"),
        Some("vault_not_found") => cli.with_hint("create one with `aidex vault init`"),
        Some("wrong_passphrase") => cli
            .with_hint("check the passphrase, or unlock with `aidex vault unlock --recovery-key`"),
        Some("missing_header" | "corrupt" | "tampered") => cli.with_hint(
            "the vault data is damaged; restore the vault directory from an encrypted backup",
        ),
        _ => cli,
    }
}

fn init(
    recovery: bool,
    from_stdin: bool,
    drive: Option<&Path>,
    output: Output,
) -> Result<Exit, CliError> {
    let mut reader = SecretReader::new(Source::choose(from_stdin, "--passphrase-stdin")?);
    let mut client = connect(drive)?;
    let passphrase = reader.new_passphrase("New vault passphrase: ")?;
    let result: InitResult = call(
        &mut client,
        INIT,
        params(vec![
            ("passphrase", Value::String(passphrase.to_string())),
            ("recovery", Value::Bool(recovery)),
        ]),
    )?;
    output.emit(&result, || {
        println!("Vault created and unlocked ({})", result.vault_id);
        if let Some(key) = &result.recovery_key {
            println!();
            println!("Recovery key (shown only once):");
            println!("  {key}");
            println!();
            println!("Store it offline. It unlocks the vault without the passphrase;");
            println!("without the passphrase or this key the vault cannot be recovered.");
        }
    })?;
    Ok(Exit::Success)
}

fn unlock(
    recovery: bool,
    from_stdin: bool,
    drive: Option<&Path>,
    output: Output,
) -> Result<Exit, CliError> {
    let mut reader = SecretReader::new(Source::choose(from_stdin, "--passphrase-stdin")?);
    let mut client = connect(drive)?;
    let request = if recovery {
        let key = reader.line("Recovery key: ")?;
        params(vec![("recovery_key", Value::String(key.to_string()))])
    } else {
        let passphrase = reader.line("Vault passphrase: ")?;
        params(vec![("passphrase", Value::String(passphrase.to_string()))])
    };
    let status: VaultStatus = call(&mut client, UNLOCK, request)?;
    output.emit(&status, || print_status(&status))?;
    Ok(Exit::Success)
}

fn lock(drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let result: LockResult = call(&mut connect(drive)?, LOCK, Value::Null)?;
    output.emit(&result, || {
        if result.was_unlocked {
            println!(
                "Vault: locked (revoked {} secret handle{})",
                result.revoked_handles,
                if result.revoked_handles == 1 { "" } else { "s" }
            );
        } else {
            println!("Vault: {}", state_name(result.state));
        }
    })?;
    Ok(Exit::Success)
}

fn status(drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let status: VaultStatus = call(&mut connect(drive)?, STATUS, Value::Null)?;
    output.emit(&status, || print_status(&status))?;
    Ok(Exit::Success)
}

fn set(
    name: String,
    consumers: Vec<String>,
    kind: String,
    from_stdin: bool,
    drive: Option<&Path>,
    output: Output,
) -> Result<Exit, CliError> {
    let mut reader = SecretReader::new(Source::choose(from_stdin, "--from-stdin")?);
    let mut client = connect(drive)?;
    let value = reader.value(&format!("Value for `{name}`: "))?;
    let request = params(vec![
        ("name", Value::String(name)),
        ("value", Value::String(value.to_string())),
        (
            "consumers",
            Value::Array(consumers.into_iter().map(Value::String).collect()),
        ),
        ("kind", Value::String(kind)),
    ]);
    drop(value);
    let meta: SecretMeta = call(&mut client, SECRET_SET, request)?;
    output.emit(&meta, || {
        println!(
            "Stored secret `{}` ({}) for: {}",
            meta.name,
            meta.kind,
            consumers_text(&meta)
        );
    })?;
    Ok(Exit::Success)
}

fn list(drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let list: SecretList = call(&mut connect(drive)?, SECRET_LIST, Value::Null)?;
    output.emit(&list, || {
        if list.secrets.is_empty() {
            println!("No secrets stored.");
        }
        for line in list_lines(&list) {
            println!("{line}");
        }
    })?;
    Ok(Exit::Success)
}

/// One line per secret. Records AiDEX manages (`managed`) are marked: `aidex
/// vault set` and `aidex vault delete` refuse them.
fn list_lines(list: &SecretList) -> Vec<String> {
    list.secrets
        .iter()
        .map(|meta| {
            let managed = if list.managed.contains(&meta.name) {
                "  (managed by AiDEX)"
            } else {
                ""
            };
            format!(
                "{}  {}  {}  updated {}{managed}",
                meta.name,
                meta.kind,
                consumers_text(meta),
                meta.updated_at.as_str()
            )
        })
        .collect()
}

fn delete(name: String, drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let result: DeleteResult = call(
        &mut connect(drive)?,
        SECRET_DELETE,
        params(vec![("name", Value::String(name))]),
    )?;
    output.emit(&result, || {
        println!("Deleted secret `{}`", result.deleted.name)
    })?;
    Ok(Exit::Success)
}

fn change_passphrase(
    from_stdin: bool,
    drive: Option<&Path>,
    output: Output,
) -> Result<Exit, CliError> {
    let mut reader = SecretReader::new(Source::choose(from_stdin, "--passphrase-stdin")?);
    let mut client = connect(drive)?;
    let status: VaultStatus = call(&mut client, STATUS, Value::Null)?;
    let mut fields = Vec::new();
    if status.unlocked_by != Some(UnlockMethod::RecoveryKey) {
        let old = reader.line("Current passphrase: ")?;
        fields.push(("old_passphrase", Value::String(old.to_string())));
    }
    let new = reader.new_passphrase("New passphrase: ")?;
    fields.push(("new_passphrase", Value::String(new.to_string())));
    let result: PassphraseChanged = call(&mut client, PASSPHRASE_CHANGE, params(fields))?;
    output.emit(&result, || println!("Vault passphrase changed"))?;
    Ok(Exit::Success)
}

fn state_name(state: VaultState) -> &'static str {
    match state {
        VaultState::Missing => "missing",
        VaultState::Locked => "locked",
        VaultState::Unlocked => "unlocked",
        VaultState::Unavailable => "unavailable",
    }
}

fn consumers_text(meta: &SecretMeta) -> String {
    if meta.policy.allowed_consumers.is_empty() {
        return "(no consumers)".into();
    }
    meta.policy
        .allowed_consumers
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_status(status: &VaultStatus) {
    println!("Vault: {}", state_name(status.state));
    if let Some(vault_id) = &status.vault_id {
        println!("Vault ID: {vault_id}");
    }
    if let Some(secrets) = status.secrets {
        println!("Secrets: {secrets}");
    }
    if let Some(recovery) = status.has_recovery_key {
        println!(
            "Recovery key: {}",
            if recovery { "configured" } else { "none" }
        );
    }
    if let Some(method) = status.unlocked_by {
        let method = match method {
            UnlockMethod::Passphrase => "passphrase",
            UnlockMethod::RecoveryKey => "recovery key",
        };
        println!("Unlocked by: {method}");
    }
    if let Some(at) = &status.unlocked_at {
        println!("Unlocked at: {}", at.as_str());
    }
    match status.idle_lock_secs {
        Some(secs) => println!("Auto-lock: after {} min idle", secs.div_ceil(60)),
        None => println!("Auto-lock: off"),
    }
    if let Some(message) = &status.message {
        println!("Problem: {message}");
    }
    if status.state == VaultState::Missing {
        println!("Create one with `aidex vault init`.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(name: &str, kind: &str) -> Value {
        json!({
            "secret_ref": {"vault_id": "01J00000000000000000000000", "secret_id": "01J00000000000000000000001"},
            "name": name,
            "kind": kind,
            "created_at": "2026-10-13T00:00:00Z",
            "updated_at": "2026-10-13T00:00:00Z",
            "policy": {"allowed_consumers": []},
        })
    }

    #[test]
    fn listed_secrets_mark_the_records_aidex_manages() {
        let list: SecretList = serde_json::from_value(json!({
            "secrets": [meta("aidex-preferences", "generic"), meta("openai", "api_key")],
            "managed": ["aidex-preferences"],
        }))
        .unwrap();
        let lines = list_lines(&list);
        assert!(
            lines[0].starts_with("aidex-preferences  generic  (no consumers)")
                && lines[0].ends_with("  (managed by AiDEX)"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("openai  api_key") && !lines[1].contains("managed"),
            "{lines:?}"
        );
        // A daemon that predates `managed` lists nothing as managed.
        let older: SecretList =
            serde_json::from_value(json!({"secrets": [meta("openai", "api_key")]})).unwrap();
        assert!(older.managed.is_empty());
        assert!(!list_lines(&older)[0].contains("managed"));
    }

    #[test]
    fn one_trailing_newline_is_removed() {
        for (input, expected) in [
            ("value\n", "value"),
            ("value\r\n", "value"),
            ("value", "value"),
            ("value\n\n", "value\n"),
            ("", ""),
        ] {
            let mut text = input.to_owned();
            strip_newline(&mut text);
            assert_eq!(text, expected, "{input:?}");
        }
    }
}
