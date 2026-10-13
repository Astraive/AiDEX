//! Indexing acceptance tests: large repositories, link boundaries, ignored and
//! secret files, incremental refresh, cancellation and resume, corruption
//! recovery, background jobs, and offline operation.

mod common;

use std::{
    fs,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime},
};

use aidex_context::{
    BuildOutcome, ContextService, DropReason, Index, IndexOptions, IndexState, JobState,
    PackInclude, PackRequest, Query, RecoveryKind, SearchMode, SymbolKind,
};
use aidex_core::{CancelToken, WorkspacePath};
use aidex_events::{Cursor, EventError, EventKind, EventScope, EventSink, MemoryEventSink};
use common::*;

fn wp(path: &str) -> WorkspacePath {
    WorkspacePath::new(path).unwrap()
}

fn paths_of(hits: &[aidex_context::SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.path.as_str()).collect()
}

#[test]
fn large_repositories_index_in_reasonable_time_and_packs_stay_in_budget() {
    let fixture = Fixture::new(&[]);
    for module in 0..50 {
        for file in 0..100 {
            fixture.write(
                &format!("src/m{module:02}/f{file:03}.rs"),
                &format!(
                    "//! Module {module} file {file}.\n\npub fn handler_{module}_{file}(input: u32) -> u32 {{\n    input + {file}\n}}\n\npub struct Widget{module}x{file};\n"
                ),
            );
        }
    }
    let started = Instant::now();
    let report = fixture.build();
    let elapsed = started.elapsed();
    assert_eq!(report.outcome, BuildOutcome::Completed);
    assert_eq!((report.files_seen, report.added), (5_000, 5_000));
    assert!(
        elapsed < Duration::from_secs(180),
        "indexing 5k files took {elapsed:?}"
    );
    let summary = fixture.service.status(&fixture.workspace).unwrap().index;
    assert_eq!(summary.state, IndexState::Complete);
    assert_eq!(summary.files, 5_000);
    assert!(summary.symbols >= 10_000);

    // A no-op refresh only compares metadata.
    let started = Instant::now();
    let again = fixture.build();
    assert_eq!((again.unchanged, again.added, again.updated), (5_000, 0, 0));
    assert!(started.elapsed() < elapsed + Duration::from_secs(30));

    let hits = fixture
        .service
        .search(&fixture.workspace, &Query::symbol("handler_42_7"))
        .unwrap();
    assert_eq!(hits[0].path, "src/m42/f007.rs");
    assert_eq!(hits[0].symbol.as_ref().unwrap().kind, SymbolKind::Function);

    let target = fixture.target();
    for budget in [50, 400, 4_000] {
        let pack = fixture
            .service
            .assemble(
                &target,
                &PackRequest::new(
                    fixture.workspace.clone(),
                    "fix handler_42_7 overflow",
                    budget,
                ),
            )
            .unwrap();
        assert!(
            pack.total_tokens <= budget,
            "{budget}: {}",
            pack.total_tokens
        );
        if budget >= 400 {
            assert!(
                pack.items
                    .iter()
                    .any(|item| item.text.contains("handler_42_7"))
            );
        }
        // Never a whole-repository dump.
        let files: std::collections::HashSet<String> = pack
            .items
            .iter()
            .filter_map(|item| match &item.source {
                aidex_context::ContextSource::File { path, .. } => Some(path.as_str().to_owned()),
                _ => None,
            })
            .collect();
        assert!(files.len() <= 40, "{} files packed", files.len());
    }
}

#[test]
fn links_leaving_the_workspace_are_never_indexed_or_searched() {
    let fixture = Fixture::new(&[("src/lib.rs", "pub fn inside() {}\n")]);
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "leak.txt", "outsideonlyword lives here\n");
    write(
        outside.path(),
        "nested/more.rs",
        "fn outsideonlyword() {}\n",
    );
    let linked_dir = dir_link(outside.path(), &fixture.root().join("linked"));
    let linked_file = file_link(
        &outside.path().join("leak.txt"),
        &fixture.root().join("leak.txt"),
    );
    let report = fixture.build();
    assert_eq!(report.added, 1);
    if linked_dir || linked_file {
        assert!(report.skipped.link >= 1, "{:?}", report.skipped);
    }
    for mode in [
        SearchMode::Lexical,
        SearchMode::Filename,
        SearchMode::Symbol,
    ] {
        let text = match mode {
            SearchMode::Filename => "leak",
            _ => "outsideonlyword",
        };
        let hits = fixture
            .service
            .search(&fixture.workspace, &Query::new(text, mode))
            .unwrap();
        assert!(hits.is_empty(), "{mode:?}: {:?}", paths_of(&hits));
    }
    let pack = fixture
        .service
        .assemble(
            &fixture.target(),
            &PackRequest::new(fixture.workspace.clone(), "outsideonlyword", 10_000)
                .with_paths(vec![wp("leak.txt"), wp("linked/leak.txt")]),
        )
        .unwrap();
    assert!(!pack.render().contains("outsideonlyword lives"));
    assert!(
        pack.dropped
            .iter()
            .all(|dropped| dropped.reason == DropReason::NotIndexed)
    );
}

