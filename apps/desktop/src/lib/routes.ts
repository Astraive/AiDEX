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
 * Pages whose content is still sample data because their backend has not landed (workspaces #11,
 * sessions #12, models #14/#15, agents #19/#23, extensions #24, remote #29). The shell labels them.
 * Home, Settings, and Security mix live Core data with labelled samples and say so themselves.
 */
export const samplePages: ReadonlySet<PageId> = new Set<PageId>();

/** `tab` selects a section within a page; `id` names the record a page shows (workspace, agent, model). */
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
  models: ["local", "providers"],
  extensions: ["skills", "mcp", "plugins"],
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

/** Pages whose `id` is an in-page selection (a file, a checkpoint, a filter) rather than the record shown. */
const selectionPages: ReadonlySet<PageId> = new Set([
  "review",
  "checkpoints",
  "history",
  "remote",
  "extensions",
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
