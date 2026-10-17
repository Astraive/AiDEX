import { useRef, useState } from "react";
import { Dialog, List, ListRow, Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { TrashIcon } from "@aidex/ui/icons";
import { Button, SearchField, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { Workspace } from "@aidex/client/dto/workspace";
import { useRpc } from "../../hooks/useRpc";
import { firstControl, useRemovalFocus } from "../../hooks/useRemovalFocus";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";

export function WorkspacesPage() {
  const workspaces = useRpc("workspace.list");
  const { navigate } = useNavigation();
  const { setWorkspaceId } = usePreferences();
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string>();
  // The remove confirmation; a failed remove is reported inside it.
  const [removing, setRemoving] = useState<Workspace>();
  const [removeBusy, setRemoveBusy] = useState(false);
  const [removeError, setRemoveError] = useState<string>();
  const items = (workspaces.data?.workspaces ?? []).filter((workspace) =>
    `${workspace.name} ${workspace.path}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  // After a remove, focus moves to the row that took its place, else to the page title.
  const titleRef = useRef<HTMLHeadingElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const removalFocus = useRemovalFocus(
    items.map((workspace) => workspace.id),
    (index) => firstControl(listRef.current?.children[index]),
    () => titleRef.current,
  );
  const remove = async (workspace: Workspace) => {
    setRemoveBusy(true);
    setRemoveError(undefined);
    try {
      await call("workspace.remove", { id: workspace.id, delete_data: false });
      removalFocus.removed(workspace.id);
      setRemoving(undefined);
      workspaces.refetch();
    } catch (caught) {
      setRemoveError(toAidexError(caught).message);
    } finally {
      setRemoveBusy(false);
    }
  };
  const closeRemove = () => {
    if (removeBusy) return;
    setRemoving(undefined);
    setRemoveError(undefined);
  };
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          titleRef={titleRef}
          title="Workspaces"
          description="Registered projects on the drive and this computer."
          actions={
            <Button
              variant="primary"
              onClick={() => navigate({ page: "workspace-create" })}
            >
              Add workspace
            </Button>
          }
        />
        <SearchField
          label="Search workspaces"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        {(error || workspaces.error) && (
          <Notice tone="danger">{error ?? workspaces.error?.message}</Notice>
        )}
        <List ref={listRef}>
          {items.map((workspace) => (
            <ListRow
              key={workspace.id}
              title={workspace.name}
              subtitle={workspace.path}
              meta={[workspace.kind.type, workspace.availability.state]}
              onSelect={() => {
                setWorkspaceId(workspace.id);
                void call("workspace.select", { id_or_name: workspace.id })
                  .then(() => navigate({ page: "workspace", id: workspace.id }))
                  .catch((caught) => setError(toAidexError(caught).message));
              }}
              trailing={
                <Button
                  variant="ghost"
                  aria-haspopup="dialog"
                  onClick={() => {
                    setRemoveError(undefined);
                    setRemoving(workspace);
                  }}
                >
                  Remove registration
                </Button>
              }
            />
          ))}
        </List>
        {!workspaces.loading && !items.length && (
          <Text tone="muted">
            No workspaces found. Add a project to start an agent task.
          </Text>
        )}
      </Stack>
      <Dialog
        open={removing !== undefined}
        title={removing ? `Remove ${removing.name} from AiDEX?` : "Remove this workspace?"}
        description="AiDEX forgets this workspace. Project files are kept."
        onClose={closeRemove}
        returnFocus={removalFocus.returnFocus}
      >
        <Stack gap={4}>
          {removeError && (
            <Notice tone="danger" role="alert">
              {removeError}
            </Notice>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={removeBusy} onClick={closeRemove}>
              Cancel
            </Button>
            <Button
              variant="primary"
              icon={<TrashIcon />}
              disabled={removeBusy || !removing}
              focusableWhenDisabled={removeBusy}
              onClick={() => removing && void remove(removing)}
            >
              {removeBusy ? "Removing…" : "Remove registration"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </PageFrame>
  );
}