#[test]
fn ignored_and_secret_files_are_excluded_from_search_and_packs() {
    let token = fake_token();
    let fixture = Fixture::new(&[
        (".gitignore", "build/\n*.log\n"),
        ("build/out.js", "const ignoredword = 1;\n"),
        ("debug.log", "ignoredword in a log\n"),
        ("sub/.ignore", "generated.rs\n"),
        ("sub/generated.rs", "fn ignoredword() {}\n"),
        (".env", &format!("GITHUB_TOKEN={token}\nsecretword=1\n")),
        (".env.local", "secretword=2\n"),
        ("deploy/id_ed25519", "secretword\n"),
        ("config/secrets.yaml", "secretword: yes\n"),
        ("certs/server.pem", "secretword\n"),
        (".aws/credentials", "secretword\n"),
        (
            "notes.txt",
            &format!("secretword notes\n{}", fake_private_key()),
        ),
        (
            "src/client.rs",
            &format!("pub fn connect() {{\n    let token = \"{token}\";\n    call(token)\n}}\n"),
        ),
    ]);
    let report = fixture.build();
    assert_eq!(report.added, 3, "{report:?}"); // .gitignore, sub/.ignore, src/client.rs
    assert_eq!(report.skipped.secret_content, 1);
    assert!(report.skipped.secret_name >= 5, "{:?}", report.skipped);
    assert_eq!(report.redacted, 1);
    for (text, mode) in [
        ("ignoredword", SearchMode::Lexical),
        ("secretword", SearchMode::Lexical),
        ("out.js", SearchMode::Filename),
        (".env", SearchMode::Filename),
        ("id_ed25519", SearchMode::Filename),
        ("secrets", SearchMode::Filename),
        ("ignoredword", SearchMode::Symbol),
    ] {
        let hits = fixture
            .service
            .search(&fixture.workspace, &Query::new(text, mode))
            .unwrap();
        assert!(hits.is_empty(), "{text} {mode:?}: {:?}", paths_of(&hits));
    }
    // Incidental credentials are redacted, not indexed.
    let hits = fixture
        .service
        .search(&fixture.workspace, &Query::lexical("token call"))
        .unwrap();
    assert_eq!(paths_of(&hits), ["src/client.rs"]);
    assert!(!hits[0].snippet.contains(&token));
    let token_hits = fixture
        .service
        .search(&fixture.workspace, &Query::lexical(token.clone()))
        .unwrap();
    assert!(token_hits.is_empty());

    let pack = fixture
        .service
        .assemble(
            &fixture.target(),
            &PackRequest::new(
                fixture.workspace.clone(),
                "secretword ignoredword token connect",
                50_000,
            )
            .with_include(PackInclude {
                explicit_paths: vec![
                    wp(".env"),
                    wp("notes.txt"),
                    wp("build/out.js"),
                    wp("src/client.rs"),
                ],
                ..PackInclude::default()
            }),
        )
        .unwrap();
    let rendered = pack.render();
    assert!(!rendered.contains(&token));
    assert!(!rendered.contains("secretword"));
    assert!(!rendered.contains("ignoredword"));
    assert!(!rendered.contains("PRIVATE KEY"));
    assert!(rendered.contains("connect"));
    let reasons: Vec<(String, DropReason)> = pack
        .dropped
        .iter()
        .filter_map(|dropped| match &dropped.source {
            aidex_context::ContextSource::File { path, .. } => {
                Some((path.as_str().to_owned(), dropped.reason))
            }
            _ => None,
        })
        .collect();
    assert!(reasons.contains(&(".env".into(), DropReason::SecretFile)));
    assert!(reasons.contains(&("notes.txt".into(), DropReason::NotIndexed)));
    assert!(reasons.contains(&("build/out.js".into(), DropReason::NotIndexed)));
}

