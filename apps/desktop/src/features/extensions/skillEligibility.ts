import type { SkillSummary } from "@aidex/client/dto/extensions";
import type { PermissionMode } from "@aidex/client/dto/sessions";
export type SkillScope = { workspaceId: string; profile: PermissionMode };
/** Manual selection may include manual-only skills, but never bypasses trust or scope. */
export function skillSelectionReason(
  skill: SkillSummary,
  scope?: SkillScope,
): string | undefined {
  if (!skill.enabled) return "Disabled";
  if (!skill.trusted) return "Untrusted";
  if (!skill.user_invocable) return "Manual invocation unavailable";
  if (scope) {
    if (!scope.workspaceId) return "Choose a project first";
    if (
      !skill.workspace_ids.includes("*") &&
      !skill.workspace_ids.includes(scope.workspaceId)
    )
      return "Workspace is not granted";
    if (
      !skill.profiles.includes("*") &&
      !skill.profiles.includes(scope.profile)
    )
      return "Permission profile is not granted";
  }
}
