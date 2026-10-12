import { List, ListRow } from "@aidex/ui/components";
import { PlusIcon } from "@aidex/ui/icons";
import { Badge, Button, IconBadge } from "@aidex/ui/primitives";
import { statusLabel, type SampleAgent } from "./sample";
import { useAgentActions } from "./useAgentActions";

type Props = { runtimes: readonly SampleAgent[] };

/** Optional third-party agent runtimes: configure installed ones, install the rest, or add a custom one. */
export function ExternalRuntimeList({ runtimes }: Props) {
  const actions = useAgentActions();

  return (
    <List metaWidth="140px">
      {runtimes.map((runtime) => {
        const installed = runtime.status !== "not-installed";
        return (
          <ListRow
            key={runtime.id}
            media={<IconBadge size="md" icon={<runtime.Icon />} />}
            title={runtime.name}
            subtitle={`${runtime.description} ${runtime.runtime}.`}
            meta={[<Badge key="status" tone={installed ? "success" : "neutral"}>{statusLabel[runtime.status]}</Badge>]}
            trailing={
              installed ? (
                <Button variant="outline" size="md" aria-label={`Configure ${runtime.name}`} onClick={() => actions.configure(runtime)}>
                  Configure
                </Button>
              ) : (
                <Button variant="outline" size="md" aria-label={`Install ${runtime.name}`} onClick={() => actions.install(runtime)}>
                  Install
                </Button>
              )
            }
          />
        );
      })}
      <ListRow
        media={<IconBadge size="md" icon={<PlusIcon />} />}
        title="Add custom runtime"
        subtitle="Connect another CLI agent through the AiDEX adapter."
        onSelect={actions.addRuntime}
      />
    </List>
  );
}
