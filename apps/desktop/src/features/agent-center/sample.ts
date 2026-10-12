// Sample data until AiDEX Core exposes the agent registry, external runtime adapters, and agent settings.
import { BotIcon, CodeIcon, SearchIcon, TerminalIcon, type IconProps } from "@aidex/ui/icons";
import type { ComponentType } from "react";

export type AgentStatus = "ready" | "installed" | "not-installed";
export type Autonomy = "ask-every-step" | "ask-before-edits" | "autonomous";

export type AgentPermissions = {
  readFiles: boolean;
  editFiles: boolean;
  runShell: boolean;
  network: boolean;
  vaultSecrets: boolean;
};

export type AgentConfig = {
  instructions: string;
  modelId: string;
  /** "none" when the agent has no fallback model. */
  fallbackModelId: string;
  permissions: AgentPermissions;
  autonomy: Autonomy;
  /** Kept as text so the field can hold partial input while typing. */
  maxSteps: string;
  sandbox: string;
  timeoutMinutes: string;
};

export type SampleAgent = {
  id: string;
  name: string;
  kind: "built-in" | "external";
  description: string;
  /** Version or runtime line, e.g. "Built in · v0.1". */
  runtime: string;
  status: AgentStatus;
  Icon: ComponentType<IconProps>;
  config: AgentConfig;
};

export const DEFAULT_AGENT_ID = "aidex-native";

export const statusLabel: Record<AgentStatus, string> = {
  ready: "Ready",
  installed: "Installed",
  "not-installed": "Not installed",
};

const basePermissions: AgentPermissions = {
  readFiles: true,
  editFiles: true,
  runShell: true,
  network: false,
  vaultSecrets: false,
};

const baseConfig: AgentConfig = {
  instructions: "",
  modelId: "qwen-coder-7b",
  fallbackModelId: "gemma-4",
  permissions: basePermissions,
  autonomy: "ask-before-edits",
  maxSteps: "40",
  sandbox: "standard",
  timeoutMinutes: "30",
};

export const sampleAgents: readonly SampleAgent[] = [
  {
    id: "aidex-native",
    name: "AiDEX Native",
    kind: "built-in",
    description: "Built-in coding harness that plans, edits, and tests inside your workspace.",
    runtime: "Built in · v0.1",
    status: "ready",
    Icon: BotIcon,
    config: {
      ...baseConfig,
      instructions: [
        "You are AiDEX Native, a careful coding agent working in the user's workspace.",
        "- Read the relevant files before proposing a change.",
        "- Make the smallest change that solves the task, then run the tests.",
        "- Ask before deleting files or touching anything outside the workspace.",
        "- Summarize what changed and why when you finish.",
      ].join("\n"),
    },
  },
  {
    id: "code-reviewer",
    name: "Code reviewer",
    kind: "built-in",
    description: "Reviews changes and finds regressions before you commit.",
    runtime: "Built in · v0.1",
    status: "ready",
    Icon: CodeIcon,
    config: {
      ...baseConfig,
      permissions: { ...basePermissions, editFiles: false, runShell: false },
      autonomy: "autonomous",
      maxSteps: "20",
      instructions: [
        "You review diffs in the current workspace. You never edit files.",
        "- Flag bugs, security issues, and missing tests first.",
        "- Quote the exact lines you are commenting on.",
        "- Keep style suggestions short and clearly optional.",
      ].join("\n"),
    },
  },
  {
    id: "research-agent",
    name: "Research agent",
    kind: "built-in",
    description: "Searches the workspace and the web, then summarizes context with sources.",
    runtime: "Built in · v0.1",
    status: "ready",
    Icon: SearchIcon,
    config: {
      ...baseConfig,
      modelId: "gemma-4",
      fallbackModelId: "llama-3.2-3b",
      permissions: { ...basePermissions, editFiles: false, runShell: false, network: true },
      autonomy: "autonomous",
      maxSteps: "25",
      instructions: [
        "You gather context for the user's question.",
        "- Prefer files in the workspace over web results.",
        "- Cite every source you use.",
        "- End with open questions you could not answer.",
      ].join("\n"),
    },
  },
  {
    id: "codex",
    name: "codex",
    kind: "external",
    description: "OpenAI's coding CLI, run through the AiDEX adapter.",
    runtime: "External CLI adapter · v0.46",
    status: "installed",
    Icon: TerminalIcon,
    config: { ...baseConfig, modelId: "provider-default", fallbackModelId: "none", instructions: "Follow the repository's AGENTS.md." },
  },
  {
    id: "claude-code",
    name: "claude code",
    kind: "external",
    description: "Anthropic's coding CLI, run through the AiDEX adapter.",
    runtime: "External CLI adapter",
    status: "not-installed",
    Icon: TerminalIcon,
    config: { ...baseConfig, modelId: "provider-default", fallbackModelId: "none", instructions: "Follow the repository's CLAUDE.md." },
  },
  {
    id: "omp",
    name: "omp",
    kind: "external",
    description: "Open multi-agent runtime for long-running tasks.",
    runtime: "External agent runtime",
    status: "not-installed",
    Icon: TerminalIcon,
    config: { ...baseConfig, fallbackModelId: "none" },
  },
];

export const builtInAgents = sampleAgents.filter((agent) => agent.kind === "built-in");
export const externalRuntimes = sampleAgents.filter((agent) => agent.kind === "external");

/** The agent named by `id`, falling back to AiDEX Native for missing or unknown ids. */
export const agentById = (id: string | undefined): SampleAgent =>
  sampleAgents.find((agent) => agent.id === id) ?? sampleAgents[0];
