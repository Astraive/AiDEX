import { Select, Stack, TextField } from "@aidex/ui/primitives";
import { useRpc } from "../../hooks/useRpc";
export const splitGrants = (value: string) => [
  ...new Set(
    value
      .split(",")
      .map((item) => item.trim())
      .filter(Boolean),
  ),
];
export function ScopeFields({
  workspaceIds,
  profiles,
  onWorkspaces,
  onProfiles,
}: {
  workspaceIds: string;
  profiles: string;
  onWorkspaces: (value: string) => void;
  onProfiles: (value: string) => void;
}) {
  const workspaces = useRpc("workspace.list");
  return (
    <Stack gap={3}>
      <Select
        label="Add a workspace grant"
        value=""
        options={[
          { value: "", label: "Choose a registered workspace" },
          ...(workspaces.data?.workspaces ?? []).map((workspace) => ({
            value: workspace.id,
            label: workspace.name,
          })),
        ]}
        onChange={(event) => {
          if (event.target.value)
            onWorkspaces(
              [...splitGrants(workspaceIds), event.target.value]
                .filter((value, index, array) => array.indexOf(value) === index)
                .join(", "),
            );
        }}
      />
      <TextField
        label="Allowed workspace IDs"
        value={workspaceIds}
        onChange={(event) => onWorkspaces(event.target.value)}
        hint="Comma separated IDs. Empty grants deny access. * explicitly grants every workspace."
      />
      <TextField
        label="Allowed permission profiles"
        value={profiles}
        onChange={(event) => onProfiles(event.target.value)}
        hint="Comma separated profiles, such as supervised or full_access. Empty grants deny access; * explicitly grants every profile."
      />
    </Stack>
  );
}
