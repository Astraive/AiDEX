/** Every screen in the desktop workspace shell. Launch screens (unlock, detection) sit outside it. */
export type PageId =
  | "home"
  | "workspaces"
  | "workspace-create"
  | "workspace"
  | "chat"
  | "agent"
  | "execution"
  | "review"
  | "history"
  | "checkpoints"
  | "agents"
  | "agent-config"
  | "models"
  | "model"
  | "extensions"
  | "security"
  | "remote"
  | "settings";

/**
 * Pages whose whole content is sample data because their backend has not landed; the shell labels
 * them with a "Sample data" badge. None is left. What is not live yet says so where it appears:
 * Settings → Drive labels its host cleanup report as a sample (#28), actions without a backend show
 * a "Preview" toast, and Remote access (#29) says it is coming soon. Review changes shows the live
 * Checkpoints page until AiDEX Core exposes workspace diffs.
 */
export const samplePages: ReadonlySet<PageId> = new Set<PageId>();

/** `tab` selects a section within a page; `id` names the record a page shows (workspace, conversation, agent, model). */
export type Route = { page: PageId; tab?: string; id?: string };

export const pageTitles: Record<PageId, string> = {
  home: "Home",
  workspaces: "Workspaces",
  "workspace-create": "Add workspace",
  workspace: "Workspace",
  chat: "Chat",
  agent: "Agent",
  execution: "Execution",
  review: "Review changes",
  history: "Session history",
  checkpoints: "Checkpoints",
  agents: "Agents",
  "agent-config": "Agent configuration",
  models: "Models",
  model: "Model details",
  extensions: "Plugins",
  security: "Security",
  remote: "Remote access",
  settings: "Settings",
};

/** Tabs available on pages that have sections, in display order. The first is the default. */
export const pageTabs = {
  workspace: ["overview", "files", "git", "environment", "sessions", "memory"],
  execution: ["execution", "graph"],
  models: ["providers", "local"],
  extensions: ["skills", "mcp", "plugins", "web"],
  security: ["overview", "vault", "sandbox", "approvals"],
  remote: ["devices", "sessions"],
  settings: ["appearance", "shortcuts", "updates", "diagnostics", "drive"],
} as const satisfies Partial<Record<PageId, readonly string[]>>;

export type TabbedPage = keyof typeof pageTabs;
export type PageTab<Page extends TabbedPage> = (typeof pageTabs)[Page][number];

/** Reads a route's tab for a tabbed page, falling back to the first tab when absent or unknown. */
export function tabOf<Page extends TabbedPage>(
  route: Route,
  page: Page,
): PageTab<Page> {
  const tabs: readonly string[] = pageTabs[page];
  return (
    route.tab && tabs.includes(route.tab) ? route.tab : tabs[0]
  ) as PageTab<Page>;
}

/** Pages whose `id` is an in-page selection (the conversation picked on Review changes or Checkpoints, a provider, a skill, MCP server, or plugin) rather than the record shown. */
const selectionPages: ReadonlySet<PageId> = new Set([
  "review",
  "checkpoints",
  "history",
  "remote",
  "extensions",
  "models",
]);

/**
 * Identity of the screen a route shows. Changing it remounts the page, resets scroll, and moves focus
 * to the new title; tab switches and in-page selections keep the same identity.
 */
export function screenKeyOf(route: Route): string {
  return selectionPages.has(route.page)
    ? route.page
    : `${route.page}:${route.id ?? ""}`;
}
