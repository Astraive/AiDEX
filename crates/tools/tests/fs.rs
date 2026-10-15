//! Filesystem tools: confinement, limits, previews, conflicts, and concurrency.

mod common;

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use aidex_policy::Action;
use aidex_tools::{ToolError, ToolLimits};
use common::{Fixture, dir_link, file_link, outside_dir};
use serde_json::{Value, json};
use sha2::Digest;

fn content(output: &aidex_tools::ToolOutput) -> &Value {
    &output.content
}

#[test]
fn read_returns_line_ranges_and_continuation() {
    let fixture = Fixture::new();
    let text: String = (1..=10).map(|line| format!("line {line}\n")).collect();
    fixture.write("a.txt", &text);
    let output = fixture
        .run(
            "fs.read",
            json!({"path": "a.txt", "start_line": 3, "max_lines": 2}),
        )
        .unwrap();
    let value = content(&output);
    assert_eq!(value["content"], "line 3\nline 4\n");
    assert_eq!(value["start_line"], 3);
    assert_eq!(value["end_line"], 4);
    assert_eq!(value["truncated"], true);
    assert_eq!(value["next_line"], 5);
    assert_eq!(value["size"], text.len() as u64);
    let whole = fixture.run("fs.read", json!({"path": "a.txt"})).unwrap();
    assert_eq!(whole.content["content"], text.as_str());
    assert_eq!(whole.content["truncated"], false);
    assert!(whole.content["next_line"].is_null());
    let tail = fixture
        .run(
            "fs.read",
            json!({"path": "a.txt", "start_line": 9, "max_lines": 2}),
        )
        .unwrap();
    assert_eq!(tail.content["truncated"], false, "nothing follows line 10");
}

#[test]
fn large_files_respect_the_read_limit() {
    let fixture = Fixture::new();
    // One long line (no newline) larger than the limit, with a multi-byte char at
    // the cut point.
    let mut big = "x".repeat(1023);
    big.push('é');
    big.push_str(&"y".repeat(10_000));
    fixture.write("big.txt", &big);
    let limits = ToolLimits {
        max_read_bytes: 1024,
        ..ToolLimits::default()
    };
    let ctx = fixture.ctx().with_limits(limits);
    let plan = fixture
        .plan_in(&ctx, "fs.read", json!({"path": "big.txt"}))
        .unwrap();
    let output = fixture.execute(&ctx, &plan).unwrap();
    let value = &output.content;
    let returned = value["content"].as_str().unwrap();
    assert!(returned.len() <= 1024);
    assert_eq!(returned, "x".repeat(1023), "cut on a UTF-8 boundary");
    assert_eq!(value["lossy_utf8"], false);
    assert_eq!(value["truncated"], true);
    assert_eq!(value["next_line"], 1);
    assert_eq!(
        value["sha256"],
        aidex_tools_sha(big.as_bytes()),
        "the hash still covers the whole file"
    );
    // Asking for more than the configured limit is capped.
    let plan = fixture
        .plan_in(
            &ctx,
            "fs.read",
            json!({"path": "big.txt", "max_bytes": 1_000_000}),
        )
        .unwrap();
    let output = fixture.execute(&ctx, &plan).unwrap();
    assert!(output.content["content"].as_str().unwrap().len() <= 1024);
}

fn aidex_tools_sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn binary_files_return_metadata_instead_of_bytes() {
    let fixture = Fixture::new();
    let mut bytes = vec![0u8, 1, 2, 3];
    bytes.extend(std::iter::repeat_n(b'z', 100_000));
    fixture.write("blob.bin", &bytes);
    let output = fixture.run("fs.read", json!({"path": "blob.bin"})).unwrap();
    assert_eq!(output.content["binary"], true);
    assert!(output.content["content"].is_null());
    assert_eq!(output.content["size"], bytes.len() as u64);
    assert_eq!(output.content["sha256"], aidex_tools_sha(&bytes));
    // Invalid UTF-8 without NUL is returned lossily and flagged.
    fixture.write("latin1.txt", [b'c', b'a', b'f', 0xe9, b'\n']);
    let output = fixture
        .run("fs.read", json!({"path": "latin1.txt"}))
        .unwrap();
    assert_eq!(output.content["lossy_utf8"], true);
    assert_eq!(output.content["binary"], false);
    // Binary files are skipped by search.
    let output = fixture.run("fs.search", json!({"pattern": "z"})).unwrap();
    assert_eq!(output.content["skipped"]["binary"], 1);
}

