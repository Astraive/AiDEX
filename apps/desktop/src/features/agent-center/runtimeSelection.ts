import type {
  ConversationMode,
  PermissionMode,
} from "@aidex/client/dto/sessions";
import type { RuntimeDescriptor } from "@aidex/client/dto/runtimes";
export const permissionOptions: { value: PermissionMode; label: string }[] = [
  { value: "supervised", label: "Supervised" },
  { value: "auto_accept_edits", label: "Auto-accept edits" },
  { value: "auto", label: "Auto · supervised review" },
  { value: "full_access", label: "Full access" },
];
export function supportsSelection(
  descriptor: RuntimeDescriptor | undefined,
  mode: ConversationMode,
  permission: PermissionMode,
): boolean {
  return (
    !!descriptor &&
    descriptor.modes.includes(mode) &&
    descriptor.permission_modes.includes(
      permission === "auto" ? "supervised" : permission,
    )
  );
}
