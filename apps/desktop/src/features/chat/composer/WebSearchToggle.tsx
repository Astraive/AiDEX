import { GlobeIcon } from "@aidex/ui/icons";
import { IconButton } from "@aidex/ui/primitives";
import type { ConversationMode } from "@aidex/client/dto/sessions";
import type { WebSearchAvailability } from "./useWebSearchToggle";

type Props = {
  availability: WebSearchAvailability;
  mode: ConversationMode;
  on: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
};

/**
 * The composer's globe button. In Chat it switches web search for this conversation; Plan and Agent
 * always offer web tools while web search is on in Extensions → Web.
 */
export function WebSearchToggle({ availability, mode, on, onChange, disabled = false }: Props) {
  if (!availability.available)
    return (
      <IconButton
        size="sm"
        label={availability.reason ?? "Set up web search in Extensions → Web"}
        icon={<GlobeIcon />}
        disabled
      />
    );
  if (mode !== "chat")
    return (
      <IconButton
        size="sm"
        label="Web search is on in Plan and Agent modes"
        icon={<GlobeIcon />}
        selected
        disabled
      />
    );
  return (
    <IconButton
      size="sm"
      label="Web search"
      icon={<GlobeIcon />}
      selected={on}
      aria-pressed={on}
      disabled={disabled}
      onClick={() => onChange(!on)}
    />
  );
}
