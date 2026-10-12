import { useEffect, useState } from "react";
import { Dialog } from "@aidex/ui/components";
import { Button, Select, Stack, Text, TextField } from "@aidex/ui/primitives";
import type { Workspace } from "@aidex/client/dto/workspace";
import { normalizeProjectPath } from "./attachmentFiles";

export type ProjectFileDialogProps = {
  open: boolean;
  onClose: () => void;
  workspaces: readonly Workspace[];
  /** Preselected project: the conversation's own, when it has one. */
  defaultWorkspaceId: string;
  /** Adds the file to the composer; the daemon reads it through the project's confined root. */
  onAttach: (workspaceId: string, path: string, projectName: string) => void;
  onAddWorkspace: () => void;
};

/**
 * "From project…": picks a file of a registered project by its project-relative path. Choosing a
 * project here only says where the file is; it does not change the conversation's project.
 */
export function ProjectFileDialog({
  open,
  onClose,
  workspaces,
  defaultWorkspaceId,
  onAttach,
  onAddWorkspace,
}: ProjectFileDialogProps) {
  const available = workspaces.filter((workspace) => workspace.availability.state === "available");
  const [workspaceId, setWorkspaceId] = useState("");
  const [path, setPath] = useState("");
  useEffect(() => {
    if (!open) return;
    const preferred = available.find((workspace) => workspace.id === defaultWorkspaceId) ?? available[0];
    setWorkspaceId(preferred?.id ?? "");
    setPath("");
  }, [open]);
  const project = available.find((workspace) => workspace.id === workspaceId);
  const relative = normalizeProjectPath(path);
  const attach = () => {
    if (!project || !relative) return;
    onAttach(project.id, relative, project.name);
    onClose();
  };
  return (
    <Dialog open={open} title="Attach a project file" onClose={onClose}>
      <Stack gap={3}>
        {available.length === 0 ? (
          <Text tone="muted">Add a workspace to attach files from a project.</Text>
        ) : (
          <>
            <Select
              label="Project"
              value={workspaceId}
              options={workspaces.map((workspace) => ({
                value: workspace.id,
                label: workspace.name,
                disabled: workspace.availability.state !== "available",
              }))}
              onChange={(event) => setWorkspaceId(event.target.value)}
            />
            <TextField
              label="File path in the project"
              placeholder="docs/notes.md"
              hint="An image, PDF, or text file. The conversation's project does not change."
              value={path}
              onChange={(event) => setPath(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  attach();
                }
              }}
            />
            <Button disabled={!project || !relative} onClick={attach}>
              Attach file
            </Button>
          </>
        )}
        <Button variant="ghost" onClick={onAddWorkspace}>
          Add a workspace
        </Button>
      </Stack>
    </Dialog>
  );
}