/// Every path-taking tool with a traversal path in each path field.
fn traversal_inputs() -> Vec<(&'static str, Value)> {
    vec![
        ("fs.read", json!({"path": "../x"})),
        ("fs.list", json!({"path": "../"})),
        ("fs.glob", json!({"pattern": "*", "path": "a/../.."})),
        ("fs.search", json!({"pattern": "x", "path": ".."})),
        ("fs.write", json!({"path": "../x", "content": ""})),
        (
            "fs.patch",
            json!({"path": "../x", "edits": [{"old": "a", "new": "b"}]}),
        ),
        ("fs.delete", json!({"path": "../x"})),
        ("git.status", json!({"paths": ["../x"]})),
        ("git.diff", json!({"paths": ["a/../../x"]})),
        ("git.log", json!({"paths": ["../x"]})),
        ("git.show", json!({"rev": "HEAD", "paths": ["../x"]})),
        ("git.commit", json!({"message": "m", "paths": ["../x"]})),
        ("shell.exec", json!({"program": "x", "cwd": ".."})),
        ("fs.read", json!({"path": "C:/Windows/win.ini"})),
        ("fs.write", json!({"path": "/tmp/x", "content": ""})),
        ("fs.read", json!({"path": "a\\..\\..\\x"})),
    ]
}

#[test]
fn traversal_is_rejected_by_every_path_taking_tool() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join(".git")).unwrap();
    for (tool, input) in traversal_inputs() {
        let error = fixture.plan(tool, input.clone()).unwrap_err();
        assert!(
            matches!(error.reason(), "invalid_input" | "escapes_root"),
            "{tool} {input}: {error}"
        );
    }
}

#[test]
fn link_escapes_are_rejected_by_every_path_taking_tool() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join(".git")).unwrap();
    let (_outside, outside) = outside_dir();
    if !dir_link(&outside, &fixture.path().join("out")) {
        return;
    }
    let cases = vec![
        ("fs.read", json!({"path": "out/secret.txt"})),
        ("fs.list", json!({"path": "out"})),
        ("fs.glob", json!({"pattern": "**", "path": "out"})),
        ("fs.search", json!({"pattern": "outside", "path": "out"})),
        ("fs.write", json!({"path": "out/new.txt", "content": "x"})),
        (
            "fs.write",
            json!({"path": "out/secret.txt", "content": "x"}),
        ),
        (
            "fs.patch",
            json!({"path": "out/secret.txt", "edits": [{"old": "outside", "new": "x"}]}),
        ),
        ("fs.delete", json!({"path": "out/secret.txt"})),
        ("git.status", json!({"paths": ["out/secret.txt"]})),
        (
            "git.commit",
            json!({"message": "m", "paths": ["out/secret.txt"]}),
        ),
        ("shell.exec", json!({"program": "x", "cwd": "out"})),
    ];
    for (tool, input) in cases {
        let error = fixture.plan(tool, input.clone()).unwrap_err();
        assert_eq!(error.reason(), "escapes_root", "{tool} {input}: {error}");
    }
    // Listing and searching from the root never follow the link.
    let listing = fixture.run("fs.list", json!({"recursive": true})).unwrap();
    let paths: Vec<&str> = listing.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["out"]);
    assert_eq!(listing.content["entries"][0]["kind"], "symlink");
    let search = fixture
        .run("fs.search", json!({"pattern": "outside"}))
        .unwrap();
    assert_eq!(search.content["matches"].as_array().unwrap().len(), 0);
    // Deleting the link removes only the link.
    fixture.run("fs.delete", json!({"path": "out"})).unwrap();
    assert!(outside.join("secret.txt").exists());
    assert!(!fixture.path().join("out").exists());
}

