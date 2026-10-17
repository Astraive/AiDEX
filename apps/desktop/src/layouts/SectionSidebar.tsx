import type { ReactNode } from "react";
import { Brand } from "@aidex/ui/components";
import { Sidebar } from "@aidex/ui/desktop";
import { PanelLeftIcon, SearchIcon } from "@aidex/ui/icons";
import { Button, IconButton, Stack } from "@aidex/ui/primitives";
import { useShell } from "./shell";

type Props = {
  /** Section name shown beside the logo, e.g. "Workspaces". */
  title: string;
  /** Primary action button under the header, e.g. "New workspace". */
  action?: { label: string; icon: ReactNode; onClick: () => void; trailing?: ReactNode; keyShortcuts?: string };
  /** Extra fixed content under the action, e.g. a workspace picker. */
  headerExtra?: ReactNode;
  /** `SidebarSection`s for this section's contextual navigation. */
  children: ReactNode;
  brandSize?: "md" | "lg";
};

/** The contextual sidebar frame every section uses: brand title, search, collapse, and an optional primary action. */
export function SectionSidebar({ title, action, headerExtra, children, brandSize = "md" }: Props) {
  const shell = useShell();

  const header = (
    <>
      <Stack direction="row" align="center" justify="between" gap={1}>
        <Brand name={title} size={brandSize} />
        <Stack direction="row" gap={1}>
          <IconButton size="sm" label="Jump to search" icon={<SearchIcon />} onClick={shell.focusSearch} />
          <IconButton
            ref={shell.collapseButtonRef}
            size="sm"
            label="Hide sidebar"
            icon={<PanelLeftIcon />}
            shortcut={shell.sidebarShortcut}
            onClick={shell.collapseSidebar}
          />
        </Stack>
      </Stack>
      {action ? (
        <Button
          variant="raised"
          size="md"
          fullWidth
          icon={action.icon}
          trailing={action.trailing}
          aria-keyshortcuts={action.keyShortcuts}
          onClick={action.onClick}
        >
          {action.label}
        </Button>
      ) : null}
      {headerExtra}
    </>
  );

  return (
    <Sidebar label={`${title} navigation`} header={header}>
      {children}
    </Sidebar>
  );
}
