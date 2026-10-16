// Mirrors the `update.*` DTOs in crates/aidexd/src/methods/update.rs and the `aidex_updater`
// reports they return (crates/updater/src/), with `aidex_bundle` types. Hand-written until schema
// generation exists. Apply and rollback switch `app/current.json` only: the new version starts on
// the next launch (`restart_required`). Refusals carry `data.reason` (stable, e.g. `bad_signature`,
// `downgrade`, `updater_unavailable`) and `data.integrity` (an authenticity or integrity refusal).

/** `aidex_bundle::Target`. */
export type BundleTarget =
  | "windows-x64"
  | "linux-x64"
  | "linux-arm64"
  | "macos-arm64"
  | "macos-x64";

/** One installed bundle: `dir` is `versions/<name>` under the drive's `app/`. */
export type VersionRef = { version: string; dir: string; manifest_sha256: string };

/** Where a trusted signing key came from. */
export type KeySource = "release" | "build" | "development" | "explicit";

/** An accepted signature check (`status` tagged). */
export type SignatureState =
  | { status: "verified"; key_id: string; key_name: string; key_source: KeySource }
  | { status: "unsigned_allowed" };

/** What applying a package does. */
export type UpdateAction =
  | "install"
  | "upgrade"
  | "repair"
  | "downgrade_to_previous"
  | "up_to_date";

/** Update journal step; `committed` and `aborted` are terminal. */
export type JournalState =
  | "staging"
  | "verified"
  | "self_test"
  | "switching"
  | "committed"
  | "aborted";

export type JournalOperation = "apply" | "rollback";

/**
 * Params of `update.check` and `update.apply`. `source` is an absolute path of a package archive,
 * an unpacked bundle, or a release directory. `allow_unsigned` is for development only.
 */
export type UpdateSourceParams = {
  source: string;
  allow_downgrade?: boolean;
  allow_unsigned?: boolean;
};

/** Params of `update.rollback` (may be omitted). */
export type UpdateRollbackParams = { allow_unsigned?: boolean };

/** Facts about a verified package or installed bundle. `created_at` is RFC 3339. */
export type CandidateInfo = {
  version: string;
  target: BundleTarget;
  manifest_sha256: string;
  created_at: string;
  git_commit: string | null;
  signature: SignatureState;
  /** The bundle lists `docs/sbom.cdx.json`. */
  sbom: boolean;
};

/** Counts from a valid SBOM. */
export type SbomSummary = {
  spec_version: string;
  /** `name@version` of the described product. */
  subject: string;
  components: number;
  libraries: number;
  files: number;
  hashed: number;
  licensed: number;
};

export type SbomStatus = {
  present: boolean;
  valid: boolean;
  summary: SbomSummary | null;
  error: string | null;
};

/** Integrity of the active bundle, verified exactly as the launcher does. */
export type IntegrityReport = {
  target: BundleTarget | null;
  installed: boolean;
  verified: boolean;
  scope: "critical" | "all";
  version: string | null;
  dir: string | null;
  manifest_sha256: string | null;
  verified_files: number | null;
  signature: SignatureState | null;
  signer_key_id: string | null;
  sbom: SbomStatus;
  previous: VersionRef | null;
  /** The state of an unfinished update journal. */
  pending_update: JournalState | null;
  /** Stable reason when not verified (`not_installed`, `unsigned`, `tampered`, ...). */
  reason: string | null;
  error: string | null;
};

/** The last (or unfinished) journalled operation. Times are RFC 3339. */
export type JournalSummary = {
  id: string;
  operation: JournalOperation;
  state: JournalState;
  started_at: string;
  updated_at: string;
  candidate_version: string | null;
  to: VersionRef | null;
  signer_key_id: string | null;
  outcome: string | null;
};

export type RecoveryOutcome = "clean" | "rolled_back" | "rolled_forward" | "journal_set_aside";

/** What startup recovery did; `removed` paths are relative to `app/`. */
export type RecoveryReport = {
  outcome: RecoveryOutcome;
  update_id: string | null;
  operation: JournalOperation | null;
  interrupted_state: JournalState | null;
  active: VersionRef | null;
  restored_pointer: boolean;
  removed: string[];
  warnings: string[];
};

/** Result of `update.status`: `aidex_updater::UpdateStatus` plus the daemon's own facts. */
export type UpdateStatus = {
  target: BundleTarget;
  current: VersionRef | null;
  previous: VersionRef | null;
  integrity: IntegrityReport;
  journal: JournalSummary | null;
  /** An operation is unfinished and needs recovery. */
  pending: boolean;
  /** Directories under `app/versions/`. */
  versions: string[];
  running_dir: string | null;
  /** The running process is not the version `app/current.json` selects. */
  restart_required: boolean;
  /** The running daemon's version. */
  daemon_version: string;
  /** What recovery did when this daemon started (`null` if it has not run). */
  recovery: RecoveryReport | null;
  recovery_error: string | null;
};

/** How the package's `SHA256SUMS` entry was checked. */
export type ChecksumStatus = "verified" | "absent" | "not_applicable";

export type UpdateSource = { type: "local_file" | "directory"; path: string };

/** Result of `update.check`: nothing is changed. */
export type CheckReport = {
  source: UpdateSource;
  /** The package that was checked (an archive, or an unpacked bundle directory). */
  package: string;
  checksum: ChecksumStatus;
  candidate: CandidateInfo;
  current: VersionRef | null;
  previous: VersionRef | null;
  action: UpdateAction;
};

export type LauncherPlacement = {
  path: string;
  action: "created" | "replaced" | "unchanged";
  /** Where the replaced launcher was kept. */
  previous: string | null;
};

export type SelfTestCommand = { command: string; exit_code: number | null; duration_ms: number };

/** Result of `update.apply`. */
export type ApplyReport = {
  update_id: string;
  action: UpdateAction;
  checksum: ChecksumStatus;
  candidate: CandidateInfo;
  from: VersionRef | null;
  to: VersionRef;
  self_test: { commands: SelfTestCommand[] } | null;
  launchers: LauncherPlacement[];
  /** Version directories removed after the commit. */
  pruned: string[];
  warnings: string[];
  restart_required: boolean;
};

/** Result of `update.rollback`: `to` (the former previous) is active and `from` became previous. */
export type RollbackReport = {
  update_id: string;
  from: VersionRef;
  to: VersionRef;
  signature: SignatureState;
  launchers: LauncherPlacement[];
  warnings: string[];
  restart_required: boolean;
};
