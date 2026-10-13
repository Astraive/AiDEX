import { useEffect, useRef, useState } from "react";
import {
  AttachmentTray,
  Composer,
  Dialog,
  FileDropZone,
  Notice,
  TranscriptDivider,
} from "@aidex/ui/components";
import { DockLayout } from "@aidex/ui/desktop";
import {
  ArrowRightLeftIcon,
  BotIcon,
  BoxIcon,
  ChatIcon,
  FolderIcon,
  GitBranchIcon,
  LayersIcon,
  SlidersIcon,
  StopIcon,
} from "@aidex/ui/icons";
import {
  Button,
  IconButton,
  Select,
  Stack,
  Text,
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
import { useToast } from "../../app/providers/toast";
import { useRpc } from "../../hooks/useRpc";
import { useShell } from "../../layouts/shell";
import { useProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import {
  isExternalRuntime,
  modelTargetOf,
  parseModelKey,
  type ModelTarget,
} from "../model-manager/catalog/modelKeys";
import { LOCAL_PROVIDER_ID } from "../model-manager/catalog/providerAccounts";
import { LocalInferenceNotice } from "../model-manager/LocalInferenceNotice";
import { ApprovalRequestCard } from "../approvals/ApprovalRequestCard";
import { approvalDecision } from "../approvals/approvalDecision";
import { HistoryMessages } from "../sessions/HistoryMessages";
import { ArtifactsDialog } from "./artifacts/ArtifactsDialog";
import { useArtifactDownload } from "./artifacts/useArtifactDownload";
import { attachmentSendBlock, reasonOf } from "./attachments/attachmentFiles";
import { ProjectFileDialog } from "./attachments/ProjectFileDialog";
import {
  trayItemOf,
  useConversationAttachments,
} from "./attachments/useConversationAttachments";
import { invokedSkillIds, invokedSkillsProblem } from "./composer/composerText";
import {
  agentModelOptions,
  carriedAttachments,
  draftWithInstruction,
  forkTitle,
  type AgentContinuation,
} from "./continuation/agentContinuation";
import { ContinueAsAgentDialog } from "./continuation/ContinueAsAgentDialog";
import {
  useRuntimeDescriptor,
  useShellNeedsFullAccess,
} from "./continuation/engineCapabilities";
import { PERMISSION_LABELS, permissionDescription } from "./continuation/permissions";
import { transcriptDividers } from "./continuation/transcriptDividers";
import type { SlashCommandId } from "./composer/slashCommands";
import { useComposerMenu, type SlashCommandState } from "./composer/useComposerMenu";
import { useWebSearchToggle } from "./composer/useWebSearchToggle";
import { WebSearchToggle } from "./composer/WebSearchToggle";
import { Transcript, type TranscriptMarker } from "./transcript/Transcript";
import {
  TranscriptProvider,
  type ArtifactRef,
  type TranscriptEnvironment,
} from "./transcript/TranscriptContext";
import { useOpenLink } from "./transcript/useOpenLink";
import { useConversationExtensions } from "./useConversationExtensions";
import { canResume } from "./canResume";
import { ConversationModelPicker } from "./ConversationModelPicker";
import { ConversationSettingsDialog } from "./ConversationSettingsDialog";
import { describeCurrentModel } from "./currentModel";
import { ModelStateNotice } from "./ModelStateNotice";
import {
  resolveConversationModel,
  type ResolvedConversationModel,
} from "./resolveConversationModel";
import {
  buildSessionConfig,
  configurationChanged,
  sessionTargetOf,
} from "./sessionConfig";
import { fallbackModelOptions, type SessionSettings } from "./sessionSettings";
import styles from "./ConversationPage.module.css";
const permissionOptions = (
  ["supervised", "auto_accept_edits", "auto", "full_access"] as const
).map((value) => ({ value, label: PERMISSION_LABELS[value] }));
const modeLabels: Record<ConversationMode, string> = {
  chat: "Chat",
  plan: "Plan",
  agent: "Agent",
};
/** Why external runtimes cannot take attachments (the attach button's tooltip and drop overlay). */
const EXTERNAL_ATTACH_REASON = "Attachments work with AiDEX native models";
const loadingSession: ResolvedConversationModel = {
  key: null,
  state: "none",
  origin: "session",
};
export function ConversationPage({
  initialMode,
}: {
  initialMode: ConversationMode;
}) {
  const { route, replace, navigate } = useNavigation();
  const preferences = usePreferences();
  const catalog = useProviderCatalog();
  const notify = useToast();
  const inference = catalog.inference;
  const workspaces = useRpc("workspace.list");
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const [draft, setDraft] = useState(() => preferences.pendingPrompt ?? "");
  const [mode, setMode] = useState(initialMode);
  const [workspaceId, setWorkspaceId] = useState(
    () => preferences.pendingWorkspaceId ?? "",
  );
  const [permission, setPermission] = useState<PermissionMode>(
    preferences.settings.permission_mode ?? "supervised",
  );
  // A new chat's own model choice; existing conversations use their configuration.
  const [draftKey, setDraftKey] = useState<string | null>(null);
  const [popup, setPopup] = useState<
    "mode" | "project" | "attach" | "continue" | "settings" | null
  >(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const createdId = useRef<string | undefined>(undefined);
  // What `conversation.create` saved, so a retry after a failed first send can re-apply changes.
  const createdConfig = useRef<SessionConfig | null>(null);
  const contextTouched = useRef(!!preferences.pendingWorkspaceId);
  const permissionTouched = useRef(false);
  // A retried send reuses its command id only when content, skills, web search, and attachments are
  // unchanged.
  const pendingCommand = useRef<{ request: string; id: string } | null>(null);
  // Continue as Agent: a retried fork or first instruction reuses its command id while unchanged.
  const continuationCommand = useRef<{ request: string; id: string } | null>(
    null,
  );
  const autoSent = useRef(false);
  // False once this page is gone (the user opened another screen): a send still finishing then must
  // not take over the route or touch the page's state.
  const mounted = useRef(true);
  // Where focus goes once "Continue here" closes its dialog (the header action that opened it goes
  // away when the conversation becomes an agent).
  const focusAfterContinue = useRef<"composer" | "heading" | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [arrivalPrompt] = useState(() => preferences.pendingPrompt);
  const [arrivalAttachments] = useState(
    () => (preferences.pendingAttachments?.length ?? 0) > 0,
  );
  const shell = useShell();
  const composerRef = useRef<HTMLFormElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const webSearch = useWebSearchToggle(route.id);
  const attachments = useConversationAttachments(
    route.id,
    preferences.pendingAttachments,
  );
  const [artifactDialog, setArtifactDialog] = useState<{ artifact: ArtifactRef | null } | null>(null);
  const openLink = useOpenLink();
  const downloadArtifact = useArtifactDownload();

  const resolved =
    route.id && !session.data
      ? loadingSession
      : resolveConversationModel({
          session: session.data,
          draftKey,
          preferenceKey: preferences.modelId,
          registryDefault: catalog.registryDefault,
          models: catalog.chatModels,
          inference,
          runtimes: catalog.runtimes,
        });
  const current = describeCurrentModel(resolved, catalog);
  const modelName = current.name;
  const target = parseModelKey(resolved.key);
  // Installed local models, to name the conversation's own local model while it is not running.
  const localCatalog = useRpc("inference.catalog", undefined, {
    enabled:
      inference?.state === "stopped" || resolved.state === "local-stopped",
  });
  const ownLocalModelId =
    target?.kind === "native" && target.providerId === LOCAL_PROVIDER_ID
      ? target.modelId
      : null;
  const ownLocalName = ownLocalModelId
    ? (localCatalog.data?.models.find((item) => item.id === ownLocalModelId)
        ?.name ?? modelName)
    : undefined;
  const runtimeDriver =
    resolved.runtime &&
    catalog.runtimeDrivers.find(
      (driver) => driver.driver === resolved.runtime?.driver,
    );
  const installedModel =
    localCatalog.data?.models.find(
      (item) =>
        item.id === (inference?.model_id ?? localCatalog.data?.settings?.model_id),
    ) ?? localCatalog.data?.models[0];
  const project = workspaces.data?.workspaces.find((w) => w.id === workspaceId);
  const latestRun = session.data?.runs
    .filter((run) => !run.parent_run_id)
    .at(-1);
  const running =
    !!latestRun &&
    ["pending", "running", "waiting_approval"].includes(latestRun.status);
  const lastMessage = session.data?.messages.at(-1);
  const external = target?.kind === "runtime";
  // "Resume task" needs the engine's word that an external runtime can resume; it is asked only
  // once such a run is interrupted.
  const runtimeDescriptor = useRuntimeDescriptor(
    target?.kind === "runtime" ? target.runtimeId : null,
    external && latestRun?.status === "interrupted",
  );
  const resumable = canResume(latestRun, external, runtimeDescriptor);
  const attachmentBlock = attachmentSendBlock({
    items: attachments.items,
    external,
    model: resolved.model
      ? {
          name: modelName,
          input: resolved.model.input,
          vision: resolved.model.capabilities.vision,
        }
      : undefined,
  });
  const extensions = useConversationExtensions({
    sessionId: route.id,
    workspaceId: project?.id,
    messages: session.data?.messages,
  });
  // "Continue as Agent" (header action, `/agent`, and Agent in the mode dialog) for a Chat or Plan
  // conversation with history on a native model.
  const continueAvailable =
    !!route.id &&
    !!session.data &&
    session.data.total_messages > 0 &&
    session.data.mode !== "agent" &&
    !external;
  const continuing = popup === "continue";
  const shellNeedsFullAccess = useShellNeedsFullAccess(
    continuing || popup === "mode",
  );
  // Sent attachments of the whole conversation (the transcript may show only its tail).
  const sessionAttachments = useRpc(
    "attachment.list",
    { session_id: route.id ?? "", limit: 200 },
    { enabled: continuing && !!route.id },
  );
  const agentModels = continuing
    ? agentModelOptions({
        models: catalog.models,
        findModel: catalog.findModel,
        providerName: (key) => catalog.providerFor(key)?.name,
        current: { key: resolved.key, name: modelName, descriptor: resolved.model },
      })
    : [];
  const forkedFrom = session.data?.forked_from;
  const conversations = useRpc("conversation.list", undefined, {
    enabled: !!forkedFrom,
  });
  const projectName = (id: string) =>
    workspaces.data?.workspaces.find((w) => w.id === id || w.name === id)?.name;
  const markers: TranscriptMarker[] = session.data
    ? transcriptDividers({
        runs: session.data.runs,
        session: session.data,
        projectName,
        sourceTitle: forkedFrom
          ? conversations.data?.sessions.find(
              (item) => item.id === forkedFrom.session_id,
            )?.title
          : undefined,
      }).map((divider) => ({
        key: divider.key,
        index: divider.index,
        node: (
          <TranscriptDivider
            label={divider.label}
            details={divider.details}
            icon={
              divider.kind === "fork" ? <GitBranchIcon /> : <ArrowRightLeftIcon />
            }
          />
        ),
      }))
    : [];
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
    preferences.setPendingAttachments(null);
  }, []);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    const target = focusAfterContinue.current;
    if (continuing || !target) return;
    focusAfterContinue.current = null;
    // After the dialog has handed focus back to its opener, which may be gone or disabled.
    const frame = requestAnimationFrame(() => {
      const field = textareaRef.current;
      const composerReady = target === "composer" && !!field && !field.disabled;
      // Without scrolling: a heading focus would jump to the top and stop following the run.
      (composerReady ? field : headingRef.current)?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [continuing]);
  useEffect(() => {
    if (!route.id && preferences.loaded && !permissionTouched.current) {
      setPermission(preferences.settings.permission_mode ?? "supervised");
    }
  }, [route.id, preferences.loaded, preferences.settings.permission_mode]);
  // External runtimes run only as agents with full access, whether a new chat's model came from the
  // picker, the saved default, or a Home handoff. Declared after the permission sync so it wins.
  useEffect(() => {
    if (route.id || target?.kind !== "runtime") return;
    setMode("agent");
    permissionTouched.current = true;
    setPermission("full_access");
  }, [route.id, resolved.key]);
  useEffect(() => {
    if (!route.id) return;
    const timer = setInterval(session.refetch, running ? 250 : 1500);
    return () => clearInterval(timer);
  }, [route.id, running, session.refetch]);
  useEffect(() => {
    if (!workspaces.data) return;
    if (session.data) {
      setMode(session.data.mode);
      setPermission(session.data.permission_mode);
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

  /** The model a configuration write uses: the conversation's own, else the new chat's choice. */
  const currentTarget = (): ModelTarget => {
    if (session.data) return sessionTargetOf(session.data);
    if (resolved.state === "ready" && target) return modelTargetOf(target);
    throw new Error(
      resolved.state === "none"
        ? "Choose a model first."
        : `${modelName} is not available right now. Choose another model.`,
    );
  };
  const configuration = (
    changes: {
      mode?: ConversationMode;
      workspaceId?: string;
      target?: ModelTarget;
      permission?: PermissionMode;
      /** New instructions, fallback, or limits (Conversation settings). */
      settings?: SessionSettings;
    } = {},
  ): SessionConfig => {
    if (route.id && !session.data)
      throw new Error("The conversation is still loading.");
    const nextTarget = changes.target ?? currentTarget();
    // Every external runtime accepts only agent mode with full access, so no write can carry
    // another combination (the engine would refuse it on the next run).
    const external = isExternalRuntime(nextTarget.runtime_id);
    return buildSessionConfig({
      session: session.data,
      defaults: preferences.settings,
      // A message of attachments only is titled by their names.
      title: (
        draft.trim() || attachments.items.map((item) => item.name).join(", ")
      ).slice(0, 72),
      mode: external ? "agent" : (changes.mode ?? mode),
      workspaceId: changes.workspaceId ?? workspaceId,
      isRegisteredWorkspace: (id) =>
        !!workspaces.data?.workspaces.some((w) => w.id === id),
      target: nextTarget,
      permission: external ? "full_access" : (changes.permission ?? permission),
      overrides: changes.settings,
    });
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
  /** Adds picked, pasted, or dropped files; refusals show in the conversation's notice. */
  const addFiles = (files: File[]) => {
    void attachments.draft.add(files).then((problems) => {
      if (problems.length) setError(problems.join(" "));
    });
  };
  /** A paste of 32 KiB or more becomes `pasted-text.txt` (Ctrl/Cmd+Shift+V keeps it inline). */
  const foldPaste = (text: string) => {
    const result = attachments.draft.addPaste(text);
    if ("problem" in result) setError(result.problem);
    else notify(`Large paste attached as ${result.name}`);
  };
  /**
   * Sends `text`. A composer send also carries the attachments and the `$skill` mentions, and clears
   * the draft; `mode` switches the conversation first (Implement switches a plan to Agent).
   */
  const submit = async (
    text: string,
    { composer = true, mode: nextMode }: { composer?: boolean; mode?: ConversationMode } = {},
  ) => {
    const attaching = composer && attachments.items.length > 0;
    // `sending`: a send this page started before it was remounted is still in flight.
    if (busy || running || attachments.sending || (!text.trim() && !attaching)) return;
    setBusy(true);
    setError(undefined);
    try {
      if (resolved.state !== "ready")
        throw new Error(
          resolved.state === "none"
            ? "Choose a model first."
            : resolved.state === "loading-local"
              ? `${modelName} is still loading. Send when it is ready.`
              : `${modelName} is not available right now. Choose another model.`,
        );
      if (attaching && attachmentBlock) throw new Error(attachmentBlock);
      const config = configuration(nextMode ? { mode: nextMode } : {});
      const native = !isExternalRuntime(config.runtime_id);
      if (attaching && !native)
        throw new Error(`${EXTERNAL_ATTACH_REASON}. Remove them to send.`);
      const content = text.trim();
      // External runtimes find their own skills and tools; they get the text unchanged.
      const skills =
        composer && native ? invokedSkillIds(content, extensions.skills, composerMenu.picked) : [];
      const skillsProblem = invokedSkillsProblem(skills);
      if (skillsProblem) throw new Error(skillsProblem);
      const searchWeb =
        native && config.mode === "chat" && webSearch.on && extensions.web.available;
      let id = route.id ?? createdId.current;
      if (!id) {
        const created = await call("conversation.create", config);
        id = created.id;
        createdId.current = id;
        createdConfig.current = config;
        webSearch.adopt(id);
        attachments.adopt(id);
      } else if (!route.id) {
        // Created earlier, but the first send failed: apply any model, mode, or project change since.
        if (
          !createdConfig.current ||
          configurationChanged(createdConfig.current, config)
        ) {
          await call("agent.configuration.set", {
            session_id: id,
            configuration: config,
          });
          createdConfig.current = config;
        }
      } else if (
        session.data &&
        (session.data.workspace_id !== config.workspace_id ||
          session.data.mode !== config.mode)
      ) {
        await call("agent.configuration.set", {
          session_id: id,
          configuration: config,
        });
      }
      // Uploads anything not yet uploaded (a new chat's attachments wait for its conversation).
      const attachmentIds = attaching
        ? await attachments.draft.uploadAll(id)
        : [];
      const request = JSON.stringify({
        content,
        skills,
        searchWeb,
        attachments: attachmentIds,
      });
      const command =
        pendingCommand.current?.request === request
          ? pendingCommand.current
          : { request, id: crypto.randomUUID() };
      pendingCommand.current = command;
      await call(config.mode === "agent" ? "agent.start" : "message.send", {
        session_id: id,
        content,
        command_id: command.id,
        ...(skills.length ? { skills } : {}),
        ...(searchWeb ? { web_search: true } : {}),
        ...(attachmentIds.length ? { attachments: attachmentIds } : {}),
      });
      pendingCommand.current = null;
      // The draft outlives this page (it is kept per conversation), so it is cleared either way.
      if (composer) attachments.draft.markSent(attachmentIds);
      if (!mounted.current) {
        // The user moved on while the attachments uploaded: say so rather than pull them back.
        notify(config.title ? `Message sent to '${config.title}'.` : "Message sent.");
        return;
      }
      if (nextMode) setMode(nextMode);
      if (composer) {
        setDraft("");
        composerMenu.clearPicked();
      }
      replace({ page: config.mode === "agent" ? "agent" : "chat", id });
      session.refetch();
    } catch (caught) {
      // The attachments this send carried can be removed again (the page may be gone by now).
      if (attaching) attachments.draft.release();
      const failure = toAidexError(caught);
      // Staged copies are deleted after a day: upload those again instead of failing for good.
      const problem =
        attaching &&
        reasonOf(failure.data) === "attachment_unavailable" &&
        (await attachments.draft.revalidate())
          ? "Some attachments had expired and are uploading again. Send again once they are ready."
          : failure.message;
      if (mounted.current) setError(problem);
      else notify(`The message was not sent: ${problem}`, "danger");
    } finally {
      setBusy(false);
    }
  };
  // A prompt (and attachments) handed over from Home sends itself once an explicit or default model
  // is ready; it never falls through to an arbitrary listed model. Attachments the model cannot take
  // stay in the composer, which says why.
  useEffect(() => {
    if (
      (!arrivalPrompt && !arrivalAttachments) ||
      autoSent.current ||
      route.id ||
      !preferences.loaded ||
      resolved.state !== "ready" ||
      (workspaceId && !project) ||
      // A runtime target is an agent even before its mode sync has rendered.
      ((mode !== "chat" || target?.kind === "runtime") && !workspaceId)
    )
      return;
    autoSent.current = true;
    if (arrivalAttachments && attachmentBlock) return;
    void submit(arrivalPrompt ?? "");
  }, [
    arrivalPrompt,
    arrivalAttachments,
    attachmentBlock,
    route.id,
    preferences.loaded,
    resolved.key,
    resolved.state,
    workspaceId,
    project,
  ]);
  const selectModel = async (key: string) => {
    const parsed = parseModelKey(key);
    if (!parsed) return;
    // Re-picking the same runtime still repairs a mode or permission it cannot run with.
    if (
      key === resolved.key &&
      !(
        parsed.kind === "runtime" &&
        (mode !== "agent" || permission !== "full_access")
      )
    )
      return;
    const nextTarget = modelTargetOf(parsed);
    const descriptor = catalog.findModel(key);
    const name =
      catalog.models.find((model) => model.key === key)?.name ??
      descriptor?.display_name ??
      parsed.modelId;
    let nextMode = mode;
    let nextPermission = permission;
    let switchedToChat = false;
    if (parsed.kind === "runtime") {
      // External runtimes run as agents with full access inside a project.
      if (!workspaceId) {
        setError("Choose a project before starting Plan or Agent mode.");
        return;
      }
      nextMode = "agent";
      nextPermission = "full_access";
    } else {
      if (target?.kind === "runtime")
        nextPermission = preferences.settings.permission_mode ?? "supervised";
      if (mode === "agent" && descriptor?.capabilities.tool_calls !== "native") {
        nextMode = "chat";
        switchedToChat = true;
      }
    }
    setError(undefined);
    if (route.id) {
      // A per-conversation choice; the default for new chats stays as it is.
      setBusy(true);
      try {
        await call("agent.configuration.set", {
          session_id: route.id,
          configuration: configuration({
            mode: nextMode,
            target: nextTarget,
            permission: nextPermission,
          }),
        });
        setMode(nextMode);
        setPermission(nextPermission);
        session.refetch();
      } catch (caught) {
        setError(toAidexError(caught).message);
        return;
      } finally {
        setBusy(false);
      }
    } else {
      // A new chat keeps the choice as its draft and as the default for the next new chat.
      setDraftKey(key);
      preferences.setModelId(key);
      setMode(nextMode);
      if (nextPermission !== permission) {
        permissionTouched.current = true;
        setPermission(nextPermission);
      }
    }
    if (switchedToChat)
      notify(`Switched to Chat: ${name} cannot use agent tools.`);
  };
  const changeMode = async (value: ConversationMode) => {
    // A conversation with history continues as an agent through the dialog (project, permission,
    // tool-capable model).
    if (value === "agent" && continueAvailable) {
      setPopup("continue");
      return;
    }
    setBusy(true);
    setError(undefined);
    try {
      const config = configuration({ mode: value });
      if (route.id)
        await call("agent.configuration.set", {
          session_id: route.id,
          configuration: config,
        });
      setMode(value);
      setPopup(null);
      session.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  const chooseProject = (value: string) => {
    contextTouched.current = true;
    if (route.id)
      void act(async () => {
        await call("agent.configuration.set", {
          session_id: route.id!,
          configuration: configuration({ workspaceId: value }),
        });
        setWorkspaceId(value);
      });
    else setWorkspaceId(value);
  };
  /** Conversation settings: one `agent.configuration.set` with the new values; throws for the dialog. */
  const saveSettings = async (settings: SessionSettings) => {
    const id = route.id;
    if (!id) return;
    await call("agent.configuration.set", {
      session_id: id,
      configuration: configuration({ settings }),
    });
    session.refetch();
  };
  /** The command id of a continuation step: reused while the same request is retried. */
  const continuationCommandId = (request: string) => {
    const command =
      continuationCommand.current?.request === request
        ? continuationCommand.current
        : { request, id: crypto.randomUUID() };
    continuationCommand.current = command;
    return command.id;
  };
  /** The full Agent configuration a "Continue as Agent" choice describes. */
  const agentConfiguration = (choice: AgentContinuation): SessionConfig => {
    const parsed = parseModelKey(choice.modelKey);
    if (parsed?.kind !== "native")
      throw new Error("Choose a model that can use agent tools.");
    return configuration({
      mode: "agent",
      workspaceId: choice.workspaceId,
      permission: choice.permission,
      target: modelTargetOf(parsed),
    });
  };
  /**
   * "Continue here": one `agent.configuration.set` (Agent, project, permission, model), then
   * `agent.start` with the instruction when there is one. Throws for the dialog to show when the
   * switch fails. When only the start fails, the switch stands (the dialog is not offered again), so
   * the dialog closes and the instruction waits in the composer.
   */
  const continueHere = async (choice: AgentContinuation) => {
    const id = route.id;
    if (!id) return;
    setBusy(true);
    try {
      await call("agent.configuration.set", {
        session_id: id,
        configuration: agentConfiguration(choice),
      });
      contextTouched.current = true;
      permissionTouched.current = true;
      setMode("agent");
      setPermission(choice.permission);
      setWorkspaceId(choice.workspaceId);
      session.refetch();
      const instruction = choice.instruction.trim();
      if (instruction) {
        try {
          await call("agent.start", {
            session_id: id,
            content: instruction,
            command_id: continuationCommandId(
              JSON.stringify({ start: id, instruction }),
            ),
          });
        } catch (caught) {
          setDraft((current) => draftWithInstruction(current, instruction));
          notify(
            `Switched to Agent, but the first instruction was not sent (${toAidexError(caught).message}). It is in the composer.`,
            "danger",
          );
          focusAfterContinue.current = "composer";
          setPopup(null);
          return;
        }
        continuationCommand.current = null;
        session.refetch();
      }
      // A started run keeps the composer disabled, so focus goes to the conversation instead.
      focusAfterContinue.current = instruction ? "heading" : "composer";
      setPopup(null);
    } finally {
      setBusy(false);
    }
  };
  /**
   * "Continue in a new Agent chat": `conversation.fork` copies the history and sent attachments into
   * a new Agent conversation, which opens; the instruction starts there. This conversation stays as
   * it is. Throws for the dialog to show when the fork fails.
   */
  const continueInNewChat = async (choice: AgentContinuation) => {
    const id = route.id;
    if (!id || !session.data) return;
    setBusy(true);
    try {
      const config = agentConfiguration(choice);
      const title = forkTitle(session.data.title);
      const fork = await call("conversation.fork", {
        source_id: id,
        configuration: config,
        title,
        command_id: continuationCommandId(
          JSON.stringify({ fork: id, config, title }),
        ),
      });
      continuationCommand.current = null;
      const instruction = choice.instruction.trim();
      if (instruction) {
        try {
          await call("agent.start", {
            session_id: fork.id,
            content: instruction,
            command_id: crypto.randomUUID(),
          });
        } catch (caught) {
          // The new chat exists; its composer gets the instruction to send again.
          preferences.setPendingPrompt(instruction);
          notify(
            `The new Agent chat is open, but its first instruction was not sent: ${toAidexError(caught).message}`,
            "danger",
          );
        }
      }
      setPopup(null);
      navigate({ page: "agent", id: fork.id });
    } finally {
      setBusy(false);
    }
  };
  const pendingApprovals =
    session.data?.approvals.filter((a) => a.status === "pending") ?? [];
  const runtimeModes = runtimeDriver?.modes ?? ["agent"];
  // The conversation's own model cannot run as an agent (it has no native tool calling).
  const agentUnavailable =
    target?.kind === "runtime"
      ? !runtimeModes.includes("agent")
      : resolved.model?.capabilities.tool_calls !== "native";
  const modeOptions = [
    {
      value: "chat",
      label: "Chat · no agent tools",
      disabled: target?.kind === "runtime" && !runtimeModes.includes("chat"),
    },
    {
      value: "plan",
      label: "Plan · project context",
      disabled: target?.kind === "runtime" && !runtimeModes.includes("plan"),
    },
    {
      value: "agent",
      label: "Agent · coding tools",
      // The Continue as Agent dialog offers a model that can.
      disabled: agentUnavailable && !continueAvailable,
    },
  ];
  const modeDisabled = (value: ConversationMode) =>
    !!modeOptions.find((option) => option.value === value)?.disabled;
  const slashCommandStates: SlashCommandState[] = [
    { id: "chat", disabled: modeDisabled("chat") },
    { id: "plan", disabled: modeDisabled("plan") },
    {
      id: "agent",
      disabled: modeDisabled("agent"),
      description: continueAvailable
        ? "Continue as Agent: choose a project, permissions, and a model"
        : undefined,
    },
    {
      id: "web",
      disabled: external || !extensions.web.available || mode !== "chat",
      description: external
        ? "This agent runtime uses its own tools"
        : !extensions.web.available
          ? (extensions.web.reason ?? "Set up web search in Extensions → Web")
          : mode !== "chat"
            ? "Plan and Agent modes search the web while it is on in Extensions → Web"
            : webSearch.on
              ? "Turn web search off for this conversation"
              : "Turn web search on for this conversation",
    },
    { id: "new" },
  ];
  const runSlashCommand = (id: SlashCommandId) => {
    if (id === "new") shell.startNewSession();
    else if (id === "web") webSearch.set(!webSearch.on);
    else if (id === "agent" && continueAvailable) setPopup("continue");
    else if (id !== mode) void changeMode(id);
  };
  const composerMenu = useComposerMenu({
    draft,
    setDraft,
    textareaRef,
    anchorRef: composerRef,
    skills: extensions.skills,
    skillsLoading: extensions.skillsLoading,
    skillsEnabled: !external,
    commands: slashCommandStates,
    onCommand: runSlashCommand,
    onManageSkills: () => navigate({ page: "extensions", tab: "skills" }),
    onOpen: (kind) => {
      extensions.refetchSkills();
      if (kind === "command") extensions.refetchWeb();
    },
    disabled: busy || running,
  });
  /** Plan card "Implement": switch this conversation to Agent and send the plan. */
  const implementPlan = (markdown: string) => {
    if (!workspaceId) {
      setError("Choose a project before starting Agent mode.");
      return;
    }
    if (agentUnavailable) {
      setError(`${modelName} cannot use agent tools. Choose a model with tool calling to implement the plan.`);
      return;
    }
    void submit(`Implement this plan:\n\n${markdown}`, { composer: false, mode: "agent" });
  };
  /** Plan card "Implement in a new chat": a new Agent chat in this project sends the plan on arrival. */
  const implementPlanInNewChat = (markdown: string) => {
    preferences.setPendingPrompt(`Implement this plan:\n\n${markdown}`);
    preferences.setPendingWorkspaceId(workspaceId || null);
    navigate({ page: "agent" });
  };
  const transcript: TranscriptEnvironment = {
    scope: { session_id: session.data?.id ?? route.id },
    openLink,
    openArtifact: (artifact) => setArtifactDialog({ artifact }),
    downloadArtifact,
    planStatus: extensions.planStatus,
    planActions: {
      implement: implementPlan,
      implementInNewChat: implementPlanInNewChat,
      disabled: busy || running,
    },
    skill: extensions.skill,
  };
  const dock = (
    <Composer
      ref={composerRef}
      textareaRef={textareaRef}
      size="compact"
      autoFocus
      label="Message"
      value={draft}
      onValueChange={setDraft}
      onSelectionChange={composerMenu.onSelectionChange}
      onKeyDown={composerMenu.onKeyDown}
      suggestions={composerMenu.suggestions}
      menu={composerMenu.menu}
      onSubmit={
        resolved.state === "ready" ? (text) => void submit(text) : undefined
      }
      disabled={busy || running || attachments.sending}
      onFiles={addFiles}
      onLargePaste={external ? undefined : foldPaste}
      attachActions={[
        {
          id: "project",
          label: "From project…",
          icon: <FolderIcon />,
          onSelect: () => setPopup("attach"),
        },
      ]}
      attachDisabledReason={external ? EXTERNAL_ATTACH_REASON : undefined}
      canSubmitEmpty={attachments.items.length > 0}
      submitBlockedReason={attachmentBlock}
      placeholder={mode === "chat" ? "Message AiDEX…" : "Describe the task…"}
      attachments={
        attachments.items.length > 0 ? (
          <AttachmentTray
            items={attachments.items.map(trayItemOf)}
            onRemove={(key) => attachments.draft.remove(key)}
            onRetry={(key) => attachments.draft.retry(key)}
            disabled={busy || running || attachments.sending}
          />
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
          <ConversationModelPicker
            value={resolved.key}
            current={current}
            onSelect={(key) => void selectModel(key)}
            disabled={busy || running || (!!route.id && !session.data)}
          />
          <IconButton
            size="sm"
            label={project ? `Project: ${project.name}` : "No project context"}
            icon={<FolderIcon />}
            selected={!!project}
            disabled={busy || running}
            onClick={() => setPopup("project")}
          />
          {!external && (
            <WebSearchToggle
              availability={extensions.web}
              mode={mode}
              on={webSearch.on}
              onChange={webSearch.set}
              disabled={busy || running}
            />
          )}
          <Text as="span" tone="muted">
            {modeLabels[mode]}
            {project ? ` · ${project.name}` : ""}
          </Text>
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
  // `LocalInferenceNotice` explains a starting or failed local model; it does not cover a
  // conversation bound to a different local model, which still needs its own notice.
  const localNoticeShown =
    (inference?.state === "starting" || inference?.state === "failed") &&
    (!ownLocalModelId ||
      !inference.model_id ||
      inference.model_id === ownLocalModelId);
  return (
    <FileDropZone
      onFiles={addFiles}
      disabled={busy || running}
      disabledReason={external ? EXTERNAL_ATTACH_REASON : undefined}
    >
      <DockLayout width="prose" dock={dock}>
        <div className={styles.conversation}>
          <header className={styles.header}>
            <h1 ref={headingRef} tabIndex={-1}>
              {session.data?.title || "New chat"}
            </h1>
            {route.id && (
              <Stack direction="row" gap={1}>
                {continueAvailable && (
                  <IconButton
                    size="sm"
                    label="Continue as Agent"
                    icon={<BotIcon />}
                    aria-haspopup="dialog"
                    disabled={busy || running}
                    onClick={() => setPopup("continue")}
                  />
                )}
                <IconButton
                  size="sm"
                  label="Conversation settings"
                  icon={<SlidersIcon />}
                  aria-haspopup="dialog"
                  disabled={!session.data}
                  onClick={() => setPopup("settings")}
                />
                <IconButton
                  size="sm"
                  label="Artifacts"
                  icon={<LayersIcon />}
                  aria-haspopup="dialog"
                  onClick={() => setArtifactDialog({ artifact: null })}
                />
                <IconButton
                  size="sm"
                  label="Execution details"
                  icon={<BoxIcon />}
                  onClick={() => navigate({ page: "execution", id: route.id })}
                />
              </Stack>
            )}
          </header>
          {(error || session.error || catalog.error || localCatalog.error) && (
            <Notice tone="danger" role="alert">
              {error ??
                session.error?.message ??
                catalog.error ??
                localCatalog.error?.message}
            </Notice>
          )}
          <LocalInferenceNotice
            status={inference}
            onRefresh={catalog.refreshInference}
          />
          {(!route.id || session.data) && (
            <ModelStateNotice
              state={resolved.state}
              name={modelName}
              installedLocalName={installedModel?.name}
              localName={ownLocalName}
              hasModels={catalog.models.length > 0}
              loading={
                catalog.loading ||
                (target?.kind === "runtime" && catalog.runtimesLoading)
              }
              localNoticeShown={localNoticeShown}
            />
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
          <TranscriptProvider value={transcript}>
            <Stack gap={5} role="log" aria-label="Conversation">
              <Transcript
                messages={session.data?.messages ?? []}
                options={{ running, complete: true }}
                markers={markers}
              />
            </Stack>
          </TranscriptProvider>
          {session.data?.messages_truncated && (
            <details>
              <summary>Earlier messages</summary>
              <HistoryMessages scope={{ session_id: session.data.id }} />
            </details>
          )}
          {latestRun?.error && (!session.data?.workspace_id || project) && (
            <Notice tone="danger">{latestRun.error}</Notice>
          )}
          {latestRun && resumable && (
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
            <ApprovalRequestCard
              key={approval.id}
              approval={approval}
              busy={busy}
              onDecide={(approved) =>
                void act(() =>
                  call(
                    "approval.respond",
                    approvalDecision(approval, approved, crypto.randomUUID()),
                  ),
                )
              }
            />
          ))}
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
            options={modeOptions}
            onChange={(event) =>
              void changeMode(event.target.value as ConversationMode)
            }
          />
          {mode !== "chat" && (
            <Select
              label="Permissions"
              disabled={busy || running || target?.kind === "runtime"}
              hint={
                target?.kind === "runtime"
                  ? "External runtimes run with full access."
                  : permissionDescription(permission, { shellNeedsFullAccess })
              }
              value={permission}
              options={permissionOptions}
              onChange={(event) => {
                const value = event.target.value as PermissionMode;
                permissionTouched.current = true;
                if (route.id)
                  void act(async () => {
                    await call("agent.configuration.set", {
                      session_id: route.id!,
                      configuration: configuration({ permission: value }),
                    });
                    setPermission(value);
                  });
                else setPermission(value);
              }}
            />
          )}
          <Text tone="muted">
            Plan and Agent require a project. Chat works independently.
          </Text>
        </Dialog>
        <Dialog
          open={popup === "project"}
          title="Project context"
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
          <Text tone="muted">
            Ordinary chats don't need a project. Choose one to include project
            context.
          </Text>
          <Button
            variant="ghost"
            onClick={() => navigate({ page: "workspace-create" })}
          >
            Add a workspace
          </Button>
        </Dialog>
        <ContinueAsAgentDialog
          open={continuing}
          onClose={() => setPopup(null)}
          workspaces={workspaces.data?.workspaces ?? []}
          workspaceIds={[session.data?.workspace_id, preferences.workspaceId]}
          models={agentModels}
          modelKeys={[resolved.key]}
          currentModelName={modelName}
          currentModelState={resolved.state}
          carried={carriedAttachments(
            sessionAttachments.data?.attachments ??
              (session.data?.messages ?? []).flatMap(
                (message) => message.attachments ?? [],
              ),
          )}
          composerAttachments={attachments.items.length}
          shellNeedsFullAccess={shellNeedsFullAccess}
          onAddWorkspace={() => navigate({ page: "workspace-create" })}
          onContinueHere={continueHere}
          onContinueInNewChat={continueInNewChat}
        />
        {session.data && (
          <ConversationSettingsDialog
            open={popup === "settings"}
            onClose={() => setPopup(null)}
            settings={{
              instructions: session.data.instructions ?? "",
              fallback_model: session.data.fallback_model ?? null,
              max_steps: session.data.max_steps ?? preferences.settings.max_steps,
              max_tokens: session.data.max_tokens ?? preferences.settings.max_tokens,
            }}
            external={external}
            fallbackOptions={
              popup === "settings" && !external
                ? fallbackModelOptions({
                    models: catalog.models,
                    findModel: catalog.findModel,
                    providerName: (key) => catalog.providerFor(key)?.name,
                    agent: mode === "agent",
                    primaryKey: resolved.key,
                    current: session.data.fallback_model ?? null,
                  })
                : []
            }
            locked={running}
            onSave={saveSettings}
          />
        )}
        <ProjectFileDialog
          open={popup === "attach"}
          onClose={() => setPopup(null)}
          workspaces={workspaces.data?.workspaces ?? []}
          defaultWorkspaceId={workspaceId}
          onAttach={(id, path, name) => {
            const problem = attachments.draft.addProjectFile(id, path, name);
            if (problem) setError(problem);
          }}
          onAddWorkspace={() => navigate({ page: "workspace-create" })}
        />
        {route.id && (
          <ArtifactsDialog
            open={artifactDialog !== null}
            sessionId={route.id}
            artifact={artifactDialog?.artifact ?? null}
            onClose={() => setArtifactDialog(null)}
            onOpenLink={openLink}
            onDownload={downloadArtifact}
          />
        )}
      </DockLayout>
    </FileDropZone>
  );
}
