import { createContext, useContext, type RefObject } from "react";

/** Shell actions that section sidebars and pages trigger without knowing the layout. */
export type Shell = {
  focusSearch: () => void;
  /** Hides the sidebar (docked) or closes it (overlay); focus moves to the top bar's toggle. */
  collapseSidebar: () => void;
  startNewSession: () => void;
  /** The sidebar's own hide button, so focus can land on it when the sidebar opens from its toggle. */
  collapseButtonRef: RefObject<HTMLButtonElement | null>;
  /** The toggle-sidebar chord as a label, e.g. "Ctrl+B", for tooltips. */
  sidebarShortcut: string;
};

export const ShellContext = createContext<Shell | null>(null);

export function useShell(): Shell {
  const shell = useContext(ShellContext);
  if (!shell) throw new Error("useShell must be used inside MainLayout");
  return shell;
}
