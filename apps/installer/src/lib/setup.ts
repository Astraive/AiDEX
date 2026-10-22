// Setup choices and planning estimates. Drives are real (`list_drives`); component sizes are
// planning estimates until the portable distribution manifest exists (issue #26), and the
// component catalog is a sample until model import (#15) and agent adapters (#23) land.
import type { VolumeInfo } from "@aidex/client/dto/drive";
import { bytesToGb } from "@aidex/client/format";

export type Preset = "lite" | "developer" | "custom";
export type ModelId = "gemma" | "embedding" | "custom";
export type AgentId = "codex" | "claude" | "omp";
export type ExtensionId = "skills" | "mcp" | "toolchains";
export type Backend = "auto" | "cpu" | "gpu";
export type VaultMode = "password" | "none";
export type InstallMode = "normal" | "portable";

export type SetupState = {
  /** Target root: per-user app directory in normal mode, removable volume root in portable mode. */
  driveRoot: string | null;
  installMode: InstallMode;
  preset: Preset;
  models: readonly ModelId[];
  customModelPath: string;
  backend: Backend;
  agents: readonly AgentId[];
  extensions: readonly ExtensionId[];
  vault: VaultMode;
};

/** AiDEX app, native agent harness, and the llama.cpp runtime are always included. */
const CORE_GB = 1.1;
const MODEL_GB: Record<ModelId, number> = { gemma: 6.2, embedding: 0.45, custom: 1.1 };
const AGENT_GB: Record<AgentId, number> = { codex: 0.3, claude: 0.3, omp: 0.3 };
const EXTENSION_GB: Record<ExtensionId, number> = { skills: 0.1, mcp: 0.2, toolchains: 0.8 };

const sumSizes = <Id extends string>(ids: readonly Id[], sizes: Record<Id, number>) =>
  ids.reduce((total, id) => total + sizes[id], 0);

export function estimateGb(state: SetupState): number {
  return CORE_GB + sumSizes(state.models, MODEL_GB) + sumSizes(state.agents, AGENT_GB) + sumSizes(state.extensions, EXTENSION_GB);
}

export const formatGb = (gb: number): string => `${gb.toFixed(1)} GB`;

/** Free space of a volume in GB, or `null` when the platform did not report it. */
export const freeGb = (volume: VolumeInfo | null): number | null =>
  volume?.free_bytes == null ? null : bytesToGb(volume.free_bytes);

/** Whether the estimate fits; `false` when no drive is chosen or its free space is unknown. */
export const fitsOnDrive = (state: SetupState, volume: VolumeInfo | null): boolean => {
  const free = freeGb(volume);
  return free !== null && estimateGb(state) <= free;
};

const PRESET_SELECTIONS: Record<Exclude<Preset, "custom">, Pick<SetupState, "models" | "agents" | "extensions">> = {
  lite: { models: [], agents: [], extensions: [] },
  developer: { models: ["gemma", "embedding"], agents: ["codex", "claude", "omp"], extensions: ["skills", "mcp", "toolchains"] },
};

/** Lite and Developer reset the component selections; Custom keeps the current ones so they can be refined. */
export function applyPreset(state: SetupState, preset: Preset): SetupState {
  return preset === "custom" ? { ...state, preset } : { ...state, preset, ...PRESET_SELECTIONS[preset] };
}

export const initialSetup: SetupState = {
  driveRoot: null,
  installMode: "normal",
  preset: "developer",
  ...PRESET_SELECTIONS.developer,
  customModelPath: "",
  backend: "auto",
  // Durable private data is stored encrypted by default (docs/security.md).
  vault: "password",
};

export const presetOptions = [
  { value: "lite", title: "Lite", description: "AiDEX app and native agent harness. No bundled model or external tools.", meta: "Smallest" },
  { value: "developer", title: "Developer", description: "Recommended portable setup with local AI and CLI agents.", meta: "Recommended" },
  { value: "custom", title: "Custom", description: "Start with the developer setup, then choose exactly what to include.", meta: "Flexible" },
] as const;

export const modelOptions = [
  { value: "gemma", title: "Gemma 4 (GGUF)", description: "Portable sample model package for offline tasks.", meta: formatGb(MODEL_GB.gemma) },
  { value: "embedding", title: "Embedding model", description: "Optional local repository search and retrieval.", meta: formatGb(MODEL_GB.embedding) },
  { value: "custom", title: "Bring your own GGUF", description: "Add a model file you already have." },
] as const;

export const agentOptions = [
  { value: "codex", title: "codex", description: "Coding CLI adapter", meta: formatGb(AGENT_GB.codex) },
  { value: "claude", title: "claude code", description: "External coding agent CLI", meta: formatGb(AGENT_GB.claude) },
  { value: "omp", title: "omp", description: "Advanced agent harness", meta: formatGb(AGENT_GB.omp) },
] as const;

export const extensionOptions = [
  { value: "skills", title: "Skills", description: "Reusable agent workflows", meta: formatGb(EXTENSION_GB.skills) },
  { value: "mcp", title: "MCP servers", description: "Tools and connectors", meta: formatGb(EXTENSION_GB.mcp) },
  { value: "toolchains", title: "Toolchains", description: "Developer runtime helpers", meta: formatGb(EXTENSION_GB.toolchains) },
] as const;

export const backendOptions = [
  { value: "auto", label: "Auto detect" },
  { value: "cpu", label: "CPU only" },
  { value: "gpu", label: "Prefer GPU" },
] as const;
