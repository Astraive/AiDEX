# aidex-workspace

Owns AiDEX workspaces (spec §23, §12.2-12.3): directories a user explicitly selected on the host, workspaces stored on the drive, and reviewable ephemeral copies. It persists a registry on the drive, gives every workspace a stable identity, and resolves workspace-relative paths so tools cannot escape the workspace root. `aidexd` serves it as the `workspace.*` JSON-RPC methods; `aidex workspace` (alias `aidex project`) is the CLI.

## API

```rust
let registry = WorkspaceRegistry::open(&drive_paths)?;            // private_config/workspaces.sqlite3
let host  = registry.add_host(Path::new("/home/me/repo"), Some("repo"))?;
let notes = registry.create_drive("notes")?;                       // workspaces/<id>/tree
registry.select(&host.id)?;                                        // persisted selection
let handle = registry.handle(&host.id)?;                           // WorkspaceHandle
let file = handle.resolve(&WorkspacePath::new("src/main.rs")?, Access::Read)?;

let copy = registry.create_ephemeral(&host.id)?;                   // EphemeralCreated
let diff = registry.review(&copy.workspace.id)?;                   // EphemeralDiff
registry.apply(&copy.workspace.id, &ApplySelection::Paths(vec![..]), Some(&diff.digest))?;
registry.discard(&copy.workspace.id)?;
```

- **Model** — `Workspace { id: EntityId ("wsp_<ULID>"), name, kind, root, created_at, last_opened_at, path, availability, repository }`. `WorkspaceKind::{Host, Drive, Ephemeral { source }}`. `WorkspaceRoot::{Host(String), Drive(RelativePath)}`: drive and ephemeral roots are relative to `DrivePaths::workspaces()` so they survive drive-letter and mount-point changes. `path`, `availability` (`Available | Unavailable { reason }`), and `repository` (`RepositoryInfo { vcs, root, is_workspace_root }`) are computed on every read.
- **Registry** — `WorkspaceRegistry::open(&DrivePaths)`; `add_host(&Path, Option<&str>)`, `create_drive(&str)`, `list()`, `get(&EntityId)`, `find(&str)` (id, else case-insensitive name), `select(&EntityId)` / `selected()`, `touch_opened(&EntityId)`, `remove(&EntityId)` (unregister only), `delete_data(&EntityId)` (drive/ephemeral only), `confined_root(&EntityId) -> ConfinedRoot`, `handle(&EntityId) -> WorkspaceHandle`. `with_limits(EphemeralLimits)` adjusts copy/review bounds.
- **Confinement** — `ConfinedRoot::new(root)`, `resolve(&WorkspacePath, Access::{Read, Write, Create}) -> PathBuf`, `relativize(&Path) -> WorkspacePath`, `contains(&Path)`, `root()`, `display_root()`. `parse_workspace_path(&str)` applies the lexical and platform rules to caller input. `WorkspaceHandle` bundles the `Workspace` with its `ConfinedRoot` (`id()`, `workspace()`, `root()`, `resolve()`, `relativize()`).
- **Discovery** — `discover(&Path) -> Discovery { selected_root, enclosing_repository, nested_repositories, vcs, scan_truncated }`, scanning at most `MAX_SCAN_DEPTH` (4) levels and `MAX_SCAN_ENTRIES` (2000) entries without following links.
- **Ephemeral** — `create_ephemeral(&EntityId) -> EphemeralCreated { workspace, files, bytes, skipped }`; `review(&EntityId) -> EphemeralDiff { changes: Vec<FileChange>, conflicts, unsupported, digest, .. }` where `FileChange { path, kind: Added|Modified|Deleted, binary, size, sha256, unified_diff, diff_truncated, conflict: Option<ConflictReason> }`; `apply(&EntityId, &ApplySelection::{All, Paths(..)}, Option<&str> digest) -> ApplyReport { applied, skipped_conflicts, failed }`; `discard(&EntityId)`.
- **Errors** — `WorkspaceError` with `code() -> ServiceErrorCode` and a finer stable `reason()` (`invalid_path`, `escapes_root`, `symlink_leaf`, `root_unavailable`, `workspace_not_found`, `path_not_found`, `forbidden_location`, `conflict`, `review_changed`, `not_a_change`, `ownership_marker`, `too_large`, ...). The daemon forwards `reason` as `data.reason`.

## Drive layout

```text
private_config/workspaces.sqlite3          registry (WAL, synchronous=FULL, PRAGMA user_version = 1)
workspaces/<id>/aidex-workspace.json       ownership marker (drive workspace)
workspaces/<id>/tree/                      drive workspace root
workspaces/ephemeral/<id>/aidex-workspace.json
workspaces/ephemeral/<id>/baseline.json    path -> size + SHA-256 at copy time
workspaces/ephemeral/<id>/tree/            ephemeral workspace root
```

## Guarantees

