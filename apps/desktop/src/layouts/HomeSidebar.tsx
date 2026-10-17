import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { ChatIcon, FolderIcon, PlusCircleIcon } from "@aidex/ui/icons";
import { Text } from "@aidex/ui/primitives";
import { useNavigation } from "../app/providers/navigation";
import { usePreferences } from "../app/providers/preferences";
import { useRpc } from "../hooks/useRpc";
import { SectionSidebar } from "./SectionSidebar";
import { useShell } from "./shell";

export function HomeSidebar() {
  const { navigate } = useNavigation();
  const preferences = usePreferences();
  const shell = useShell();
  const workspaces = useRpc("workspace.list");
  const sessions = useRpc("conversation.list");
  return (
    <SectionSidebar
      title="AiDEX"
      action={{
        label: "New chat",
        icon: <PlusCircleIcon />,
        onClick: shell.startNewSession,
      }}
    >
      <SidebarSection title="Projects">
        {workspaces.data?.workspaces.slice(0, 7).map((workspace) => (
          <SidebarItem
            key={workspace.id}
            label={workspace.name}
            icon={<FolderIcon />}
            active={preferences.workspaceId === workspace.id}
            onSelect={() => {
              preferences.setWorkspaceId(workspace.id);
              navigate({ page: "workspace", id: workspace.id });
            }}
          />
        ))}
        {!workspaces.loading && !workspaces.data?.workspaces.length && (
          <Text tone="muted">No projects added.</Text>
        )}
        <SidebarItem
          label="Add workspace"
          icon={<PlusCircleIcon />}
          onSelect={() => navigate({ page: "workspace-create" })}
        />
      </SidebarSection>
      <SidebarSection title="Recent chats">
        {sessions.data?.sessions
          .filter((s) => !s.locked)
          .slice(0, 7)
          .map((session) => (
            <SidebarItem
              key={session.id}
              label={session.title || "Untitled chat"}
              icon={<ChatIcon />}
              onSelect={() =>
                navigate({
                  page: session.mode === "agent" ? "agent" : "chat",
                  id: session.id,
                })
              }
            />
          ))}
      </SidebarSection>
    </SectionSidebar>
  );
}
