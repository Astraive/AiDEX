import { useRef } from "react";
import { EntityCard } from "@aidex/ui/components";
import { CheckCircleIcon } from "@aidex/ui/icons";
import { Badge, Button, IconBadge, Stack } from "@aidex/ui/primitives";
import { statusLabel, type SampleAgent } from "./sample";

type Props = {
  agent: SampleAgent;
  isDefault: boolean;
  onConfigure: () => void;
  onSetDefault: () => void;
};

export function BuiltInAgentCard({ agent, isDefault, onConfigure, onSetDefault }: Props) {
  // "Set as default" disappears once pressed, so focus stays in this card on its Configure button.
  const configureRef = useRef<HTMLButtonElement>(null);
  const setDefault = () => {
    onSetDefault();
    requestAnimationFrame(() => configureRef.current?.focus());
  };

  return (
    <EntityCard
      media={<IconBadge icon={<agent.Icon />} tone={isDefault ? "accent" : "neutral"} />}
      title={agent.name}
      subtitle={agent.runtime}
      badges={
        <Stack direction="row" gap={2} wrap>
          {isDefault ? <Badge icon={<CheckCircleIcon />}>Default</Badge> : null}
          <Badge tone="success">{statusLabel[agent.status]}</Badge>
        </Stack>
      }
      description={agent.description}
      footer={
        <Stack direction="row" gap={2} wrap>
          <Button ref={configureRef} variant="outline" size="md" aria-label={`Configure ${agent.name}`} onClick={onConfigure}>
            Configure
          </Button>
          {isDefault ? null : (
            <Button variant="ghost" size="md" aria-label={`Set ${agent.name} as default`} onClick={setDefault}>
              Set as default
            </Button>
          )}
        </Stack>
      }
    />
  );
}
