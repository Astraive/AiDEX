// Mirrors the installer reports in apps/installer/src-tauri/src/reports.rs.
// Hand-written until schema generation exists.
import type { BridgeErrorPayload } from "../errors";
import type { DriveHealth, VolumeInfo } from "./drive";

/** Verified distribution metadata, canonical in aidex-bundle. */
export type BundleCatalog = {
  source: string;
  version: string;
  target: string;
  installed_bytes: number;
  required_free_bytes: number;
  file_count: number;
  manifest_sha256: string;
};
export type InstallJob = {
  id: string;
  drive_root: string;
  state: "running" | "completed" | "failed" | "cancelled" | "interrupted";
  step: string;
  completed_steps: number;
  total_steps: number;
  report: { version: string; dir: string } | null;
  error: string | null;
};

/** `inspect_drive` / `inspect_normal_target`: target volume, eligibility, and existing AiDEX state. */
export type DriveInspection = {
  /** Target path: a removable drive root in portable mode or a per-user directory in standard mode. */
  root: string;
  volume: VolumeInfo;
  /** Whether this target can be initialized using its selected installation mode. */
  eligible: boolean;
  /** Why it cannot be initialized, when `eligible` is false. */
  ineligible_reason: string | null;
  /** Manifest health when the volume already holds a supported AiDEX manifest (read only). */
  health: DriveHealth | null;
  /** Why the existing manifest could not be read, when there is one. */
  health_error: BridgeErrorPayload | null;
};

/** `init_drive`: what initialization found and did. */
export type InitReport = {
  root: string;
  /** Manifest schema before init (`null` when the drive had none). */
  previous_schema_version: number | null;
  health: DriveHealth;
  /** Manifest-declared directories, relative to the root; all exist after init. */
  directories: string[];
};
