// "Continue as Agent": the choices the dialog offers and the defaults it starts from. Pure functions,
// so they run during prerendering.
import type { AttachmentKind, AttachmentState } from "@aidex/client/dto/attachments";
import type { ModelInput, ProviderModel } from "@aidex/client/dto/models";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import type { Workspace } from "@aidex/client/dto/workspace";
import { parseModelKey } from "../../model-manager/catalog/modelKeys";
import type { ConversationModelState } from "../resolveConversationModel";

/** What the user chose in the dialog. */
export type AgentContinuation = {
  workspaceId: string;
  permission: PermissionMode;
  /** A native model key with native tool calling. */
  modelKey: string;
  /** Sent with `agent.start` when not blank. */
  instruction: string;
};

export type AgentModelOption = {
  key: string;
  /** Model name, for messages. */
  name: string;
  /** Model and provider, for the list. */
  label: string;
  input: ModelInput;
};

/** A picker row: its key, name, and whether it can be chosen. */
type ListedModel = { key: string; name: string; unavailable?: boolean; disabledReason?: string | null };

/** Image and PDF input of a model; daemons before attachments report only `vision`. */
export function modelInputOf(model: Pick<ProviderModel, "input" | "capabilities">): ModelInput {
  return model.input ?? { images: model.capabilities.vision, pdf: false };
}

/** Whether a listed model can run as an agent: AiDEX's own harness with native tool calling. */
export function canRunAgent(key: string, descriptor: ProviderModel | undefined): descriptor is ProviderModel {
  return parseModelKey(key)?.kind === "native" && descriptor?.capabilities.tool_calls === "native";
}

/**
 * Models an agent can run on, in picker order: native models with native tool calling that can be
 * chosen now. The conversation's own model is kept even when it is hidden from the picker.
 */
export function agentModelOptions(input: {
  models: readonly ListedModel[];
  findModel: (key: string) => ProviderModel | undefined;
  providerName: (key: string) => string | undefined;
  current?: { key: string | null; name: string; descriptor?: ProviderModel };
}): AgentModelOption[] {
  const option = (key: string, name: string, descriptor: ProviderModel): AgentModelOption => {
    const provider = input.providerName(key);
    return { key, name, label: provider ? `${name} · ${provider}` : name, input: modelInputOf(descriptor) };
  };
  const options: AgentModelOption[] = [];
  for (const model of input.models) {
    if (model.unavailable || model.disabledReason) continue;
    const descriptor = input.findModel(model.key);
    if (canRunAgent(model.key, descriptor)) options.push(option(model.key, model.name, descriptor));
  }
  const current = input.current;
  if (current?.key && !options.some((item) => item.key === current.key) && canRunAgent(current.key, current.descriptor))
    options.unshift(option(current.key, current.name, current.descriptor));
  return options;
}

/**
 * Why the conversation's own model is not offered, or undefined when it is (or there is none). A model
 * that is not listed now may well use tools, so it is called unavailable; any other is not offered
 * because it has no native tool calling (AiDEX's local model never has).
 */
export function currentModelHint(input: {
  name: string;
  state: ConversationModelState;
  offered: boolean;
}): string | undefined {
  if (input.offered) return undefined;
  if (input.state === "unlisted")
    return `${input.name} is not available right now. Choose a model that can use agent tools.`;
  return `${input.name} can't use agent tools. Choose a model that can.`;
}

/**
 * The composer text after "Continue here" switched the conversation but its first instruction was
 * not sent: the instruction, after any text already there.
 */
export function draftWithInstruction(draft: string, instruction: string): string {
  return draft.trim() ? `${draft.trimEnd()}\n\n${instruction}` : instruction;
}

/** Images and PDFs the conversation's messages carry to the agent (excluded ones are not sent). */
export function carriedAttachments(
  attachments: readonly { kind: AttachmentKind; state?: AttachmentState; excluded?: boolean }[],
): { images: number; pdfs: number } {
  const sent = attachments.filter((item) => (item.state ?? "sent") === "sent" && !item.excluded);
  return {
    images: sent.filter((item) => item.kind === "image").length,
    pdfs: sent.filter((item) => item.kind === "pdf").length,
  };
}

function count(n: number, noun: string): string {
  return n === 1 ? `1 ${noun}` : `${n} ${noun}s`;
}

/**
 * Why the agent would not see some earlier attachments with `model`, or undefined. The harness
 * replaces what a model cannot read with a line naming the file, so this warns rather than blocks.
 */
export function carriedAttachmentWarning(
  model: AgentModelOption | undefined,
  carried: { images: number; pdfs: number },
): string | undefined {
  if (!model) return undefined;
  const unread = [
    ...(carried.images > 0 && !model.input.images ? [count(carried.images, "image")] : []),
    ...(carried.pdfs > 0 && !model.input.pdf ? [count(carried.pdfs, "PDF")] : []),
  ];
  if (!unread.length) return undefined;
  return `${model.name} can't read the ${unread.join(" and ")} in this conversation; the agent sees only their names.`;
}

/** The dialog's starting choices: the conversation's project and model when they fit, Supervised. */
export function defaultContinuation(input: {
  workspaces: readonly Workspace[];
  /** The conversation's project, then the default for new chats. */
  workspaceIds: readonly (string | null | undefined)[];
  models: readonly AgentModelOption[];
  /** The conversation's model, then the default for new chats. */
  modelKeys: readonly (string | null | undefined)[];
}): AgentContinuation {
  const available = input.workspaces.filter((workspace) => workspace.availability.state === "available");
  const workspace =
    input.workspaceIds
      .map((id) => available.find((item) => !!id && (item.id === id || item.name === id)))
      .find(Boolean) ?? (available.length === 1 ? available[0] : undefined);
  const model = input.modelKeys.map((key) => input.models.find((item) => item.key === key)).find(Boolean);
  return { workspaceId: workspace?.id ?? "", permission: "supervised", modelKey: model?.key ?? "", instruction: "" };
}

/** Title of the new Agent chat (`conversation.fork`); titles are at most 512 bytes. */
export function forkTitle(title: string): string {
  const base = title.trim() || "Conversation";
  const next = `${base} (Agent)`;
  return new TextEncoder().encode(next).length <= 512 ? next : base;
}