- **Explicit roots.** A host root is exactly the directory the user selected, canonicalized; discovery only informs. It must exist. The drive root, any directory that contains the drive (which would expose the vault, identity, and sessions), and roots containing or lying inside any declared managed directory or `.aidex` are refused, including custom layouts. Unrelated sibling directories remain allowed. The check is repeated whenever a host root is resolved for use. A root that is already registered (in any spelling) is a conflict; names are unique case-insensitively.
- **Unavailable roots are reported, never recreated.** A missing host root (or one added on another OS) is `Unavailable` in `list`/`get` and `RootUnavailable` for `confined_root`/`handle`; nothing is created. The registry, ids, and selection survive restarts.
  Persisted host roots replaced by symbolic links or junctions are unavailable;
  workspace access never redirects to the replacement target.
- **Confinement.** `WorkspacePath` rejects traversal, absolute and drive-letter paths, UNC paths, backslashes, and `:` (so NTFS alternate data streams). Resolution walks components from the canonical root: real entries are physical children, and a symbolic link or junction is followed only when its fully resolved target stays inside the root; dangling or looping links fail closed (`EscapesRoot`). `Create` refuses a leaf that is itself a link. On Windows, reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM0-9`, `LPT0-9`, superscript forms, `CONIN$`, `CONOUT$`, any extension), trailing dots/spaces, and `<>:"|?*` are rejected; comparisons ignore case and treat `\\?\C:\x`/`C:\x` and `\\?\UNC\s\x`/`\\s\x` as the same location. Drive workspace roots must be reached through real directories from `workspaces/`.
- **Ephemeral copies.** The copy honours `.gitignore` files (from the source root down, plus those between an enclosing repository's root and the source, and `.git/info/exclude`; the user's global excludes file is host-specific and not read), always skips `.git/` and `.aidex/`, never follows or reproduces links (reported as `skipped`), and hashes exactly the bytes it writes into the baseline. Exceeding `EphemeralLimits` (default 50 000 files, 2 GiB) aborts and removes the partial copy.
  Repository excludes also come from valid `gitdir:` files (separate Git directories)
  and linked worktrees' `commondir` metadata, matched against the repository root.
- **Review before apply.** Review compares every baseline entry explicitly (even if now ignored) and lists new non-ignored files; links, special files, and `.git`/`.aidex` content in the copy are never reviewable or applied. A change conflicts when the source no longer matches the baseline (unless it already equals the result) or its path is now unsafe. Diffs are bounded (1 MiB input, 64 KiB per file, 256 KiB per review).
  File-count and aggregate-byte limits also bound the mutable review tree, including
  baseline files newly ignored by the copy. The byte cap is enforced while hashing
  actual contents, even when files grow after the directory scan; source conflict
  checks have a separate aggregate cap of the same size. Inputs used to generate
  text diffs are reread with bounded reads and checked against their recorded sizes
  and hashes; changes detected during those reads refuse the review. Oversized
  review/apply returns `TooLarge` before any selected change is applied.
- **Selective, atomic apply.** Only selected, non-conflicting changes are written; a selected path that is not a change rejects the whole request before anything is written, and a `digest` that no longer matches refuses the apply (`ReviewChanged`). Each file is re-verified (ephemeral hash, source state, confinement after creating parents and again just before rename), written to an exclusive temporary file, fsynced, and renamed into place. Applied changes are folded into the baseline; rejected and unselected changes are never applied.
- **Deletion only of AiDEX-created data.** `remove` only unregisters. `delete_data`/`discard` refuse host workspaces, delete only `workspaces/<id>/` or `workspaces/ephemeral/<id>/` after verifying the ownership marker names that workspace, refuse when the directory or its parents are links, remove links inside the tree without following them, and never touch anything else. A workspace with ephemeral copies cannot be removed until they are discarded.
  Ephemeral creation and source data deletion share the operation lock: a completed
  child prevents source deletion, and a completed deletion prevents child creation.

## Limitations

- Resolution is a point-in-time check (TOCTOU): a process that can modify the workspace concurrently could swap a verified directory for a link between resolution and use, and `create_dir_all` during apply could then create empty directories through it. Apply narrows the window by re-resolving before each rename. OS-enforced confinement (handles, `openat`, sandbox mounts) belongs to `aidex-sandbox`, which is not yet wired to workspace roots.
- Windows 8.3 short names are not normalized; they resolve as the directories they alias but compare differently (which can only cause a false refusal). NTFS per-directory case sensitivity is not detected. Reserved-name rules are enforced only on Windows.
- Empty directories are not tracked by the baseline or by review; directory deletions are not applied (emptied directories remain). File permissions other than the Unix mode bits are not preserved.
- Review requires the source to be available. Very large reviews are bounded, not paginated; the daemon truncates the listed entries to fit one IPC message while the digest still covers every change.
  Changes, conflicts, and unsupported entries are trimmed independently, including
  reviews with no changes. The digest and total change/conflict counts remain complete.
- No mirror/clone workspaces, worktree management (§33.1), or git integration beyond detecting `.git`. Workspace events are returned as values, not published: `aidex-events` wiring comes later.
