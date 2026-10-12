import { useNavigation } from "../../app/providers/navigation";
import { useToast } from "../../app/providers/toast";
import type { SampleAgent } from "./sample";

/** Navigation and preview actions shared by the agent catalog and its sidebar. */
export function useAgentActions() {
  const { route, navigate, replace } = useNavigation();
  const notify = useToast();

  return {
    // Switching between agents' settings replaces the entry, so Back still returns to the catalog.
    configure: (agent: SampleAgent) =>
      (route.page === "agent-config" ? replace : navigate)({ page: "agent-config", id: agent.id }),
    install: (agent: SampleAgent) =>
      notify(`Preview: installing ${agent.name} will be available when AiDEX Core is connected.`),
    createAgent: () => notify("Preview: creating custom agents will be available when AiDEX Core is connected."),
    addRuntime: () => notify("Preview: adding a custom runtime will be available when AiDEX Core is connected."),
  };
}
