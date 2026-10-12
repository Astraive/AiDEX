import { useEffect } from "react";
import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { ChatIcon, ClockIcon, LockIcon, PlusIcon } from "@aidex/ui/icons";
import { Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import type { PageId } from "../../lib/routes";

export function LiveSessionSidebar({
  agent = false,
  title,
  page,
}: {
  agent?: boolean;
  title?: string;
  page?: PageId;
}) {
  const { route, navigate } = useNavigation();
  const { setPendingPrompt, setPendingAttachments } = usePreferences();
  const sessions = useRpc("conversation.list");
  useEffect(() => {
    const timer = setInterval(sessions.refetch, 2000);
    return () => clearInterval(timer);
  }, [sessions.refetch]);
  return (
    <SectionSidebar title={title ?? (agent ? "Agents" : "Conversations")}>
      <SidebarSection title="Start">
        <SidebarItem
          label={agent ? "New agent task" : "New chat"}
          icon={<PlusIcon />}
          onSelect={() => {
            setPendingPrompt(null);
            setPendingAttachments(null);
            navigate({ page: agent ? "agent" : "chat" });
          }}
        />
        <SidebarItem
          label="History"
          icon={<ClockIcon />}
          active={route.page === "history"}
          onSelect={() => navigate({ page: "history" })}
        />
      </SidebarSection>
      <SidebarSection title="Recent sessions">
        {(sessions.data?.sessions ?? []).map((session) => (
          <SidebarItem
            key={session.id}
            label={
              session.title ||
              (session.locked ? "Locked conversation" : "Untitled conversation")
            }
            icon={session.locked ? <LockIcon /> : <ChatIcon />}
            active={route.id === session.id}
            onSelect={() =>
              navigate({
                page: page ?? (agent ? "agent" : "chat"),
                id: session.id,
              })
            }
          />
        ))}
        {sessions.error && <Text tone="muted">{sessions.error.message}</Text>}
        {!sessions.loading && !sessions.data?.sessions.length && (
          <Text tone="muted">No saved sessions yet.</Text>
        )}
      </SidebarSection>
    </SectionSidebar>
  );
}