#[test]
fn refresh_reindexes_only_changed_files_and_drops_deleted_ones() {
    let mut files: Vec<(String, String)> = (0..20)
        .map(|n| {
            (
                format!("src/f{n:02}.rs"),
                format!("pub fn original_{n}() {{}}\n"),
            )
        })
        .collect();
    files.push(("README.md".into(), "# Project\n".into()));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    assert_eq!(fixture.build().added, 21);

    // Modify (different size), delete, add, and touch (same bytes, new mtime).
    fixture.write(
        "src/f00.rs",
        "pub fn rewritten_zero() { let changed = 1; }\n",
    );
    fs::remove_file(fixture.root().join("src/f01.rs")).unwrap();
    fixture.write("src/new.rs", "pub fn brand_new() {}\n");
    let touched = fs::File::options()
        .write(true)
        .open(fixture.root().join("src/f02.rs"))
        .unwrap();
    touched
        .set_modified(SystemTime::now() - Duration::from_secs(3_600))
        .unwrap();
    drop(touched);
    // A newly ignored file is removed too.
    fixture.write(".gitignore", "README.md\n");

    // Before re-indexing, packs refuse the stale file instead of packing old text.
    let stale = fixture
        .service
        .assemble(
            &fixture.target(),
            &PackRequest::new(fixture.workspace.clone(), "original_0", 10_000),
        )
        .unwrap();
    assert!(!stale.render().contains("original_0"));
    assert!(
        stale
            .dropped
            .iter()
            .any(|dropped| dropped.reason == DropReason::Stale)
    );

    let report = fixture.build();
    assert_eq!(
        (
            report.added,
            report.updated,
            report.touched,
            report.removed,
            report.unchanged
        ),
        (2, 1, 1, 2, 17),
        "{report:?}"
    );
    let search = |text: &str| {
        fixture
            .service
            .search(&fixture.workspace, &Query::symbol(text))
            .unwrap()
    };
    assert!(search("original_0").is_empty());
    assert_eq!(paths_of(&search("rewritten_zero")), ["src/f00.rs"]);
    assert!(
        search("original_1")
            .iter()
            .all(|hit| hit.path != "src/f01.rs")
    );
    assert_eq!(paths_of(&search("brand_new")), ["src/new.rs"]);
    assert_eq!(paths_of(&search("original_2")), ["src/f02.rs"]);
    assert!(
        fixture
            .service
            .search(&fixture.workspace, &Query::filename("README"))
            .unwrap()
            .is_empty()
    );
    let summary = fixture.service.status(&fixture.workspace).unwrap().index;
    assert_eq!(summary.files, 21);
}

