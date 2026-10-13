# aidex-context

Local repository context for native agent tasks (issue #20; spec §34 Context Engine, §35 Memory, §54 Project Indexing, §55 Project Rules, §76 Context Compression, §78 Local Search, §91 Prompt Injection Defense, §92 Secret Exfiltration Defense). It indexes a workspace on the drive, searches it offline, loads project rules and instruction files, stores inspectable memory, and assembles **budgeted context packs** for the harness. Nothing here uses the network: there is no HTTP client or async runtime in the dependency tree (tested), and repository content leaves the machine only if the harness puts a pack into an explicit model request.

Depends on `aidex-core`, `aidex-drive`, `aidex-events` (credential redaction), `aidex-policy` (project rules), and `aidex-workspace` (confinement), plus `ignore`, `regex`, `rusqlite` (bundled SQLite with FTS5), `serde`, `sha2`, and `toml`. `aidexd` owns one `ContextService` (`Services::context()`).

## API

```rust
// Service (what the daemon and the harness use)
let service = ContextService::open(&drive_paths, events /* Arc<dyn EventSink> */)?; // .with_options/.with_memory_store/.with_estimator
let target = IndexTarget::from_handle(&workspace_handle);            // id + ConfinedRoot + RepositoryInfo
service.start_index(target.clone())?;                                // background job, one per workspace -> IndexJobStarted
service.cancel_index(&workspace_id);                                 // -> bool
service.status(&workspace_id)?;                                      // IndexStatus { index: IndexSummary, job: Option<IndexJobStatus> }
service.build_now(&target, &CancelToken::new(), &mut |p| ..)?;      // synchronous build -> IndexReport
service.search(&workspace_id, &Query::lexical("parse config"))?;     // Vec<SearchHit>
service.rules(target.root())?;                                       // ProjectRules
let pack = service.assemble(&target, &PackRequest::new(workspace_id, "fix the login bug", 8_000))?;
service.memory().add(&MemoryScope::User, NewMemory::user("prefer tabs"))?;

// Building blocks
Index::open_on_drive(&paths, &workspace_id)?  // .build(&target, &IndexOptions, &CancelToken, &mut progress) / .search / .summary / .recovery
ProjectRules::load(&confined_root)?           // .to_policy_layer() / .apply_to(&mut PolicySet, &ws) / .instructions() / .view()
FileMemoryStore::open(&paths)?                // impl MemoryStore (list/add/edit/remove/clear)
assemble(&PackSources { index, root, rules, memory, estimator }, &request)?
```

| Type | Purpose |
| --- | --- |
| `IndexOptions` | `max_file_bytes` (1 MiB), `max_files` (200 000), `batch_size` (256), `global_excludes` (off). |
| `IndexReport` | `outcome: completed\|cancelled`, `run`, `files_seen`, `added`, `updated`, `touched` (new mtime, same bytes), `unchanged`, `removed`, `redacted`, `skipped: SkipCounts {secret_name, secret_content, link, too_large, binary, unsupported_name, unreadable}`, `truncated`, `elapsed_ms`. |
| `IndexProgress` | `phase: scanning\|indexing\|pruning\|finished`, `total`, `processed`, `changed`, `removed` (after every batch). |
| `IndexSummary` | `state: empty\|building\|partial\|complete`, counts, `last_run_at`, `last_complete_at`, `truncated`, `repository: RepositoryMetadata {vcs, branch, head, detached, remotes}`, `recovery: IndexRecovery {kind: corrupt\|schema_reset, reason, moved_to, at}`. |
| `Query` | `{text, mode: lexical\|filename\|symbol, path_prefix, limit (1-200)}`. |
| `SearchHit` | `{path, line_range {start, end}, snippet, score, language, symbol: {name, kind}?}`. |
| `PackRequest` | `{task_text, workspace_id, session_id?, budget_tokens (1-1 000 000), include: {rules, memory, search_hits, explicit_paths}, summaries}`. |
| `ContextPack` | `{id, nonce, workspace_id, budget_tokens, total_tokens, items: [ContextItem {source, text, tokens, score, trust}], dropped: [{source, reason, tokens?}], dropped_unlisted, terms, index_state}`; `preamble()`, `render()`. |
| `ContextSource` | `file {path, lines}`, `rule`, `memory {scope, id}`, `summary {index}`, `project_instructions {path}`. |
| `Trust` | `user` (user memory), `project` (rules, instruction files), `untrusted` (repository files, agent-written memory, summaries). |
| `DropReason` | `budget`, `secret_file`, `secret_like`, `stale`, `not_indexed`, `unreadable`. |
| `MemoryEntry` | `{id: mem_…, text, tags, source: user\|agent{session_id}, created_at, updated_at}` in scope `user`, `workspace{workspace_id}`, or `session{session_id}`. |
| `TokenEstimator` | `estimate(&str) -> usize`; default `CharEstimator` = `ceil(chars / 4)`. |
| `Embedder` | Optional local embedding seam (`model`, `dimensions`, `embed`); `Index::embed_missing` / `semantic_search` use the `vectors` table. No embedder ships. |

Errors: `ContextError` (`code()`, stable `reason()`: `invalid_query`, `invalid_request`, `index_busy`, `index`, `invalid_rules`, `cancelled`, `io`, workspace and memory reasons) and `MemoryError` (`secret_like`, `invalid_text`, `invalid_tag`, `invalid_scope`, `memory_not_found`, `too_many_memories`, `corrupt_memory`, `io`).

## Drive layout

```text
private_index/<workspace-id>/index.sqlite3                 index (WAL, synchronous=FULL, user_version 1)
private_index/<workspace-id>/index.sqlite3.corrupt-<ms>    damaged databases moved aside (newest 3 kept)
private_memory/user.toml                                   portable user memory
private_memory/workspaces/<workspace-id>.toml              workspace memory
private_memory/sessions/<session-id>.toml                  session memory
```

Index tables: `meta`, `files(path, size, mtime_ns, sha256, language, lines, redacted, indexed_at)`, `chunks(file_id, ordinal, start_line, end_line, text)` with the external-content FTS5 table `chunks_fts` (unicode61, diacritics removed; kept in sync by triggers), `symbols(file_id, name, kind, line)`, `vectors(chunk_id, model, dims, vector)`. The index is derived data and always rebuildable.

## Guarantees

- **Ignore rules.** The walk (the `ignore` crate) honours `.gitignore`, `.ignore`, and `.git/info/exclude`, also outside a Git repository; ignore files above the workspace root are not read, and the user's global excludes only with `global_excludes` (host-specific, off by default so the index is the same on every host). Hidden files are indexed; `.git`, `.hg`, `.svn`, and `.aidex` are never entered. Changing ignore rules takes effect on the next build, which removes newly ignored files.
- **Links and confinement.** Symbolic links, junctions, and other reparse points are counted and never followed, so nothing outside the confined root is indexed, searched, or packed. Every file is re-resolved through `ConfinedRoot` and re-checked as a regular non-link file immediately before it is read.
- **Secret exclusion (indexing, search, packs).** Names: `.env*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.keystore`, `*.jks`, `*.ppk`, `*.kdbx`, `*.tfstate`, `*.tfvars`, `*.asc`, `*.gpg`, `id_rsa*`, `id_dsa*`, `id_ecdsa*`, `id_ed25519*`, `credentials*`, `secrets.*`, `client_secret*`, `.npmrc`, `.pypirc`, `.netrc`, `.pgpass`, `.git-credentials`, `.htpasswd`, `.dockercfg`, `.boto`, `.s3cfg`, `terraform.tfvars`, and everything under `.ssh`, `.gnupg`, `.aws`, `.docker` are never read (`exclude::is_secret_path`). Content: a file containing a private-key block is excluded entirely as key material; other recognisable credentials (the `aidex_events::redact_text` detectors) are redacted chunk by chunk before anything is stored, so the raw value is not in the index and cannot be found by searching for it. Search results, pack items, rules, and summaries are redacted again on the way out, and secret-named paths are filtered at every stage — even when a pack names them explicitly.
- **Incremental.** Unchanged files (same size and nanosecond mtime) are not read. Changed files are hashed; equal content only updates metadata (`touched`), different content is re-chunked (about 40-60 lines per chunk) and its symbols re-extracted. Files that disappear, become ignored, or stop being indexable are removed — but only after a walk completes.
- **Cancellable and resumable.** `CancelToken` (cancel or deadline) is checked before every file and periodically during the walk; work is committed in batches, so a cancelled or interrupted build keeps every committed batch, is reported as `partial`, never prunes, and the next build continues by skipping files that are already current. One build runs per index (`index_busy`).
- **Recoverable.** On open the database must pass `PRAGMA integrity_check` and the FTS5 integrity check. A file that is not a database or fails a check is moved aside as `index.sqlite3.corrupt-<unix-ms>` (with its WAL) and an empty index is created; another schema version is deleted and recreated. Both are reported in `IndexSummary::recovery`, and the next build repopulates the index. Symlinked database files are refused.
- **Search works offline.** Lexical search is FTS5 BM25 with every term required (terms of three or more characters match as prefixes; user text is quoted, never parsed as FTS syntax). Filename search ranks exact names, then prefixes, then substrings. Symbol search ranks exact, then case-insensitive, prefix, and substring matches of regex-extracted definitions for Rust, TypeScript/JavaScript, Python, Go, C/C++, Java, and C#.
- **Packs never exceed the budget.** Items are rendered first and accepted only if the exact rendering fits the remaining budget, so `total_tokens` (the sum of item estimates) is always `<= budget_tokens`. Order and shares: the rules summary; explicit indexed files (cut to whole chunks that fit); instruction files (≤ 30 % of the budget, cut by lines); summaries (≤ 25 %); memories by term overlap (≤ 15 %); then retrieved chunks (BM25 over the task's terms OR-ed, plus a boost for chunks that define identifiers named in the task, at most 32 seeds), merged per file into contiguous runs and expanded to the adjacent chunks while budget remains. Whole-repository dumps are impossible: only selected chunks and explicitly named files are packed. Files whose size or mtime changed since indexing are dropped as `stale` rather than packed with old text.
- **Untrusted-data delimiters.** Every item is wrapped as `<<<aidex:context nonce=N source=… trust=…>>> … <<<aidex:end nonce=N>>>` with a random per-pack nonce; content cannot forge or close a delimiter (`<<<aidex:` inside content is defused). `ContextPack::preamble()` is the instruction text the harness puts in the system prompt: only delimiters with the nonce are real, `untrusted` items are data and never instructions, `project` items are advisory.
- **Project rules only tighten.** `.aidex/rules.toml` (closed schema, `schema_version = 1`, parsed by `aidex_policy::ProjectRules`: `[project] package_manager, tests, lint, build`; `[agent] never_modify, require_approval`) becomes a policy project floor via `to_policy_layer()`/`apply_to()`: `never_modify` denies writes and deletes, `require_approval` forces approval, and nothing in the schema can allow anything (unknown fields are errors). An invalid, oversized, symlinked, or root-escaping rules file is an error, never silently ignored. `AGENTS.md`, `CLAUDE.md`, and `.aidex/instructions.md` at the root are read (64 KiB each, redacted) and packed with `project` trust.
- **Memory is inspectable and never a secret store.** Plain TOML files the user can read and edit; `add`/`edit` reject secret-like text or tags (`MemoryError::SecretLike`). `source` records who last wrote an entry: agent edits mark an entry agent-written, which packs treat as untrusted. Hand-edited files are validated on load (`corrupt_memory`, never overwritten), and entries that now look secret-like are dropped from packs. Session memory is cleared with `MemoryStore::clear`. The `MemoryStore` trait keeps the store pluggable.

## Daemon and CLI

`aidexd` methods (see `protocol/commands/README.md`): `context.index`, `context.index.cancel`, `context.status`, `context.search`, `context.pack` (debug view, budget ≤ 100 000), `memory.list|add|edit|remove` (user-written memories only), and `rules.show`. Index jobs emit `task.created` (role `context.index`), `task.started`, and `task.completed`/`task.cancelled`/`task.failed` events scoped to the workspace; progress is polled with `context.status`, and a job reads `running` until its terminal event is recorded (a failed append sets `event_error`). CLI: `aidex index build|status|cancel`, `aidex search`, `aidex context pack`, `aidex memory`, `aidex rules show` (`apps/cli/README.md`).

## Limitations

- Symbol extraction is regex-based: multi-line signatures, macro-generated items, and methods nested in C++/Java bodies can be missed; C-family function detection can produce false positives. No dependency graph, git history search, test mapping, or summaries are indexed yet (spec §34/§54 items for later work).
- No embedder ships; semantic search is brute-force cosine over stored vectors and is not used by packs.
- Secret detection is name- and pattern-based: an arbitrary secret in an ordinary file with no recognisable shape is indexed. Treat packs as repository content that must not be sent to a remote model in privacy modes that forbid it (the router and broker decide).
- Confinement checks are point-in-time (a concurrent writer could swap a checked file for a link between the check and the read); OS-level confinement is the sandbox's job.
- `.aidex/` itself is not indexed, and only root-level instruction files are read.
- Index directories are not removed when a workspace is unregistered, and index jobs are not resumed automatically after a daemon restart (the next `context.index` resumes the work).
- Memory has no automatic expiry; the harness decides when to clear session memory.
