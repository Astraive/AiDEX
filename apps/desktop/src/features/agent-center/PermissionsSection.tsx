import { useId } from "react";
import { SectionHeader, SettingRow } from "@aidex/ui/components";
import { Stack, Switch } from "@aidex/ui/primitives";
import { permissionRows } from "./agentConfig";
import type { AgentPermissions } from "./sample";

type Props = {
  permissions: AgentPermissions;
  onChange: (permissions: AgentPermissions) => void;
};

export function PermissionsSection({ permissions, onChange }: Props) {
  const idPrefix = useId();
  const enabledCount = permissionRows.filter((row) => permissions[row.key]).length;

  return (
    <section>
      <Stack gap={2}>
        <SectionHeader title="Permissions" meta={`${enabledCount} of ${permissionRows.length} allowed`} />
        <div>
          {permissionRows.map((row) => {
            const controlId = `${idPrefix}-${row.key}`;
            return (
              <SettingRow
                key={row.key}
                title={row.title}
                description={row.description}
                controlId={controlId}
                control={
                  <Switch
                    id={controlId}
                    aria-describedby={`${controlId}-description`}
                    checked={permissions[row.key]}
                    onCheckedChange={(checked) => onChange({ ...permissions, [row.key]: checked })}
                  />
                }
              />
            );
          })}
        </div>
      </Stack>
    </section>
  );
}
