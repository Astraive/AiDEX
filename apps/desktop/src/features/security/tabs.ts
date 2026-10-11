import type { ComponentType } from "react";
import {
  ClipboardCheckIcon,
  KeyIcon,
  LockIcon,
  ShieldIcon,
  type IconProps,
} from "@aidex/ui/icons";
import type { PageTab } from "../../lib/routes";

export type SecurityTab = PageTab<"security">;

/** Security sections in display order; the page tabs and the sidebar both read this. */
export const securityTabs: readonly {
  value: SecurityTab;
  label: string;
  Icon: ComponentType<IconProps>;
}[] = [
  { value: "overview", label: "Overview", Icon: ShieldIcon },
  { value: "vault", label: "Vault and secrets", Icon: KeyIcon },
  { value: "sandbox", label: "Sandbox and policies", Icon: LockIcon },
  { value: "approvals", label: "Approvals", Icon: ClipboardCheckIcon },
];
