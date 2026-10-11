import { useEffect, useRef, useState } from "react";
import { CodeBlock, Composer, Dialog, Notice } from "@aidex/ui/components";
import { DockLayout } from "@aidex/ui/desktop";
import {
  BoxIcon,
  ChatIcon,
  FolderIcon,
  LayersIcon,
  StopIcon,
} from "@aidex/ui/icons";
import {
  Button,
  Card,
  IconButton,
  Select,
  Stack,
  Text,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type {
  ConversationMode,
  PermissionMode,
  SessionConfig,
} from "@aidex/client/dto/sessions";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";
import { useInferenceStatus } from "../../hooks/useInferenceStatus";
import { useSessionUpdates } from "../../hooks/useSessionUpdates";
import { LocalInferenceNotice } from "../model-manager/LocalInferenceNotice";
import { HistoryMessages, MessageContent } from "../sessions/HistoryMessages";
import { SkillSelection } from "../extensions/SkillSelection";
import { approvalDecision } from "../approvals/approvalDecision";
import { skillSelectionReason } from "../extensions/skillEligibility";
import {
  permissionOptions,
  supportsSelection,
} from "../agent-center/runtimeSelection";
import styles from "./ConversationPage.module.css";
export function ConversationPage({
  initialMode,
}: {
  initialMode: ConversationMode;
}) {
  const { route, replace, navigate } = useNavigation();
  const preferences = usePreferences();
  const models = useRpc("model.list");
  const inference = useInferenceStatus(models.refetch);
  const localCatalog = useRpc("inference.catalog", undefined, {
    enabled: inference.data?.state === "stopped",
  });
  const workspaces = useRpc("workspace.list");
  const skills = useRpc("extensions.skills.list");
  const runtimes = useRpc("runtime.list");
  const execution = useRpc("execution.status");
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const live = useSessionUpdates(route.id, session.refetch);
  const [draft, setDraft] = useState(() => preferences.pendingPrompt ?? "");
  const [mode, setMode] = useState(initialMode);
  const [runtimeId, setRuntimeId] = useState<string>(
    initialMode === "agent" ? (preferences.settings.runtime_id ?? "") : "",
  );
  const [runtimeModelId, setRuntimeModelId] = useState(
    preferences.settings.runtime_model_id ?? "",
  );
  const runtimeTouched = useRef(false);
  const external = !!runtimeId && runtimeId !== "native";
  const runtime = runtimes.data?.runtimes.find((item) => item.id === runtimeId);
  const descriptor = execution.data?.runtime_descriptors.find(
    (item) => item.id === runtimeId,
  );
  const runtimeModels = useRpc(
    "runtime.models",
    { id: runtimeId },
    { enabled: external && !!runtime?.enabled },
  );
  const [workspaceId, setWorkspaceId] = useState(
    () => preferences.pendingWorkspaceId ?? "",
  );
  const [permission, setPermission] = useState<PermissionMode>(
    preferences.settings.permission_mode ?? "supervised",
  );
  const [instructions, setInstructions] = useState(
    preferences.settings.instructions,
  );
  const [fallbackModel, setFallbackModel] = useState(
    preferences.settings.fallback_model ?? "",
  );
  const [maxSteps, setMaxSteps] = useState(preferences.settings.max_steps);
  const [maxTokens, setMaxTokens] = useState(preferences.settings.max_tokens);
  const [selectedSkillIds, setSelectedSkillIds] = useState(
    preferences.settings.skill_ids ?? [],
  );
  const settingsTouched = useRef(false);
  const [popup, setPopup] = useState<
    "mode" | "model" | "project" | "attach" | "skills" | null
  >(null);
  const [attachmentPath, setAttachmentPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const createdId = useRef<string | undefined>(undefined);
  const contextTouched = useRef(!!preferences.pendingWorkspaceId);
  const permissionTouched = useRef(false);
  const pendingCommand = useRef<{ content: string; id: string } | null>(null);
  const autoSent = useRef(false);
  const endRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [arrivalPrompt] = useState(() => preferences.pendingPrompt);
  const availableModels =
    models.data?.models.filter((m) => m.capabilities.chat) ?? [];
  const nativeSession =
    session.data &&
    (!session.data.runtime_id || session.data.runtime_id === "native")
      ? session.data
      : undefined;
  const currentRef = nativeSession
    ? `${nativeSession.provider_id}/${nativeSession.model_id}`
    : preferences.modelId;
  const selectedModel = availableModels.find(
    (m) => `${m.model.provider_id}/${m.model.model_id}` === currentRef,
  );
  const preferredLocal =
    currentRef?.startsWith("local-llama/") ||
    (!currentRef && models.data?.default_model?.provider_id === "local-llama");
  const installedModel =
    localCatalog.data?.models.find(
      (item) =>
        item.id ===
        (inference.data?.model_id ?? localCatalog.data?.settings?.model_id),
    ) ?? localCatalog.data?.models[0];
  const chosenModel = preferredLocal
    ? inference.data?.state === "ready"
      ? (selectedModel ??
        (!nativeSession
          ? availableModels.find(
              (item) => item.model.provider_id === "local-llama",
            )
          : undefined))
      : undefined
    : nativeSession
      ? selectedModel
      : (selectedModel ??
        availableModels.find(
          (m) =>
            m.model.provider_id === models.data?.default_model?.provider_id &&
            m.model.model_id === models.data?.default_model?.model_id,
        ) ??
        availableModels[0]);
  const model =
    chosenModel?.model.provider_id === "local-llama" &&
    inference.data?.state !== "ready"
      ? undefined
      : chosenModel;
  const project = workspaces.data?.workspaces.find((w) => w.id === workspaceId);
  const latestRun = session.data?.runs
    .filter((run) => !run.parent_run_id)
    .at(-1);
  const running =
    !!latestRun &&
    ["pending", "running", "waiting_approval"].includes(latestRun.status);
  const modelName = external
    ? (runtimeModels.data?.models.find((item) => item.id === runtimeModelId)
        ?.name ??
      (runtimeModelId || "Choose a runtime model"))
    : (model?.display_name ??
      inference.data?.model_name ??
      installedModel?.name ??
      "Choose a model");
  const lastMessage = session.data?.messages.at(-1);
  useEffect(() => {
    const container = endRef.current?.closest("main");
    if (!container) return;
    const track = () => {
      following.current =
        container.scrollHeight - container.scrollTop - container.clientHeight <
        180;
    };
    container.addEventListener("scroll", track, { passive: true });
    return () => container.removeEventListener("scroll", track);
  }, []);
  useEffect(() => {
    const container = endRef.current?.closest("main");
    if (container && following.current)
      container.scrollTop = container.scrollHeight;
  }, [session.data?.messages.length, lastMessage?.content]);
  useEffect(() => {
    preferences.setPendingPrompt(null);
    preferences.setPendingWorkspaceId(null);
  }, []);
  useEffect(() => {
    if (!route.id && preferences.loaded && !permissionTouched.current) {
      setPermission(preferences.settings.permission_mode ?? "supervised");
    }
  }, [route.id, preferences.loaded, preferences.settings.permission_mode]);
  useEffect(() => {
    if (
      !route.id &&
      preferences.loaded &&
      !runtimeTouched.current &&
      initialMode === "agent"
    ) {
      setRuntimeId(preferences.settings.runtime_id ?? "");
      setRuntimeModelId(preferences.settings.runtime_model_id ?? "");
    }
  }, [
    route.id,
    preferences.loaded,
    preferences.settings.runtime_id,
    preferences.settings.runtime_model_id,
    initialMode,
  ]);
  useEffect(() => {
    if (!route.id && preferences.loaded && !settingsTouched.current) {
      setInstructions(preferences.settings.instructions);
      setSelectedSkillIds(preferences.settings.skill_ids ?? []);
      setFallbackModel(preferences.settings.fallback_model ?? "");
      setMaxSteps(preferences.settings.max_steps);
      setMaxTokens(preferences.settings.max_tokens);
    }
  }, [
    route.id,
    preferences.loaded,
    preferences.settings.instructions,
    preferences.settings.skill_ids,
    preferences.settings.fallback_model,
    preferences.settings.max_steps,
    preferences.settings.max_tokens,
  ]);
  useEffect(() => {
    if (!route.id) return;
    const timer = setInterval(session.refetch, running ? 250 : 1500);
    return () => clearInterval(timer);
  }, [route.id, running, session.refetch]);
  useEffect(() => {
    if (!workspaces.data) return;
    if (session.data) {
      setMode(session.data.mode);
      setRuntimeId(
        session.data.runtime_id === "native"
          ? ""
          : (session.data.runtime_id ?? ""),
      );
      if (session.data.runtime_id && session.data.runtime_id !== "native")
        setRuntimeModelId(session.data.model_id);
      setPermission(session.data.permission_mode);
      setInstructions(session.data.instructions ?? "");
      setSelectedSkillIds(session.data.skill_ids ?? []);
      setFallbackModel(session.data.fallback_model ?? "");
      setMaxSteps(session.data.max_steps ?? 16);
      setMaxTokens(session.data.max_tokens ?? 4096);
      const selected = workspaces.data.workspaces.find(
        (w) =>
          w.id === session.data!.workspace_id ||
          w.name === session.data!.workspace_id,
      );
      setWorkspaceId(selected?.id ?? "");
    } else if (initialMode !== "chat" && !contextTouched.current) {
      const selected = workspaces.data.workspaces.find(
        (w) =>
          w.id === preferences.workspaceId ||
          w.name === preferences.workspaceId,
      );
      setWorkspaceId(selected?.id ?? "");
    }
  }, [
    session.data?.id,
    session.data?.workspace_id,
    session.data?.mode,
    session.data?.permission_mode,
    session.data?.runtime_id,
    session.data?.model_id,
    session.data?.instructions,
    JSON.stringify(session.data?.skill_ids ?? []),
    session.data?.fallback_model,
    session.data?.max_steps,
    session.data?.max_tokens,
    workspaces.data,
    preferences.workspaceId,
    initialMode,
  ]);
  useEffect(() => {
    if (
      preferences.loaded &&
      workspaces.data &&
      preferences.workspaceId &&
      !workspaces.data.workspaces.some(
        (w) =>
          w.id === preferences.workspaceId ||
          w.name === preferences.workspaceId,
      )
    )
      preferences.setWorkspaceId("");
  }, [preferences.loaded, preferences.workspaceId, workspaces.data]);
  const configuration = (
    nextMode = mode,
    context = workspaceId,
    target = model,
  ): SessionConfig => {
    if (route.id && !session.data)
      throw new Error("The conversation is still loading.");
    if (!external && !target)
      throw new Error("Connect a provider or start a local model first.");
    if (external && (!runtime?.enabled || !descriptor))
      throw new Error("Select an enabled runtime instance in Agents first.");
    if (external && !supportsSelection(descriptor, nextMode, permission))
      throw new Error(
        "The selected runtime does not support this mode or permission scope. Choose a supported mode and permission explicitly.",
      );
    if (external && !runtimeModelId.trim())
      throw new Error("Choose or enter an exact model ID for this runtime.");
    if (
      !Number.isInteger(maxSteps) ||
      maxSteps < 1 ||
      maxSteps > 128 ||
      !Number.isInteger(maxTokens) ||
      maxTokens < 1 ||
      maxTokens > 32768
    )
      throw new Error("Use 1–128 steps and 1–32768 output tokens.");
    if (nextMode !== "chat" && !context)
      throw new Error("Choose a project before starting Plan or Agent mode.");
    if (context && !workspaces.data?.workspaces.some((w) => w.id === context))
      throw new Error("Choose a registered project or No project context.");
    if (context && selectedSkillIds.length) {
      if (!skills.data)
        throw new Error(
          "Wait for the installed skill catalog before sending selected skills.",
        );
      for (const id of selectedSkillIds) {
        const skill = skills.data.find((item) => item.id === id);
        const reason = skill
          ? skillSelectionReason(skill, {
              workspaceId: context,
              profile: permission,
            })
          : "Skill is not installed";
        if (reason)
          throw new Error(
            `Selected skill ${id}: ${reason}. Remove it or select a granted context.`,
          );
      }
    }
    return {
      title: session.data?.title ?? draft.trim().slice(0, 72),
      workspace_id: context || null,
      mode: nextMode,
      provider_id: external ? "runtime" : target!.model.provider_id,
      model_id: external ? runtimeModelId.trim() : target!.model.model_id,
      permission_mode: permission,
      runtime_id: external ? runtimeId : null,
      instructions,
      skill_ids: context ? selectedSkillIds : [],
      fallback_model: external ? null : fallbackModel || null,
      max_steps: maxSteps,
      max_tokens: maxTokens,
    };
  };
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      session.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  const submit = async (text: string) => {
    if (busy || running || !text.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      const config = configuration();
      let content = text.trim();
      if (attachmentPath.trim()) {
        if (!workspaceId)
          throw new Error("Choose a project before attaching a project file.");
        content += `\n\nAttached project file ${attachmentPath.trim()}:\n${await readAttachment(workspaceId, attachmentPath.trim())}`;
      }
      let id = route.id ?? createdId.current;
      if (!id) {
        const created = await call("conversation.create", config);
        id = created.id;
        createdId.current = id;
      } else if (session.data) {
        await call("agent.configuration.set", {
          session_id: id,
          configuration: config,
        });
      }
      const command =
        pendingCommand.current?.content === content
          ? pendingCommand.current
          : { content, id: crypto.randomUUID() };
      pendingCommand.current = command;
      await call(mode === "agent" ? "agent.start" : "message.send", {
        session_id: id,
        content,
        command_id: command.id,
      });
      pendingCommand.current = null;
      setDraft("");
      setAttachmentPath("");
      replace({ page: mode === "agent" ? "agent" : "chat", id });
      session.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (
      !arrivalPrompt ||
      !preferences.loaded ||
      (!runtimeTouched.current &&
        initialMode === "agent" &&
        runtimeId !== (preferences.settings.runtime_id ?? "")) ||
      (!settingsTouched.current &&
        JSON.stringify(selectedSkillIds) !==
          JSON.stringify(preferences.settings.skill_ids ?? [])) ||
      (workspaceId && selectedSkillIds.length > 0 && !skills.data) ||
      autoSent.current ||
      route.id ||
      (!external && !model) ||
      (external &&
        (!runtimeModelId.trim() ||
          !supportsSelection(descriptor, mode, permission))) ||
      (workspaceId && !project) ||
      (mode !== "chat" && !workspaceId)
    )
      return;
    autoSent.current = true;
    void submit(arrivalPrompt);
  }, [
    arrivalPrompt,
    preferences.loaded,
    preferences.settings.runtime_id,
    preferences.settings.skill_ids,
    route.id,
    model?.model.provider_id,
    model?.model.model_id,
    runtimeId,
    runtimeModelId,
    descriptor,
    skills.data,
    permission,
    workspaceId,
    project,
  ]);
  const changeMode = async (value: ConversationMode) => {
    setError(undefined);
    setMode(value);
  };
  const chooseProject = (value: string) => {
    contextTouched.current = true;
    if (route.id)
      void act(async () => {
        await call("agent.configuration.set", {
          session_id: route.id!,
          configuration: configuration(mode, value),
        });
        setWorkspaceId(value);
        setAttachmentPath("");
      });
    else {
      setWorkspaceId(value);
      setAttachmentPath("");
    }
  };
  const pendingApprovals =
    session.data?.approvals.filter((a) => a.status === "pending") ?? [];
  const dock = (
    <Composer
      size="compact"
      autoFocus
      label="Message"
      value={draft}
      onValueChange={setDraft}
      onSubmit={
        external
          ? runtimeModelId.trim() &&
            supportsSelection(descriptor, mode, permission) &&
            workspaceId
            ? (text) => void submit(text)
            : undefined
          : model
            ? (text) => void submit(text)
            : undefined
      }
      disabled={busy || running}
      onAttach={() => setPopup("attach")}
      placeholder={mode === "chat" ? "Message AiDEX…" : "Describe the task…"}
      attachments={
        attachmentPath ? (
          <span className={styles.attachment}>
            {attachmentPath}
            <IconButton
              size="sm"
              label="Remove attachment"
              icon={<StopIcon />}
              onClick={() => setAttachmentPath("")}
            />
          </span>
        ) : undefined
      }
      toolbarStart={
        <>
          <IconButton
            size="sm"
            label={`Mode: ${mode}`}
            icon={<ChatIcon />}
            disabled={busy || running}
            onClick={() => setPopup("mode")}
          />
          <IconButton
            size="sm"
            label={`Model: ${modelName}`}
            icon={<BoxIcon />}
            disabled={busy || running}
            onClick={() => setPopup("model")}
          />
          <IconButton
            size="sm"
            label={project ? `Project: ${project.name}` : "No project context"}
            icon={<FolderIcon />}
            selected={!!project}
            disabled={busy || running}
            onClick={() => setPopup("project")}
          />
          <IconButton
            size="sm"
            label={`Selected skills: ${project ? selectedSkillIds.length : 0}`}
            icon={<LayersIcon />}
            selected={!!project && selectedSkillIds.length > 0}
            disabled={busy || running || !workspaceId}
            onClick={() => setPopup("skills")}
          />
          <span className={styles.summary}>
            {mode[0].toUpperCase() + mode.slice(1)} · {modelName}
            {external ? ` · ${runtimeId}` : ""}
            {project ? ` · ${project.name}` : ""}
            {project && selectedSkillIds.length
              ? ` · ${selectedSkillIds.length} skills`
              : ""}
          </span>
        </>
      }
      toolbarEnd={
        running ? (
          <IconButton
            size="sm"
            label="Stop generation"
            icon={<StopIcon />}
            onClick={() =>
              void act(() => call("run.cancel", { id: latestRun!.id }))
            }
          />
        ) : undefined
      }
    />
  );
  return (
    <DockLayout width="prose" dock={dock}>
      <div className={styles.conversation}>
        <header className={styles.header}>
          <h1 tabIndex={-1}>{session.data?.title || "New chat"}</h1>
          {route.id && (
            <IconButton
              size="sm"
              label="Execution details"
              icon={<BoxIcon />}
              onClick={() => navigate({ page: "execution", id: route.id })}
            />
          )}
        </header>
        {live.reconnecting && (
          <Notice tone="warning">
            Reconnecting live updates. Saved conversation history remains
            available.{live.error ? ` ${live.error}` : ""}
          </Notice>
        )}
        {(error ||
          session.error ||
          (external
            ? runtimes.error || execution.error
            : models.error || inference.error || localCatalog.error)) && (
          <Notice tone="danger" role="alert">
            {error ??
              session.error?.message ??
              (external
                ? (runtimes.error?.message ?? execution.error?.message)
                : (models.error?.message ??
                  inference.error?.message ??
                  localCatalog.error?.message))}
          </Notice>
        )}
        {!external && (
          <LocalInferenceNotice
            status={inference.data}
            onRefresh={inference.refetch}
          />
        )}
        {external && !supportsSelection(descriptor, mode, permission) && (
          <Notice tone="warning">
            Choose a supported runtime permission in Conversation mode. Full
            access allows this external agent to act as your current user.
          </Notice>
        )}
        {external && !runtimeModelId.trim() && (
          <Notice>
            Choose a runtime model from the model picker before sending a task.
          </Notice>
        )}
        {!external &&
          !inference.data &&
          !inference.error &&
          !availableModels.length && (
            <Notice role="status">
              Checking the local model on this drive…
            </Notice>
          )}
        {!external &&
          !models.loading &&
          !availableModels.length &&
          !!inference.data &&
          !localCatalog.loading &&
          (inference.data.state !== "stopped" || !!localCatalog.data) &&
          inference.data?.state !== "starting" &&
          inference.data?.state !== "failed" && (
            <Notice>
              <Text>
                {installedModel
                  ? `${installedModel.name} is installed on this drive. Start it in Models and accounts to use local chat.`
                  : "Choose a model to start chatting."}
              </Text>
              <Button
                variant="outline"
                onClick={() => navigate({ page: "models" })}
              >
                Models and accounts
              </Button>
            </Notice>
          )}
        {!session.data?.messages.length && !route.id && (
          <div className={styles.empty}>
            <ChatIcon size={28} />
            <h2>What would you like to talk about?</h2>
            <Text tone="muted">
              Chat about anything. Add a project only when you need its context.
            </Text>
          </div>
        )}
        <Stack gap={5} role="log" aria-label="Conversation">
          {session.data?.messages
            .filter(
              (m) =>
                m.role !== "system" &&
                (m.role !== "assistant" ||
                  !!m.content ||
                  !!m.tool_calls.length),
            )
            .map((message) => (
              <MessageContent
                key={message.original_index}
                message={message}
                scope={{ session_id: session.data!.id }}
              />
            ))}
        </Stack>
        {session.data?.messages_truncated && (
          <details>
            <summary>Earlier messages</summary>
            <HistoryMessages scope={{ session_id: session.data.id }} />
          </details>
        )}
        {latestRun?.error && (!session.data?.workspace_id || project) && (
          <Notice tone="danger">{latestRun.error}</Notice>
        )}
        {latestRun &&
          latestRun.status === "interrupted" &&
          (!external ||
            (descriptor?.supports_resume && latestRun.has_runtime_ref)) && (
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() =>
                void act(() => call("run.resume", { id: latestRun.id }))
              }
            >
              Resume task
            </Button>
          )}
        {pendingApprovals.map((approval) => (
          <Card key={approval.id} padding="md">
            <Stack gap={3}>
              <Text>Approval: {approval.tool_name}</Text>
              <CodeBlock
                language="json"
                code={JSON.stringify(approval.arguments, null, 2)}
              />
              {approval.preview != null && (
                <CodeBlock
                  title="Change preview"
                  language="json"
                  code={JSON.stringify(approval.preview, null, 2)}
                />
              )}
              {(approval.arguments_truncated || approval.preview_truncated) && (
                <Notice tone="warning">
                  Request a smaller action to review it completely.
                </Notice>
              )}
              <Stack direction="row" gap={2}>
                {[true, false].map((approved) => (
                  <Button
                    key={String(approved)}
                    variant={approved ? "primary" : "outline"}
                    disabled={
                      busy ||
                      (approved &&
                        (approval.arguments_truncated ||
                          approval.preview_truncated))
                    }
                    onClick={() =>
                      void act(() =>
                        call(
                          "approval.respond",
                          approvalDecision(
                            approval,
                            approved,
                            crypto.randomUUID(),
                          ),
                        ),
                      )
                    }
                  >
                    {approved ? "Approve once" : "Deny"}
                  </Button>
                ))}
              </Stack>
            </Stack>
          </Card>
        ))}
        {session.data?.approvals_truncated && (
          <Notice>
            Conversation approval history is bounded.{" "}
            <Button
              variant="ghost"
              onClick={() => navigate({ page: "security", tab: "approvals" })}
            >
              Open all pending broker requests
            </Button>
          </Notice>
        )}
        <div ref={endRef} />
      </div>
      <Dialog
        open={popup === "mode"}
        title="Conversation mode"
        onClose={() => setPopup(null)}
      >
        {error && <Notice tone="danger">{error}</Notice>}
        <Select
          label="Mode"
          disabled={busy || running}
          value={mode}
          options={[
            {
              value: "chat",
              label: "Chat · no agent tools",
              disabled: external && !descriptor?.modes.includes("chat"),
            },
            {
              value: "plan",
              label: "Plan · project context",
              disabled: external && !descriptor?.modes.includes("plan"),
            },
            {
              value: "agent",
              label: "Agent · coding tools",
              disabled: external
                ? !descriptor?.modes.includes("agent")
                : model?.capabilities.tool_calls !== "native",
            },
          ]}
          onChange={(event) =>
            void changeMode(event.target.value as ConversationMode)
          }
        />
        {mode === "agent" && (
          <Select
            label="Agent runtime"
            disabled={busy || running}
            value={runtimeId}
            options={[
              { value: "", label: "AiDEX Native · connected provider model" },
              ...(runtimes.data?.runtimes ?? []).map((item) => ({
                value: item.id,
                label: `${item.id} · ${item.driver}`,
                disabled: !item.enabled,
              })),
            ]}
            onChange={(event) => {
              runtimeTouched.current = true;
              const id = event.target.value;
              setRuntimeId(id);
              setRuntimeModelId(
                id === session.data?.runtime_id
                  ? session.data.model_id
                  : id === preferences.settings.runtime_id
                    ? (preferences.settings.runtime_model_id ?? "")
                    : "",
              );
            }}
          />
        )}
        {mode !== "chat" && (
          <Select
            label="Permissions"
            disabled={busy || running}
            value={permission}
            options={permissionOptions.map((option) => ({
              ...option,
              disabled:
                external &&
                !descriptor?.permission_modes.includes(
                  option.value === "auto" ? "supervised" : option.value,
                ),
            }))}
            onChange={(event) => {
              const value = event.target.value as PermissionMode;
              permissionTouched.current = true;
              setPermission(value);
            }}
          />
        )}
        <Text tone="muted">
          Plan and Agent require a project. These selections apply to the next
          message.
        </Text>
        <TextArea
          label="Instructions for this conversation"
          rows={4}
          maxLength={32768}
          value={instructions}
          onChange={(event) => {
            settingsTouched.current = true;
            setInstructions(event.target.value);
          }}
        />
        {!external && (
          <Select
            label="Fallback provider model"
            value={fallbackModel}
            options={[
              { value: "", label: "No fallback" },
              ...availableModels.map((item) => ({
                value: `${item.model.provider_id}/${item.model.model_id}`,
                label: item.display_name,
              })),
            ]}
            onChange={(event) => {
              settingsTouched.current = true;
              setFallbackModel(event.target.value);
            }}
          />
        )}
        <TextField
          label="Maximum steps"
          type="number"
          min={1}
          max={128}
          value={maxSteps}
          onChange={(event) => {
            settingsTouched.current = true;
            setMaxSteps(Number(event.target.value));
          }}
        />
        <TextField
          label="Maximum output tokens"
          type="number"
          min={1}
          max={32768}
          value={maxTokens}
          onChange={(event) => {
            settingsTouched.current = true;
            setMaxTokens(Number(event.target.value));
          }}
        />
        <Button
          variant="outline"
          disabled={busy || running}
          onClick={() => {
            if (route.id)
              void act(async () => {
                await call("agent.configuration.set", {
                  session_id: route.id!,
                  configuration: configuration(),
                });
                setPopup(null);
              });
            else setPopup(null);
          }}
        >
          Apply task settings
        </Button>
        <Button variant="ghost" onClick={() => navigate({ page: "agents" })}>
          Manage agent runtimes and defaults
        </Button>
      </Dialog>
      <Dialog
        open={popup === "model"}
        title="Choose a model"
        onClose={() => setPopup(null)}
      >
        {error && <Notice tone="danger">{error}</Notice>}
        {!external && (
          <LocalInferenceNotice
            status={inference.data}
            onRefresh={inference.refetch}
          />
        )}
        {external ? (
          <Stack gap={3}>
            <Text>
              {runtimeId} owns this model selection and its credentials.
            </Text>
            {runtimeModels.loading && (
              <Text tone="muted">Checking runtime model catalog…</Text>
            )}
            {runtimeModels.error && (
              <Notice>
                {runtimeModels.error.message} Enter an exact documented model ID
                if discovery is unavailable.
              </Notice>
            )}
            {!!runtimeModels.data?.models.length && (
              <Select
                label="Runtime model"
                disabled={busy || running}
                value={runtimeModelId}
                options={[
                  { value: "", label: "Choose a model" },
                  ...runtimeModels.data.models.map((item) => ({
                    value: item.id,
                    label: item.name,
                  })),
                ]}
                onChange={(event) => setRuntimeModelId(event.target.value)}
              />
            )}
            <TextField
              label="Exact runtime model ID"
              disabled={busy || running}
              value={runtimeModelId}
              onChange={(event) => setRuntimeModelId(event.target.value)}
              hint="Pi and OMP use provider/model. Provider model defaults remain separate."
            />
            <Button onClick={() => setPopup(null)}>
              Use this model for the task
            </Button>
          </Stack>
        ) : (
          <Select
            label="Model"
            disabled={busy || running}
            value={
              model ? `${model.model.provider_id}/${model.model.model_id}` : ""
            }
            options={availableModels.map((m) => ({
              value: `${m.model.provider_id}/${m.model.model_id}`,
              label: m.display_name,
            }))}
            onChange={(event) => {
              const selected = availableModels.find(
                (m) =>
                  `${m.model.provider_id}/${m.model.model_id}` ===
                  event.target.value,
              );
              if (!selected) return;
              preferences.setModelId(event.target.value);
              if (route.id)
                void act(() =>
                  call("agent.configuration.set", {
                    session_id: route.id!,
                    configuration: configuration(mode, workspaceId, selected),
                  }),
                );
              setPopup(null);
            }}
          />
        )}
        <Button
          variant="ghost"
          onClick={() => navigate({ page: external ? "agents" : "models" })}
        >
          {external
            ? "Manage runtime configuration"
            : "Manage models and accounts"}
        </Button>
      </Dialog>
      <Dialog
        open={popup === "skills"}
        title="Skills for this conversation"
        onClose={() => setPopup(null)}
      >
        <SkillSelection
          skills={skills.data ?? []}
          selectedIds={selectedSkillIds}
          onChange={(ids) => {
            settingsTouched.current = true;
            setSelectedSkillIds(ids);
          }}
          scope={{ workspaceId, profile: permission }}
          disabled={busy || running}
          loading={skills.loading}
          error={skills.error?.message}
        />
        <Button
          variant="outline"
          disabled={busy || running}
          onClick={() => {
            if (route.id)
              void act(async () => {
                await call("agent.configuration.set", {
                  session_id: route.id!,
                  configuration: configuration(),
                });
                setPopup(null);
              });
            else setPopup(null);
          }}
        >
          Apply selected skills
        </Button>
        <Button
          variant="ghost"
          onClick={() => navigate({ page: "extensions", tab: "skills" })}
        >
          Manage installed skills
        </Button>
      </Dialog>
      <Dialog
        open={popup === "project" || popup === "attach"}
        title={popup === "attach" ? "Attach project file" : "Project context"}
        onClose={() => setPopup(null)}
      >
        {error && <Notice tone="danger">{error}</Notice>}
        <Select
          label="Project"
          disabled={busy || running}
          value={workspaceId}
          options={[
            { value: "", label: "No project context" },
            ...(workspaces.data?.workspaces ?? []).map((w) => ({
              value: w.id,
              label: w.name,
              disabled: w.availability.state !== "available",
            })),
          ]}
          onChange={(event) => chooseProject(event.target.value)}
        />
        {popup === "attach" ? (
          <>
            <TextField
              label="Relative file path"
              value={attachmentPath}
              onChange={(event) => setAttachmentPath(event.target.value)}
              disabled={!workspaceId}
            />
            <Button
              disabled={!workspaceId || !attachmentPath.trim()}
              onClick={() => setPopup(null)}
            >
              Attach file
            </Button>
          </>
        ) : (
          <Text tone="muted">
            Ordinary chats don't need a project. Choose one to include project
            context.
          </Text>
        )}
        <Button
          variant="ghost"
          onClick={() => navigate({ page: "workspace-create" })}
        >
          Add a workspace
        </Button>
      </Dialog>
    </DockLayout>
  );
}
async function readAttachment(
  workspace_id: string,
  path: string,
): Promise<string> {
  const { callUntyped } = await import("@aidex/client/rpc");
  const file = (await callUntyped("workspace.files.read", {
    workspace_id,
    path,
  })) as {
    content: string | null;
    binary: boolean;
    truncated: boolean;
    lossy_utf8: boolean;
  };
  if (file.binary || file.content === null)
    throw new Error("Choose a text file.");
  if (file.truncated) throw new Error("Choose a smaller file or excerpt.");
  if (file.lossy_utf8)
    throw new Error("Convert the file to UTF-8 before attaching.");
  return file.content;
}
