// Permission labels and descriptions that say what each mode actually does in AiDEX's harness
// (crates/harness/src/broker.rs): Auto asks like Supervised, deleting files always asks, and shell
// commands outside Full access need filesystem confinement, which Windows does not have.
// Pure functions, so they run during prerendering.
import type { PermissionMode } from "@aidex/client/dto/sessions";

export const PERMISSION_LABELS: Record<PermissionMode, string> = {
  supervised: "Supervised",
  auto_accept_edits: "Auto-accept edits",
  auto: "Auto (asks like Supervised)",
  full_access: "Full access",
};

/** What the engine reports about running commands outside Full access. */
export type PermissionContext = {
  /** Commands outside Full access are refused on this computer (no filesystem confinement). */
  shellNeedsFullAccess: boolean;
};

const SHELL_NOTE = "Commands need Full access on this computer.";

/** One sentence (two when commands need Full access) describing `mode` honestly. */
export function permissionDescription(mode: PermissionMode, context: PermissionContext): string {
  const shell = context.shellNeedsFullAccess ? ` ${SHELL_NOTE}` : "";
  switch (mode) {
    case "supervised":
      return `Ask before commands and file changes.${shell}`;
    case "auto_accept_edits":
      return `Auto-approve edits, ask before other actions.${shell}`;
    case "auto":
      return `Asks before commands and file changes, like Supervised.${shell}`;
    case "full_access":
      return "Allow commands and edits without prompts; deleting files still asks.";
  }
}

export type PermissionChoice = { value: PermissionMode; title: string; description: string };

/**
 * The permission modes to offer, in order. Auto behaves like Supervised, so it is listed only when
 * the conversation already uses it (`current`), labelled as such.
 */
export function permissionChoices(context: PermissionContext, current?: PermissionMode): PermissionChoice[] {
  const modes: PermissionMode[] =
    current === "auto"
      ? ["supervised", "auto_accept_edits", "auto", "full_access"]
      : ["supervised", "auto_accept_edits", "full_access"];
  return modes.map((value) => ({
    value,
    title: PERMISSION_LABELS[value],
    description: permissionDescription(value, context),
  }));
}