#[test]
fn cancelled_builds_keep_committed_batches_and_resume() {
    let files: Vec<(String, String)> = (0..1_000)
        .map(|n| {
            (
                format!("d{}/f{n:04}.txt", n / 100),
                format!("entry number {n}\n"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let options = IndexOptions {
        batch_size: 100,
        ..IndexOptions::default()
    };
    let fixture = Fixture::with_options(&borrowed, options);
    let stop = CancelToken::new();
    let report = fixture
        .service
        .build_now(&fixture.target(), &stop.clone(), &mut |progress| {
            if progress.processed >= 300 {
                stop.cancel();
            }
        })
        .unwrap();
    assert_eq!(report.outcome, BuildOutcome::Cancelled);
    assert_eq!(report.added, 300);
    let summary = fixture.service.status(&fixture.workspace).unwrap().index;
    assert_eq!((summary.state, summary.files), (IndexState::Partial, 300));
    assert!(summary.last_complete_at.is_none());

    let resumed = fixture.build();
    assert_eq!(resumed.outcome, BuildOutcome::Completed);
    assert_eq!((resumed.unchanged, resumed.added), (300, 700));

    // A cancelled run never prunes: unvisited files are kept until a complete walk.
    for n in 0..10 {
        fs::remove_file(fixture.root().join(format!("d0/f{n:04}.txt"))).unwrap();
    }
    let cancelled = CancelToken::new();
    cancelled.cancel();
    let report = fixture
        .service
        .build_now(&fixture.target(), &cancelled, &mut |_| {})
        .unwrap();
    assert_eq!(report.outcome, BuildOutcome::Cancelled);
    assert_eq!(
        fixture
            .service
            .status(&fixture.workspace)
            .unwrap()
            .index
            .files,
        1_000
    );
    assert_eq!(fixture.build().removed, 10);
}

#[test]
fn corrupt_indexes_are_moved_aside_and_rebuilt() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", "pub fn survives_corruption() {}\n"),
        ("docs/guide.md", "how to rebuild\n"),
    ]);
    let paths = fixture.drive.paths();
    let dir = fixture.index_dir();
    {
        let index = Index::open_on_drive(&paths, &fixture.workspace).unwrap();
        let report = index
            .build(
                &fixture.target(),
                &IndexOptions::default(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(report.added, 2);
    }
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(dir.join(format!("index.sqlite3{suffix}")));
    }
    let garbage = b"this is not a database, just garbage bytes \x00\x01\x02".repeat(64);
    fs::write(dir.join("index.sqlite3"), garbage).unwrap();

    let index = Index::open_on_drive(&paths, &fixture.workspace).unwrap();
    let recovery = index.recovery().cloned().expect("recovered");
    assert_eq!(recovery.kind, RecoveryKind::Corrupt);
    let moved = recovery.moved_to.clone().unwrap();
    assert!(dir.join(&moved).is_file(), "{moved}");
    let summary = index.summary().unwrap();
    assert_eq!((summary.state, summary.files), (IndexState::Empty, 0));
    assert_eq!(
        summary.recovery.as_ref().unwrap().kind,
        RecoveryKind::Corrupt
    );
    let report = index
        .build(
            &fixture.target(),
            &IndexOptions::default(),
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(report.added, 2);
    let hits = index.search(&Query::symbol("survives_corruption")).unwrap();
    assert_eq!(paths_of(&hits), ["src/lib.rs"]);
}

#[test]
fn background_jobs_report_task_events_and_can_be_cancelled() {
    let files: Vec<(String, String)> = (0..400)
        .map(|n| (format!("f{n:03}.md"), format!("note {n}\n")))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    let started = fixture.service.start_index(fixture.target()).unwrap();
    assert!(!started.already_running);
    let status = fixture
        .service
        .wait_index(&fixture.workspace, Duration::from_secs(120))
        .unwrap();
    assert_eq!(status.state, JobState::Completed, "{status:?}");
    assert_eq!(status.report.unwrap().added, 400);
    assert_eq!(status.job_id, started.job_id);
    let names = fixture.events.names();
    assert_eq!(names, ["task.created", "task.started", "task.completed"]);

    // A cancelled job ends as cancelled with a task.cancelled event.
    fixture.service.cancel_index(&fixture.workspace);
    assert!(!fixture.service.cancel_index(&fixture.workspace));
    for n in 0..400 {
        fixture.write(
            &format!("f{n:03}.md"),
            &format!("changed note {n} with more text\n"),
        );
    }
    fixture.service.start_index(fixture.target()).unwrap();
    fixture.service.cancel_index(&fixture.workspace);
    let status = fixture
        .service
        .wait_index(&fixture.workspace, Duration::from_secs(120))
        .unwrap();
    // The cancel can land after the build finished; either way the recorded
    // terminal event matches the state `wait_index` reports.
    let expected = match status.state {
        JobState::Cancelled => "task.cancelled",
        JobState::Completed => "task.completed",
        _ => panic!("{status:?}"),
    };
    assert_eq!(fixture.events.names().last(), Some(&expected));
    assert_eq!(status.event_error, None);
}

/// Records events, holding the first terminal `task.*` event of a job until the
/// test releases it (or drops the release sender).
#[derive(Default)]
struct GatedSink {
    inner: MemoryEventSink,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl EventSink for GatedSink {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        let terminal = matches!(
            event,
            EventKind::TaskCompleted { .. }
                | EventKind::TaskCancelled { .. }
                | EventKind::TaskFailed { .. }
        );
        let gate = if terminal {
            self.gate.lock().unwrap().take()
        } else {
            None
        };
        if let Some((reached, release)) = gate {
            let _ = reached.send(());
            let _ = release.recv();
        }
        self.inner.emit(scope, event)
    }
}

#[test]
fn index_jobs_finish_only_after_their_terminal_event_is_recorded() {
    let fixture = Fixture::new(&[("notes.md", "a note\n")]);
    let sink = Arc::new(GatedSink::default());
    let service = ContextService::open(&fixture.drive.paths(), sink.clone()).unwrap();
    // Created after `service`, so a failing assertion drops the release sender
    // (unblocking the worker) before the service joins its job.
    let (reached_tx, reached) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    *sink.gate.lock().unwrap() = Some((reached_tx, release_rx));

    let started = service.start_index(fixture.target()).unwrap();
    reached
        .recv_timeout(Duration::from_secs(120))
        .expect("the job reaches its terminal event");
    // The build is over but `task.completed` is not recorded yet: every observer
    // still sees the job running, and no second job can start and interleave.
    let job = service.status(&fixture.workspace).unwrap().job.unwrap();
    assert_eq!(job.state, JobState::Running, "{job:?}");
    let waited = service
        .wait_index(&fixture.workspace, Duration::ZERO)
        .unwrap();
    assert_eq!(waited.state, JobState::Running, "{waited:?}");
    let again = service.start_index(fixture.target()).unwrap();
    assert!(again.already_running);
    assert_eq!(again.job_id, started.job_id);
    assert_eq!(sink.inner.names(), ["task.created", "task.started"]);

    release.send(()).unwrap();
    let status = service
        .wait_index(&fixture.workspace, Duration::from_secs(120))
        .unwrap();
    assert_eq!(status.state, JobState::Completed, "{status:?}");
    assert_eq!(status.event_error, None);
    assert_eq!(
        sink.inner.names(),
        ["task.created", "task.started", "task.completed"]
    );
}

#[test]
fn search_modes_rank_and_filter() {
    let fixture = Fixture::new(&[
        (
            "src/config.rs",
            "pub fn parse_config(text: &str) -> Config {\n    todo!()\n}\n\npub struct Config;\n",
        ),
        (
            "src/config_loader.rs",
            "use crate::config::parse_config;\n\nfn load() { parse_config(\"\"); }\n",
        ),
        (
            "tests/config.rs",
            "#[test]\nfn parses() { parse_config(\"x\"); }\n",
        ),
        (
            "web/app.ts",
            "export function parseConfig(raw: string) {\n  return JSON.parse(raw);\n}\n",
        ),
    ]);
    fixture.build();
    let service = &fixture.service;
    let ws = &fixture.workspace;
    let filename = service.search(ws, &Query::filename("config")).unwrap();
    assert_eq!(filename[0].path, "src/config.rs");
    assert_eq!(filename.len(), 3);
    let symbol = service.search(ws, &Query::symbol("parse_config")).unwrap();
    assert_eq!(paths_of(&symbol), ["src/config.rs"]);
    assert_eq!(symbol[0].line_range.start, 1);
    let camel = service.search(ws, &Query::symbol("parseconfig")).unwrap();
    assert_eq!(paths_of(&camel), ["web/app.ts"]);
    let lexical = service.search(ws, &Query::lexical("parse_config")).unwrap();
    assert_eq!(lexical.len(), 3);
    let scoped = service
        .search(ws, &Query::lexical("parse_config").with_prefix("tests/"))
        .unwrap();
    assert_eq!(paths_of(&scoped), ["tests/config.rs"]);
    assert!(service.search(ws, &Query::lexical("")).is_err());
    assert!(
        service
            .search(ws, &Query::lexical("x").with_limit(0))
            .is_err()
    );
    assert!(
        service
            .search(ws, &Query::lexical("x").with_prefix("../etc"))
            .is_err()
    );
    // FTS syntax in user text is quoted, never interpreted.
    assert!(
        service
            .search(ws, &Query::lexical("parse OR \" NEAR("))
            .is_ok()
    );
}

/// Offline by design: no HTTP or network client crate is a dependency of this
/// crate or of the AiDEX crates it builds on.
#[test]
fn the_context_crate_has_no_network_dependencies() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest_dir.parent().unwrap();
    let forbidden = [
        "ureq",
        "reqwest",
        "hyper",
        "curl",
        "isahc",
        "surf",
        "attohttpc",
        "tokio",
        "async-std",
        "tungstenite",
    ];
    for name in ["context", "core", "drive", "events", "policy", "workspace"] {
        let manifest = fs::read_to_string(crates.join(name).join("Cargo.toml")).unwrap();
        for line in manifest.lines() {
            let key = line.split('=').next().unwrap_or("").trim();
            assert!(
                !forbidden.contains(&key),
                "{name} depends on the network crate `{key}`"
            );
        }
    }
}
