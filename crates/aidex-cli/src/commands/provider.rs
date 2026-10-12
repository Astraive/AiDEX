//! `aidex provider add|list|remove|health`: configure model providers.
//!
//! A provider authenticates with a vault secret *name* (`--secret`); the key
//! itself is stored with `aidex vault set <NAME> --consumer provider:<ID>` and
//! is never accepted on the command line.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidex_providers::{
    Locality, ModelConfig, ModelId, ModelRole, ProviderEntry, ProviderKind, ToolCallSupport,
};
use aidexd::methods::models::{
    MODEL_PROVIDERS, PROVIDER_ADD, PROVIDER_HEALTH, PROVIDER_REMOVE, ProviderHealthResult,
    ProviderList, ProviderRemoved,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct ProviderArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: ProviderCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// Add an OpenAI-compatible provider (OpenAI, vLLM, llama-server, ...).
    Add {
        /// Provider id: lowercase letters, digits, `-`, `_`.
        id: String,
        #[arg(long, default_value = "openai-compatible")]
        kind: ProviderKind,
        /// API root including the version segment, e.g. https://api.openai.com/v1.
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// Vault secret holding the API key (its policy must allow provider:<ID>).
        #[arg(long = "secret", value_name = "NAME")]
        secret_name: Option<String>,
        /// Treat a loopback endpoint as remote (for example a tunnel to a cloud API).
        #[arg(long)]
        remote: bool,
        /// Do not query /models; offer only the models declared with --model.
        #[arg(long)]
        no_discover: bool,
        /// Declare a model the provider serves (repeatable); the flags below apply to each.
        #[arg(long = "model", value_name = "MODEL_ID")]
        models: Vec<String>,
        /// Context window of declared models.
        #[arg(long, value_name = "TOKENS")]
        context_window: Option<u32>,
        /// Maximum output tokens of declared models.
        #[arg(long, value_name = "TOKENS")]
        max_output: Option<u32>,
        /// Tool support of declared models: native, structured, or none.
        #[arg(long, value_name = "SUPPORT", value_parser = parse_tools, default_value = "none")]
        tools: ToolCallSupport,
        /// Declared models accept JSON-schema constrained output.
        #[arg(long)]
        json_schema: bool,
        /// Roles of declared models (repeatable): code, plan, summarize, classify, embed, verify, general.
        #[arg(long = "role", value_name = "ROLE")]
        roles: Vec<ModelRole>,
    },
    /// List providers, their endpoints, locality, and authentication.
    List,
    /// Remove a configured provider.
    Remove { id: String },
    /// Check that a provider answers (refused, without traffic, when offline).
    Health { id: String },
}

fn parse_tools(text: &str) -> Result<ToolCallSupport, String> {
    match text {
        "native" => Ok(ToolCallSupport::Native),
        "structured" => Ok(ToolCallSupport::Structured),
        "none" => Ok(ToolCallSupport::None),
        _ => Err("use native, structured, or none".into()),
    }
}

impl ProviderCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Add { .. } => "aidex provider add",
            Self::List => "aidex provider list",
            Self::Remove { .. } => "aidex provider remove",
            Self::Health { .. } => "aidex provider health",
        }
    }
}

fn print_entry(entry: &ProviderEntry) {
    let descriptor = &entry.descriptor;
    let auth = match &descriptor.auth {
        aidex_providers::AuthConfig::None => "none".to_owned(),
        aidex_providers::AuthConfig::VaultSecret { secret_name } => {
            format!("vault secret `{secret_name}`")
        }
    };
    // Account metadata from provider-accounts.toml: the name and a disabled flag.
    let name = entry
        .display_name
        .as_deref()
        .map(|name| format!("  \"{name}\""))
        .unwrap_or_default();
    println!(
        "{}  {}  {}  {}  auth: {}  ({}){}{}",
        descriptor.id,
        descriptor.kind.as_str(),
        descriptor.locality.as_str(),
        descriptor.base_url.as_deref().unwrap_or("-"),
        auth,
        match entry.origin {
            aidex_providers::ProviderOrigin::Configured => "configured",
            aidex_providers::ProviderOrigin::Runtime => "runtime",
        },
        name,
        if entry.enabled { "" } else { "  disabled" },
    );
}

pub fn run(args: ProviderArgs, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(args.drive.as_deref())?;
    match args.command {
        ProviderCommand::Add {
            id,
            kind,
            base_url,
            secret_name,
            remote,
            no_discover,
            models,
            context_window,
            max_output,
            tools,
            json_schema,
            roles,
        } => {
            let declared = if models.is_empty() {
                Vec::new()
            } else {
                let window = context_window.ok_or_else(|| {
                    CliError::new(Exit::Usage, "declared models need --context-window")
                })?;
                models
                    .iter()
                    .map(|model| {
                        let mut config = ModelConfig::new(
                            ModelId::new(model.as_str())
                                .map_err(|error| CliError::new(Exit::Usage, error.to_string()))?,
                            window,
                        );
                        config.max_output_tokens = max_output;
                        config.tool_calls = tools;
                        config.json_schema_output = json_schema;
                        config.roles.clone_from(&roles);
                        Ok(config)
                    })
                    .collect::<Result<Vec<_>, CliError>>()?
            };
            let entry: ProviderEntry = client.call_as(
                PROVIDER_ADD,
                json!({
                    "id": id,
                    "kind": kind,
                    "base_url": base_url,
                    "secret_name": secret_name,
                    "locality": remote.then_some(Locality::Remote),
                    "discover_models": !no_discover,
                    "models": declared,
                }),
            )?;
            output.emit(&entry, || {
                print!("Added provider ");
                print_entry(&entry);
                if let aidex_providers::AuthConfig::VaultSecret { secret_name } =
                    &entry.descriptor.auth
                {
                    println!(
                        "Store its key with `aidex vault set {secret_name} --consumer provider:{}`",
                        entry.descriptor.id
                    );
                }
            })?;
        }
        ProviderCommand::List => {
            let list: ProviderList = client.call_as(MODEL_PROVIDERS, Value::Null)?;
            output.emit(&list, || {
                if list.providers.is_empty() {
                    println!("No providers configured. Add one with `aidex provider add`.");
                }
                for entry in &list.providers {
                    print_entry(entry);
                }
            })?;
        }
        ProviderCommand::Remove { id } => {
            let removed: ProviderRemoved = client.call_as(PROVIDER_REMOVE, json!({ "id": id }))?;
            output.emit(&removed, || {
                println!("Removed provider {}", removed.removed)
            })?;
        }
        ProviderCommand::Health { id } => {
            let result: ProviderHealthResult =
                client.call_as(PROVIDER_HEALTH, json!({ "id": id }))?;
            output.emit(&result, || {
                let health = &result.health;
                let status = match health.status {
                    aidex_providers::HealthStatus::Ready => "ready",
                    aidex_providers::HealthStatus::Unavailable => "unavailable",
                    aidex_providers::HealthStatus::Refused => "refused",
                    aidex_providers::HealthStatus::AuthRequired => "auth_required",
                };
                match (health.latency_ms, &health.message) {
                    (Some(ms), _) => println!("{}: {status} ({ms} ms)", result.provider_id),
                    (None, Some(message)) => {
                        println!("{}: {status}: {message}", result.provider_id)
                    }
                    (None, None) => println!("{}: {status}", result.provider_id),
                }
            })?;
            if result.health.status != aidex_providers::HealthStatus::Ready {
                return Ok(Exit::ModelUnavailable);
            }
        }
    }
    Ok(Exit::Success)
}
