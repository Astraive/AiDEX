import { useEffect, useRef, useState } from "react";
import { Dialog, Notice } from "@aidex/ui/components";
import { Button, ChoiceCardGroup, Select, Stack, Text, TextArea } from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import type { Workspace } from "@aidex/client/dto/workspace";
import type { ConversationModelState } from "../resolveConversationModel";
import {
  carriedAttachmentWarning,
  currentModelHint,
  defaultContinuation,
  type AgentContinuation,
  type AgentModelOption,
} from "./agentContinuation";
import { permissionChoices } from "./permissions";

export type ContinueAsAgentDialogProps = {
  open: boolean;
  onClose: () => void;
  workspaces: readonly Workspace[];
  /** Preferred projects in order: the conversation's own, then the default for new chats. */
  workspaceIds: readonly (string | null | undefined)[];
  /** Models an agent can run on (`agentModelOptions`). */
  models: readonly AgentModelOption[];
  /**
   * Preselected model: the conversation's own, kept when it can use tools; otherwise one must be
   * picked.
   */
  modelKeys: readonly (string | null | undefined)[];
  /** Name of the conversation's model, to say why it is not offered. */
  currentModelName: string;
  /** Whether the conversation's model can run now (an unlisted one is unavailable, not tool-less). */
  currentModelState: ConversationModelState;
  /** Images and PDFs the conversation's messages carry. */
  carried: { images: number; pdfs: number };
  /** Attachments waiting in the composer (they stay there). */
  composerAttachments: number;
  /** Commands outside Full access are refused on this computer. */
  shellNeedsFullAccess: boolean;
  onAddWorkspace: () => void;
  /** Switches this conversation to Agent, then starts the instruction when there is one. */
  onContinueHere: (choice: AgentContinuation) => Promise<void>;
  /** Forks this conversation into a new Agent chat, opens it, and starts the instruction. */
  onContinueInNewChat: (choice: AgentContinuation) => Promise<void>;
};

/**
 * "Continue as Agent" for a Chat or Plan conversation with history: a project, a permission mode
 * (described as AiDEX applies it), a model with native tool calling, and an optional first
 * instruction. "Continue here" (focused first) switches this conversation; "Continue in a new Agent
 * chat" copies the conversation into a new one and leaves this one as it is.
 */
export function ContinueAsAgentDialog({
  open,
  onClose,
  workspaces,
  workspaceIds,
  models,
  modelKeys,
  currentModelName,
  currentModelState,
  carried,
  composerAttachments,
  shellNeedsFullAccess,
  onAddWorkspace,
  onContinueHere,
  onContinueInNewChat,
}: ContinueAsAgentDialogProps) {
  const [choice, setChoice] = useState<AgentContinuation>(() =>
    defaultContinuation({ workspaces, workspaceIds, models, modelKeys }),
  );
  const [pending, setPending] = useState<"here" | "new" | null>(null);
  const [error, setError] = useState<string>();
  const [focusRequest, setFocusRequest] = useState(0);
  const hereRef = useRef<HTMLButtonElement>(null);
  const projectRef = useRef<HTMLSelectElement>(null);
  const modelRef = useRef<HTMLSelectElement>(null);
  const available = workspaces.filter((workspace) => workspace.availability.state === "available");
  // Each opening starts from the conversation as it is now.
  useEffect(() => {
    if (!open) return;
    setChoice(defaultContinuation({ workspaces, workspaceIds, models, modelKeys }));
    setError(undefined);
    setPending(null);
    setFocusRequest((request) => request + 1);
  }, [open]);
  // Once those choices have rendered (so "Continue here" is enabled when they are complete), focus
  // it, or the first choice still missing. The Dialog has already opened by then.
  useEffect(() => {
    if (!focusRequest) return;
    if (!choice.workspaceId) projectRef.current?.focus();
    else if (!choice.modelKey) modelRef.current?.focus();
    else hereRef.current?.focus();
  }, [focusRequest]);
  const update = (patch: Partial<AgentContinuation>) => setChoice((current) => ({ ...current, ...patch }));
  const model = models.find((item) => item.key === choice.modelKey);
  const ready = !!available.find((workspace) => workspace.id === choice.workspaceId) && !!model;
  const warning = carriedAttachmentWarning(model, carried);
  const currentOffered = modelKeys[0] ? models.some((item) => item.key === modelKeys[0]) : true;
  const run = async (target: "here" | "new") => {
    if (!ready || pending) return;
    setPending(target);
    setError(undefined);
    try {
      await (target === "here" ? onContinueHere : onContinueInNewChat)(choice);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setPending(null);
    }
  };
  return (
    <Dialog
      open={open}
      title="Continue as Agent"
      description="The agent sees this conversation's messages and attachments."
      onClose={() => {
        if (!pending) onClose();
      }}
    >
      <Stack gap={4}>
        {available.length === 0 ? (
          <Notice tone="info">Add a workspace so the agent has a project to work in.</Notice>
        ) : (
          <Select
            ref={projectRef}
            label="Project"
            value={choice.workspaceId}
            disabled={!!pending}
            hint="The agent reads and changes files in this project."
            options={[
              { value: "", label: "Choose a project", disabled: true },
              ...workspaces.map((workspace) => ({
                value: workspace.id,
                label: workspace.name,
                disabled: workspace.availability.state !== "available",
              })),
            ]}
            onChange={(event) => update({ workspaceId: event.target.value })}
          />
        )}
        <Stack direction="row">
          <Button variant="ghost" disabled={!!pending} onClick={onAddWorkspace}>
            Add a workspace
          </Button>
        </Stack>
        <ChoiceCardGroup<PermissionMode>
          label="Permission"
          variant="rows"
          options={permissionChoices({ shellNeedsFullAccess }).map((option) => ({
            ...option,
            disabled: !!pending,
          }))}
          value={choice.permission}
          onValueChange={(permission) => update({ permission })}
        />
        {models.length === 0 ? (
          <Notice tone="warning">
            No connected model can use agent tools. Connect a provider with a tool-calling model first.
          </Notice>
        ) : (
          <Select
            ref={modelRef}
            label="Model"
            value={choice.modelKey}
            disabled={!!pending}
            hint={currentModelHint({ name: currentModelName, state: currentModelState, offered: currentOffered })}
            options={[
              { value: "", label: "Choose a model that can use tools", disabled: true },
              ...models.map((item) => ({ value: item.key, label: item.label })),
            ]}
            onChange={(event) => update({ modelKey: event.target.value })}
          />
        )}
        {warning && <Notice tone="warning">{warning}</Notice>}
        <TextArea
          label="First instruction (optional)"
          rows={3}
          value={choice.instruction}
          disabled={!!pending}
          placeholder="What should the agent do first?"
          hint="Leave it empty to switch now and write the task in the composer."
          onChange={(event) => update({ instruction: event.target.value })}
        />
        {composerAttachments > 0 && (
          <Text tone="muted">Files waiting in the composer stay in this conversation's composer.</Text>
        )}
        {error && (
          <Notice tone="danger" role="alert">
            {error}
          </Notice>
        )}
        <Stack direction="row" gap={2} wrap>
          <Button ref={hereRef} variant="primary" disabled={!ready || !!pending} onClick={() => void run("here")}>
            {pending === "here" ? "Continuing…" : "Continue here"}
          </Button>
          <Button variant="outline" disabled={!ready || !!pending} onClick={() => void run("new")}>
            {pending === "new" ? "Creating…" : "Continue in a new Agent chat"}
          </Button>
        </Stack>
      </Stack>
    </Dialog>
  );
}
