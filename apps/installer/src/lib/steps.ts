export type StepId = "drive" | "setup" | "models" | "agents" | "security" | "install";

/** The six setup steps in order; `kicker` names each step's subject above its title. */
export const steps = [
  { id: "drive", title: "Target", kicker: "Installation mode" },
  { id: "setup", title: "Setup", kicker: "Configuration" },
  { id: "models", title: "Models", kicker: "Inference" },
  { id: "agents", title: "Agents", kicker: "Toolchains" },
  { id: "security", title: "Security", kicker: "Protection" },
  { id: "install", title: "Install", kicker: "Review" },
] as const satisfies readonly { id: StepId; title: string; kicker: string }[];

export const LAST_STEP = steps.length - 1;

export const stepKicker = (index: number): string => `Step ${index + 1} of ${steps.length} · ${steps[index].kicker}`;
