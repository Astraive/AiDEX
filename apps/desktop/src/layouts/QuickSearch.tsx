import { useState, type Ref } from "react";
import { CommandPalette, Notice, type CommandMenuItem, type CommandMenuSection } from "@aidex/ui/components";
import { BotIcon, ChatIcon, FolderIcon } from "@aidex/ui/icons";
import { useRpc } from "../hooks/useRpc";
import { useNavigation } from "../app/providers/navigation";

type Destination = { page: "chat" | "agent"; id: string } | { page: "workspace"; id: string };

export function QuickSearch({
  open,
  onClose,
  inputRef,
}: {
  open: boolean;
  onClose: () => void;
  inputRef: Ref<HTMLInputElement>;
}) {
  const [query, setQuery] = useState("");
  const { navigate } = useNavigation();
  const sessions = useRpc("conversation.list", undefined, { enabled: open });
  const workspaces = useRpc("workspace.list", undefined, { enabled: open });
  const needle = query.trim().toLowerCase();
  const matchingSessions = (sessions.data?.sessions ?? [])
    .filter(
      (session) =>
        !session.locked &&
        (!needle ||
          `${session.title ?? ""} ${session.mode ?? ""}`
            .toLowerCase()
            .includes(needle)),
    )
    .slice(0, 8);
  const matchingProjects = (workspaces.data?.workspaces ?? [])
    .filter(
      (workspace) =>
        !needle ||
        `${workspace.name} ${workspace.path}`.toLowerCase().includes(needle),
    )
    .slice(0, 8);

  // Where each result leads, by its row id.
  const destinations = new Map<string, Destination>();
  const sections: CommandMenuSection[] = [
    {
      id: "conversations",
      label: "Conversations",
      items: matchingSessions.map((session): CommandMenuItem => {
        const agent = session.mode === "agent";
        const id = `session:${session.id}`;
        destinations.set(id, { page: agent ? "agent" : "chat", id: session.id });
        return {
          id,
          title: session.title || "Untitled chat",
          icon: agent ? <BotIcon /> : <ChatIcon />,
          badge: session.mode ?? "chat",
        };
      }),
    },
    {
      id: "projects",
      label: "Projects",
      items: matchingProjects.map((workspace): CommandMenuItem => {
        const id = `workspace:${workspace.id}`;
        destinations.set(id, { page: "workspace", id: workspace.id });
        const { availability } = workspace;
        const unavailable = availability.state !== "available";
        return {
          id,
          title: workspace.name,
          description: workspace.path,
          icon: <FolderIcon />,
          badge: unavailable ? { label: "Unavailable", tone: "warning" } : undefined,
          // The reason is not shown in the row; a cut-off path shows in the tooltip by itself.
          tooltip:
            availability.state === "unavailable"
              ? `${workspace.path} — ${availability.reason}`
              : undefined,
        };
      }),
    },
  ];

  const close = () => {
    onClose();
    setQuery("");
  };

  return (
    <CommandPalette
      open={open}
      onClose={close}
      label="Search"
      inputLabel="Search conversations and projects"
      placeholder="Conversation title or project name…"
      inputRef={inputRef}
      query={query}
      onQueryChange={setQuery}
      sections={sections}
      loading={sessions.loading || workspaces.loading}
      notice={
        sessions.error || workspaces.error ? (
          <Notice tone="danger">
            {sessions.error?.message ?? workspaces.error?.message}
          </Notice>
        ) : undefined
      }
      onSelect={(item) => {
        const destination = destinations.get(item.id);
        if (!destination) return;
        close();
        navigate(destination);
      }}
    />
  );
}
