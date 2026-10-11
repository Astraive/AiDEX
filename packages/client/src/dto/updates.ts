/** Mirrors aidex-updater and aidexd::methods::update; signatures remain mandatory in the desktop. */
export type BundleTarget =
  "windows-x64" | "linux-x64" | "linux-arm64" | "macos-arm64" | "macos-x64";
export type VersionRef = {
  version: string;
  dir: string;
  manifest_sha256: string;
};
export type SignatureState =
  | {
      status: "verified";
      key_id: string;
      key_name: string;
      key_source: "release" | "build" | "development" | "explicit";
    }
  | { status: "unsigned_allowed" };
export type UpdateAction =
  "install" | "upgrade" | "repair" | "downgrade_to_previous" | "up_to_date";
export type UpdateSourceParams = {
  source: string;
  allow_downgrade?: boolean;
  allow_unsigned?: boolean;
};
export type JournalState =
  "staging" | "verified" | "self_test" | "switching" | "committed" | "aborted";
export type CandidateInfo = {
  version: string;
  target: BundleTarget;
  manifest_sha256: string;
  created_at: string;
  git_commit: string | null;
  signature: SignatureState;
  sbom: boolean;
};
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
  sbom: {
    present: boolean;
    valid: boolean;
    summary: {
      spec_version: string;
      subject: string;
      components: number;
      libraries: number;
      files: number;
      hashed: number;
      licensed: number;
    } | null;
    error: string | null;
  };
  previous: VersionRef | null;
  pending_update: JournalState | null;
  reason: string | null;
  error: string | null;
};
export type RecoveryReport = {
  outcome: "clean" | "rolled_back" | "rolled_forward" | "journal_set_aside";
  update_id: string | null;
  operation: "apply" | "rollback" | null;
  interrupted_state: JournalState | null;
  active: VersionRef | null;
  restored_pointer: boolean;
  removed: string[];
  warnings: string[];
};
export type UpdateStatus = {
  target: BundleTarget;
  current: VersionRef | null;
  previous: VersionRef | null;
  integrity: IntegrityReport;
  journal: {
    id: string;
    operation: "apply" | "rollback";
    state: JournalState;
    started_at: string;
    updated_at: string;
    candidate_version: string | null;
    to: VersionRef | null;
    signer_key_id: string | null;
    outcome: string | null;
  } | null;
  pending: boolean;
  versions: string[];
  running_dir: string | null;
  restart_required: boolean;
  daemon_version: string;
  recovery: RecoveryReport | null;
  recovery_error: string | null;
};
export type CheckReport = {
  source: { type: "local_file" | "directory"; path: string };
  package: string;
  checksum: "verified" | "absent" | "not_applicable";
  candidate: CandidateInfo;
  current: VersionRef | null;
  previous: VersionRef | null;
  action: UpdateAction;
};
export type LauncherPlacement = {
  path: string;
  action: "created" | "replaced" | "unchanged";
  previous: string | null;
};
export type ApplyReport = {
  update_id: string;
  action: UpdateAction;
  checksum: CheckReport["checksum"];
  candidate: CandidateInfo;
  from: VersionRef | null;
  to: VersionRef;
  self_test: {
    commands: {
      command: string;
      exit_code: number | null;
      duration_ms: number;
    }[];
  } | null;
  launchers: LauncherPlacement[];
  pruned: string[];
  warnings: string[];
  restart_required: boolean;
};
export type RollbackReport = {
  update_id: string;
  from: VersionRef;
  to: VersionRef;
  signature: SignatureState;
  launchers: LauncherPlacement[];
  warnings: string[];
  restart_required: boolean;
};