#[test]
fn file_symlink_leaves_are_never_replaced() {
    let fixture = Fixture::new();
    fixture.write("real.txt", "inside\n");
    if !file_link(
        &fixture.path().join("real.txt"),
        &fixture.path().join("alias.txt"),
    ) {
        return;
    }
    let error = fixture
        .plan("fs.write", json!({"path": "alias.txt", "content": "x"}))
        .unwrap_err();
    assert_eq!(error.reason(), "symlink_leaf");
    // Reading through an in-workspace link is allowed.
    let output = fixture
        .run("fs.read", json!({"path": "alias.txt"}))
        .unwrap();
    assert_eq!(output.content["content"], "inside\n");
    let (_outside, outside) = outside_dir();
    file_link(
        &outside.join("secret.txt"),
        &fixture.path().join("leak.txt"),
    );
    let error = fixture
        .plan("fs.read", json!({"path": "leak.txt"}))
        .unwrap_err();
    assert_eq!(error.reason(), "escapes_root");
}

#[test]
fn listing_glob_and_search_respect_gitignore() {
    let fixture = Fixture::new();
    fixture.write(".gitignore", "target/\n*.log\n");
    fixture.write("src/main.rs", "fn main() { println!(\"needle\"); }\n");
    fixture.write("src/lib.rs", "// needle in lib\npub fn f() {}\n");
    fixture.write("src/.gitignore", "generated.rs\n");
    fixture.write("src/generated.rs", "needle\n");
    fixture.write("target/out.rs", "needle\n");
    fixture.write("debug.log", "needle\n");
    fixture.write(".env", "NEEDLE=1\n");
    fs::create_dir(fixture.path().join(".git")).unwrap();
    fixture.write(".git/config", "needle\n");
    let listing = fixture.run("fs.list", json!({"recursive": true})).unwrap();
    let paths: Vec<&str> = listing.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            ".env",
            ".gitignore",
            "src",
            "src/.gitignore",
            "src/lib.rs",
            "src/main.rs"
        ]
    );
    let all = fixture
        .run(
            "fs.list",
            json!({"recursive": true, "include_ignored": true}),
        )
        .unwrap();
    assert!(
        all.content["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "target/out.rs")
    );
    let glob = fixture
        .run("fs.glob", json!({"pattern": "**/*.rs"}))
        .unwrap();
    let matched: Vec<&str> = glob.content["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(matched, ["src/lib.rs", "src/main.rs"]);
    let search = fixture
        .run(
            "fs.search",
            json!({"pattern": "needle", "case_insensitive": true, "context_lines": 1}),
        )
        .unwrap();
    let hits: Vec<(&str, u64)> = search.content["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| (hit["path"].as_str().unwrap(), hit["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(hits, [("src/lib.rs", 1), ("src/main.rs", 1)]);
    assert_eq!(
        search.content["matches"][0]["after"],
        json!(["pub fn f() {}"])
    );
    assert_eq!(
        search.content["skipped"]["sensitive"], 1,
        ".env is left to fs.read + policy"
    );
    let bounded = fixture
        .run("fs.search", json!({"pattern": "needle", "max_results": 1}))
        .unwrap();
    assert_eq!(bounded.content["truncated"], true);
    let regex = fixture
        .run(
            "fs.search",
            json!({"pattern": "^pub fn \\w+", "regex": true, "glob": "src/*.rs"}),
        )
        .unwrap();
    assert_eq!(regex.content["matches"][0]["line"], 2);
}

#[test]
fn write_previews_a_diff_and_applies_exactly_it() {
    let fixture = Fixture::new();
    fixture.write("src/a.txt", "one\ntwo\n");
    let (ctx, plan) = fixture
        .plan(
            "fs.write",
            json!({"path": "src/a.txt", "content": "one\nTWO\nthree\n"}),
        )
        .unwrap();
    assert_eq!(
        fixture.read("src/a.txt"),
        "one\ntwo\n",
        "planning has no side effects"
    );
    let preview = plan.preview().unwrap();
    assert_eq!((preview.additions(), preview.deletions()), (2, 1));
    let unified = preview.files[0].unified.as_deref().unwrap();
    assert!(unified.contains("-two\n+TWO\n+three\n"), "{unified}");
    assert_eq!(
        plan.facts().paths[0].sha256.as_deref().map(str::len),
        Some(64)
    );
    let output = fixture.execute(&ctx, &plan).unwrap();
    assert_eq!(fixture.read("src/a.txt"), "one\nTWO\nthree\n");
    assert_eq!(output.diff.as_ref(), Some(preview));
    assert_eq!(output.content["created"], false);
    // Creating a file creates its parents inside the workspace.
    fixture
        .run(
            "fs.write",
            json!({"path": "new/dir/b.txt", "content": "b\n"}),
        )
        .unwrap();
    assert_eq!(fixture.read("new/dir/b.txt"), "b\n");
    // create_only and expected_sha256 guards.
    let error = fixture
        .plan(
            "fs.write",
            json!({"path": "src/a.txt", "content": "x", "create_only": true}),
        )
        .unwrap_err();
    assert_eq!(error.reason(), "conflict");
    let error = fixture
        .plan(
            "fs.write",
            json!({"path": "src/a.txt", "content": "x", "expected_sha256": "00"}),
        )
        .unwrap_err();
    assert_eq!(error.reason(), "conflict");
    // No temporary files are left behind.
    let names: Vec<String> = fs::read_dir(fixture.path().join("src"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.txt"]);
}

#[test]
fn writes_respect_size_limits() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx().with_limits(ToolLimits {
        max_write_bytes: 8,
        ..ToolLimits::default()
    });
    let error = fixture
        .plan_in(
            &ctx,
            "fs.write",
            json!({"path": "a.txt", "content": "123456789"}),
        )
        .unwrap_err();
    assert_eq!(error.reason(), "too_large");
}

#[test]
fn stale_targets_conflict_instead_of_being_clobbered() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "base\n");
    let (ctx, plan) = fixture
        .plan("fs.write", json!({"path": "a.txt", "content": "mine\n"}))
        .unwrap();
    fixture.write("a.txt", "theirs\n");
    let error = fixture.execute(&ctx, &plan).unwrap_err();
    assert!(matches!(error, ToolError::Conflict { .. }), "{error}");
    assert_eq!(fixture.read("a.txt"), "theirs\n");
    // A planned creation conflicts when the file appears meanwhile.
    let (ctx, plan) = fixture
        .plan("fs.write", json!({"path": "b.txt", "content": "mine"}))
        .unwrap();
    fixture.write("b.txt", "theirs");
    assert_eq!(
        fixture.execute(&ctx, &plan).unwrap_err().reason(),
        "conflict"
    );
    // Patches and deletes re-verify too.
    let (ctx, patch) = fixture
        .plan(
            "fs.patch",
            json!({"path": "a.txt", "edits": [{"old": "theirs", "new": "x"}]}),
        )
        .unwrap();
    fixture.write("a.txt", "theirs\nmore\n");
    assert_eq!(
        fixture.execute(&ctx, &patch).unwrap_err().reason(),
        "conflict"
    );
    let (ctx, delete) = fixture.plan("fs.delete", json!({"path": "a.txt"})).unwrap();
    fixture.write("a.txt", "changed again\n");
    assert_eq!(
        fixture.execute(&ctx, &delete).unwrap_err().reason(),
        "conflict"
    );
    assert!(fixture.path().join("a.txt").exists());
}

#[test]
fn simultaneous_writes_to_one_file_serialize_and_detect_conflicts() {
    let fixture = Arc::new(Fixture::new());
    fixture.write("shared.txt", "base\n");
    const WRITERS: usize = 8;
    // Every plan is made against the same base content before any executes.
    let plans: Vec<_> = (0..WRITERS)
        .map(|index| {
            fixture
                .plan(
                    "fs.write",
                    json!({"path": "shared.txt", "content": format!("writer {index}\n")}),
                )
                .unwrap()
        })
        .collect();
    let barrier = Arc::new(Barrier::new(WRITERS));
    let handles: Vec<_> = plans
        .into_iter()
        .map(|(ctx, plan)| {
            let (fixture, barrier) = (fixture.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                fixture.execute(&ctx, &plan).map(|_| ())
            })
        })
        .collect();
    let results: Vec<Result<(), ToolError>> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let succeeded = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(succeeded, 1, "{results:?}");
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.reason() == "conflict")
    );
    assert!(fixture.read("shared.txt").starts_with("writer "));
    // Writers to different files run concurrently and all succeed.
    let handles: Vec<_> = (0..WRITERS)
        .map(|index| {
            let fixture = fixture.clone();
            thread::spawn(move || {
                fixture
                    .run(
                        "fs.write",
                        json!({"path": format!("f{index}.txt"), "content": "x"}),
                    )
                    .map(|_| ())
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
}

#[test]
fn patch_edits_and_unified_diffs_apply_or_fail_cleanly() {
    let fixture = Fixture::new();
    fixture.write("code.rs", "fn a() {}\nfn b() {}\nfn a2() {}\n");
    let (ctx, plan) = fixture
        .plan(
            "fs.patch",
            json!({"path": "code.rs", "edits": [
                {"old": "fn b()", "new": "fn beta()"},
                {"old": "{}", "new": "{ }", "expected_occurrences": 3}
            ]}),
        )
        .unwrap();
    let unified = plan.preview().unwrap().files[0].unified.clone().unwrap();
    assert!(unified.contains("+fn beta() { }"), "{unified}");
    fixture.execute(&ctx, &plan).unwrap();
    assert_eq!(
        fixture.read("code.rs"),
        "fn a() { }\nfn beta() { }\nfn a2() { }\n"
    );
    let error = fixture
        .plan(
            "fs.patch",
            json!({"path": "code.rs", "edits": [{"old": "fn a", "new": "x"}]}),
        )
        .unwrap_err();
    match error {
        ToolError::PatchFailed { index, reason } => {
            assert_eq!(index, 0);
            assert!(reason.contains("2 times"), "{reason}");
        }
        other => panic!("unexpected {other:?}"),
    }
    let diff = "--- a/code.rs\n+++ b/code.rs\n@@ -2,1 +2,2 @@\n fn beta() { }\n+fn c() {}\n";
    fixture
        .run("fs.patch", json!({"path": "code.rs", "unified_diff": diff}))
        .unwrap();
    assert_eq!(
        fixture.read("code.rs"),
        "fn a() { }\nfn beta() { }\nfn c() {}\nfn a2() { }\n"
    );
    let bad = "@@ -1,1 +1,1 @@\n-not there\n+x\n";
    let error = fixture
        .plan("fs.patch", json!({"path": "code.rs", "unified_diff": bad}))
        .unwrap_err();
    assert_eq!(error.reason(), "patch_failed");
    fixture.write("blob.bin", [0u8, 1, 2]);
    let error = fixture
        .plan(
            "fs.patch",
            json!({"path": "blob.bin", "edits": [{"old": "a", "new": "b"}]}),
        )
        .unwrap_err();
    assert_eq!(error.reason(), "invalid_input");
}

#[test]
fn delete_is_explicit_about_directories() {
    let fixture = Fixture::new();
    fixture.write("dir/a.txt", "a\n");
    fixture.write("dir/sub/b.txt", "b\n");
    fixture.write("file.txt", "gone\n");
    let (ctx, plan) = fixture
        .plan("fs.delete", json!({"path": "file.txt"}))
        .unwrap();
    assert!(!plan.facts().destructive);
    assert_eq!(plan.preview().unwrap().deletions(), 1);
    fixture.execute(&ctx, &plan).unwrap();
    assert!(!fixture.path().join("file.txt").exists());
    let error = fixture
        .plan("fs.delete", json!({"path": "dir"}))
        .unwrap_err();
    assert_eq!(error.reason(), "invalid_input");
    let (ctx, plan) = fixture
        .plan("fs.delete", json!({"path": "dir", "recursive": true}))
        .unwrap();
    assert!(plan.facts().destructive);
    assert!(plan.intent().attributes.destructive);
    assert_eq!(plan.preview().unwrap().files.len(), 2);
    let output = fixture.execute(&ctx, &plan).unwrap();
    assert_eq!(output.content["files_deleted"], 2);
    assert!(!fixture.path().join("dir").exists());
    // A recursive delete conflicts if the tree changed after review.
    fixture.write("tree/a.txt", "a");
    let (ctx, plan) = fixture
        .plan("fs.delete", json!({"path": "tree", "recursive": true}))
        .unwrap();
    fixture.write("tree/new.txt", "surprise");
    assert_eq!(
        fixture.execute(&ctx, &plan).unwrap_err().reason(),
        "conflict"
    );
    assert!(fixture.path().join("tree/new.txt").exists());
}
#[test]
fn delete_requires_expected_content_when_digest_is_supplied() {
    let fixture = Fixture::new();
    fixture.write("file.txt", "keep\n");
    let bad=fixture.plan("fs.delete",json!({"path":"file.txt","expected_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}));
    assert!(bad.is_err());
    assert!(fixture.path().join("file.txt").exists());
    let expected = format!("{:x}", sha2::Sha256::digest(b"keep\n"));
    let (ctx, plan) = fixture
        .plan(
            "fs.delete",
            json!({"path":"file.txt","expected_sha256":expected}),
        )
        .unwrap();
    fixture.execute(&ctx, &plan).unwrap();
    assert!(!fixture.path().join("file.txt").exists());
}

#[test]
fn git_metadata_is_never_modified_and_aidex_writes_imply_policy_change() {
    let fixture = Fixture::new();
    fixture.write(".git/hooks/README", "x");
    for (tool, input) in [
        (
            "fs.write",
            json!({"path": ".git/hooks/pre-commit", "content": "#!/bin/sh"}),
        ),
        (
            "fs.write",
            json!({"path": "sub/.GIT/config", "content": "x"}),
        ),
        (
            "fs.patch",
            json!({"path": ".git/hooks/README", "edits": [{"old": "x", "new": "y"}]}),
        ),
        ("fs.delete", json!({"path": ".git", "recursive": true})),
    ] {
        let error = fixture.plan(tool, input.clone()).unwrap_err();
        assert_eq!(error.reason(), "forbidden_path", "{tool} {input}");
    }
    let (_, plan) = fixture
        .plan(
            "fs.write",
            json!({"path": ".aidex/rules.toml", "content": "x"}),
        )
        .unwrap();
    assert_eq!(plan.facts().implied_actions, [Action::PolicyChange]);
}

#[test]
fn a_path_scope_is_the_one_a_plan_of_the_path_reports() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "x");
    fixture.write(".env", "TOKEN=x");
    fixture.write("dir/inner.txt", "x");
    // An existing file, a credential-bearing one, and a file being created.
    for (path, tool, input) in [
        ("notes.txt", "fs.read", json!({"path": "notes.txt"})),
        (".env", "fs.read", json!({"path": ".env"})),
        (
            "new.txt",
            "fs.write",
            json!({"path": "new.txt", "content": "x"}),
        ),
    ] {
        let (_, plan) = fixture.plan(tool, input).unwrap();
        let planned = plan.facts().intent_attributes().paths.remove(0);
        assert_eq!(
            aidex_tools::path_scope(&fixture.root, path).unwrap(),
            planned,
            "{path}"
        );
    }
    let env = aidex_tools::path_scope(&fixture.root, ".env").unwrap();
    assert_eq!(env.class, aidex_policy::PathClass::Sensitive);
    // The root, as a listing names it.
    let (_, plan) = fixture.plan("fs.list", json!({})).unwrap();
    let root = plan.facts().intent_attributes().paths.remove(0);
    for raw in ["", "."] {
        assert_eq!(aidex_tools::path_scope(&fixture.root, raw).unwrap(), root);
    }
    // Malformed paths and paths outside the workspace have no scope.
    for raw in ["../outside.txt", "/etc/passwd", "a/../../b"] {
        assert!(
            aidex_tools::path_scope(&fixture.root, raw).is_err(),
            "{raw}"
        );
    }
}
