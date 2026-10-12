import {
  BoxIcon,
  ChatIcon,
  ClockIcon,
  CloudIcon,
  FolderIcon,
  GridIcon,
  HomeIcon,
  SettingsIcon,
  ShieldIcon,
  SmartphoneIcon,
  UsersIcon,
  type IconProps,
} from "@aidex/ui/icons";
import type { ComponentType } from "react";
import { tabOf, type Route } from "./routes";

export const railItems = [
  { id: "home", label: "Home", Icon: HomeIcon, route: { page: "home" } },
  { id: "workspaces", label: "Workspaces", Icon: FolderIcon, route: { page: "workspaces" } },
  { id: "chat", label: "Chat", Icon: ChatIcon, route: { page: "chat" } },
  { id: "history", label: "History", Icon: ClockIcon, route: { page: "history" } },
  { id: "remote", label: "Remote access", Icon: SmartphoneIcon, route: { page: "remote" } },
  { id: "extensions", label: "Plugins", Icon: GridIcon, route: { page: "extensions" } },
  { id: "agents", label: "Agents", Icon: UsersIcon, route: { page: "agents" } },
  { id: "models", label: "Local models", Icon: BoxIcon, route: { page: "models", tab: "local" } },
  { id: "providers", label: "Providers", Icon: CloudIcon, route: { page: "models", tab: "providers" } },
  { id: "security", label: "Security", Icon: ShieldIcon, route: { page: "security" } },
  { id: "settings", label: "Settings", Icon: SettingsIcon, route: { page: "settings" } },
] as const satisfies readonly { id: string; label: string; Icon: ComponentType<IconProps>; route: Route }[];

export type RailId = (typeof railItems)[number]["id"];

/** Which rail destination a route belongs to, so drill-in pages keep their section highlighted. */
export function railIdFor(route: Route): RailId {
  switch (route.page) {
    case "workspace-create":
    case "workspace":
      return "workspaces";
    case "agent":
    case "execution":
    case "review":
    case "agent-config":
      return "agents";
    case "checkpoints":
      return "history";
    case "model":
      return "models";
    case "models":
      return tabOf(route, "models") === "providers" ? "providers" : "models";
    default:
      return route.page;
  }
}
