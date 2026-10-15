import { useRef, useState } from "react";
import { AttachmentTray, Composer, Notice } from "@aidex/ui/components";
import { CodeIcon, FileTextIcon, LightbulbIcon, MessageSquareIcon } from "@aidex/ui/icons";
import { Button, Container, Heading, SegmentedControl, Stack, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useToast } from "../../app/providers/toast";
import { attachmentSendBlock } from "../chat/attachments/attachmentFiles";
import { trayItemOf, useDraftAttachments } from "../chat/attachments/useConversationAttachments";
import { SystemOverview } from "./SystemOverview";

type Mode = "chat" | "agent";

type Props = { autoFocus?: boolean };

const modeOptions = [
  { value: "chat", label: "Chat" },
  { value: "agent", label: "Agent" },
] as const;

const placeholderByMode: Record<Mode, string> = {
  chat: "Ask anything, start a task, or work on your codebase...",
  agent: "Describe a task for an agent to carry out...",
};

const suggestions = [
  { label: "Summarize notes", Icon: FileTextIcon, prompt: "Summarize these notes: " },
  { label: "Fix a bug", Icon: CodeIcon, prompt: "Help me fix a bug: " },
  { label: "Plan feature", Icon: LightbulbIcon, prompt: "Help me plan a feature: " },
  { label: "Review code", Icon: MessageSquareIcon, prompt: "Review this code: " },
] as const;

export function HomePage({ autoFocus = false }: Props) {
  const [mode, setMode] = useState<Mode>("chat");
  const [draft, setDraft] = useState("");
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const { navigate } = useNavigation();
  const { setPendingPrompt, setPendingAttachments } = usePreferences();
  const notify = useToast();
  // Attachments are prepared here and uploaded by the new chat once its conversation exists.
  const attachments = useDraftAttachments();
  const [problem, setProblem] = useState<string>();
  const attachmentBlock = attachmentSendBlock({ items: attachments.items, external: false });

  // The chat or agent screen picks the prompt and attachments up on arrival.
  const submit = (prompt: string) => {
    if (attachmentBlock) return;
    setPendingPrompt(prompt.trim());
    setPendingAttachments(attachments.items.length ? attachments.draft.takeItems() : null);
    navigate({ page: mode === "agent" ? "agent" : "chat" });
  };

  const addFiles = (files: File[]) => {
    setProblem(undefined);
    void attachments.draft.add(files).then((problems) => {
      if (problems.length) setProblem(problems.join(" "));
    });
  };

  const foldPaste = (text: string) => {
    setProblem(undefined);
    const result = attachments.draft.addPaste(text);
    if ("problem" in result) setProblem(result.problem);
    else notify(`Large paste attached as ${result.name}`);
  };

  // Frames whatever is already typed instead of discarding it.
  const applySuggestion = (prompt: string) => {
    setDraft((current) => (current.trim() ? prompt + current.trimStart() : prompt));
    composerRef.current?.focus();
  };

  return (
    <Container size="md" verticallyCentered>
      <Stack gap={8}>
        <Stack direction="row" justify="center">
          <SegmentedControl label="Conversation mode" options={modeOptions} value={mode} onValueChange={setMode} />
        </Stack>
        <Stack gap={6}>
          <Stack gap={3}>
            <Heading level={1} size="display" align="center">
              Welcome to AiDEX
            </Heading>
            <Text variant="lead" tone="secondary" align="center">
              Your AI workspace for deeper thinking, faster building, and greater outcomes.
            </Text>
          </Stack>
          <Stack gap={5}>
            {problem && (
              <Notice tone="danger" role="alert">
                {problem}
              </Notice>
            )}
            <Composer
              label="Message"
              placeholder={placeholderByMode[mode]}
              value={draft}
              onValueChange={setDraft}
              onSubmit={submit}
              onFiles={addFiles}
              onLargePaste={foldPaste}
              canSubmitEmpty={attachments.items.length > 0}
              submitBlockedReason={attachmentBlock}
              attachments={
                attachments.items.length > 0 ? (
                  <AttachmentTray
                    items={attachments.items.map(trayItemOf)}
                    onRemove={(key) => attachments.draft.remove(key)}
                  />
                ) : undefined
              }
              textareaRef={composerRef}
              autoFocus={autoFocus}
            />
            <Stack direction="row" gap={4} justify="center" wrap role="group" aria-label="Suggestions">
              {suggestions.map(({ label, Icon, prompt }) => (
                <Button key={label} variant="outline" size="md" icon={<Icon />} onClick={() => applySuggestion(prompt)}>
                  {label}
                </Button>
              ))}
            </Stack>
          </Stack>
        </Stack>
        <SystemOverview />
      </Stack>
    </Container>
  );
}
