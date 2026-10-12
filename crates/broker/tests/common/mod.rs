//! Shared fixtures for broker integration tests.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use aidex_broker::{
    Broker, BrokerConfig, BrokerOutcome, ManualClock, Origin, PolicySettings, StaticWorkspaces,
    ToolInvocation,
};
use aidex_core::{EntityId, Timestamp};
use aidex_events::{EventEnvelope, MemoryEventSink};
use aidex_policy::Profile;
use aidex_sandbox::Sandbox;
use aidex_tools::ToolId;
use aidex_vault::VaultService;
use aidex_workspace::ConfinedRoot;
use serde_json::Value;

pub const HELPER: &str = env!("CARGO_BIN_EXE_aidex-broker-test-sandbox-exec");
pub const WORKSPACE: &str = "wsp_test";
pub const SESSION: &str = "ses_test";
pub const AGENT: &str = "aidex_native";

pub fn id(text: &str) -> EntityId {
    EntityId::new(text).unwrap()
}

/// A broker over a temporary workspace with in-memory events and a manual clock.
pub struct Fixture {
    pub state: tempfile::TempDir,
    pub workspace: tempfile::TempDir,
    pub events: Arc<MemoryEventSink>,
    pub clock: Arc<ManualClock>,
    pub workspaces: Arc<StaticWorkspaces>,
    pub broker: Arc<Broker>,
}

pub struct Options {
    pub profile: Profile,
    /// Policy documents (`(file name, TOML)`).
    pub documents: Vec<(String, String)>,
    pub vault: Option<Arc<VaultService>>,
    /// Workspace-relative directories treated as protected.
    pub protected: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            documents: Vec::new(),
            vault: None,
            protected: Vec::new(),
        }
    }
}

impl Options {
    pub fn profile(profile: Profile) -> Self {
        Self {
            profile,
            ..Self::default()
        }
    }

    pub fn document(mut self, name: &str, text: &str) -> Self {
        self.documents.push((name.to_owned(), text.to_owned()));
        self
    }
}

impl Fixture {
    pub fn new(options: Options) -> Self {
        let state = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        Self::open(state, workspace, options)
    }

    /// Open a broker over existing state and workspace directories (restarts).
    pub fn open(state: tempfile::TempDir, workspace: tempfile::TempDir, options: Options) -> Self {
        let mut config = BrokerConfig::in_dir(state.path());
        config.default_settings = PolicySettings {
            profile: options.profile,
            ..PolicySettings::default()
        };
        let policies = state.path().join("policies");
        fs::create_dir_all(&policies).unwrap();
        for (name, text) in &options.documents {
            fs::write(policies.join(name), text).unwrap();
        }
        config.policies_dir = Some(policies);
        config.protected_paths = options
            .protected
            .iter()
            .map(|relative| {
                let dir = workspace.path().join(relative);
                fs::create_dir_all(&dir).unwrap();
                dir.canonicalize().unwrap()
            })
            .collect();
        let events = Arc::new(MemoryEventSink::new());
        let clock = Arc::new(ManualClock::new(&Timestamp::now()));
        let workspaces = Arc::new(StaticWorkspaces::new());
        workspaces.insert(
            id(WORKSPACE),
            "demo",
            ConfinedRoot::new(workspace.path()).unwrap(),
        );
        let mut builder = Broker::builder(config, workspaces.clone(), events.clone())
            .sandbox(Sandbox::new(HELPER))
            .clock(clock.clone());
        if let Some(vault) = options.vault {
            builder = builder.vault(vault);
        }
        let broker = Arc::new(builder.open().unwrap());
        Self {
            state,
            workspace,
            events,
            clock,
            workspaces,
            broker,
        }
    }

    pub fn root(&self) -> &Path {
        self.workspace.path()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.workspace.path().join(relative)
    }

    pub fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    pub fn submit(&self, tool: &str, input: Value) -> BrokerOutcome {
        self.broker.submit(agent_call(tool, input)).unwrap()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.events.names()
    }

    /// Every recorded event as JSON text (for secret greps).
    pub fn event_text(&self) -> String {
        self.events
            .events()
            .iter()
            .map(|envelope: &EventEnvelope| serde_json::to_string(envelope).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The `rule` of every `policy.decision` event, in order.
    pub fn decision_rules(&self) -> Vec<String> {
        self.events
            .events()
            .iter()
            .filter_map(|envelope| {
                let wire = serde_json::to_value(&envelope.event).unwrap();
                (wire["type"] == "policy.decision")
                    .then(|| wire["data"]["rule"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    /// Initialize a git repository in the workspace with one commit on `main`.
    pub fn git_repository(&self) {
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.root())
                .output()
                .expect("git is installed");
            assert!(output.status.success(), "git {args:?}: {output:?}");
        };
        git(&["init", "--quiet"]);
        git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        self.write("README.md", "hello\n");
        git(&["add", "README.md"]);
        git(&["commit", "--quiet", "-m", "initial"]);
    }

    pub fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root())
            .output()
            .expect("git is installed");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }
}

/// An agent-originated invocation in the fixture workspace and session.
pub fn agent_call(tool: &str, input: Value) -> ToolInvocation {
    ToolInvocation::new(
        id(SESSION),
        id(WORKSPACE),
        ToolId::new(tool).unwrap(),
        input,
        Origin::Agent,
    )
    .with_agent(id(AGENT))
}

/// The approval id of a pending outcome.
pub fn pending(outcome: &BrokerOutcome) -> EntityId {
    match outcome {
        BrokerOutcome::PendingApproval { approval_id, .. } => approval_id.clone(),
        other => panic!("expected a pending approval, got {other:?}"),
    }
}

pub fn assert_completed(outcome: &BrokerOutcome) {
    assert!(
        matches!(outcome, BrokerOutcome::Completed { .. }),
        "expected completion, got {outcome:?}"
    );
}

pub fn assert_denied(outcome: &BrokerOutcome) -> String {
    match outcome {
        BrokerOutcome::Denied { rule, .. } => rule.clone(),
        other => panic!("expected a denial, got {other:?}"),
    }
}

/// `(program, args)` for a process that runs for about `seconds` without network
/// or file effects.
pub fn long_running(seconds: u32) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "ping".into(),
            vec!["-n".into(), (seconds + 1).to_string(), "127.0.0.1".into()],
        )
    } else {
        ("sleep".into(), vec![seconds.to_string()])
    }
}

/// A global policy document allowing the long-running test program.
pub const ALLOW_WAITING: &str = r#"
schema_version = 1
scope = "global"

[[rules]]
id = "allow-waiting"
action = "shell.exec"
verdict = "allow"
[rules.when]
only_programs = ["ping", "sleep"]
"#;
