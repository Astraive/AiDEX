import {
  useCallback,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import {
  CommandMenu,
  type CommandMenuFieldState,
  type CommandMenuItem,
  type CommandMenuSection,
} from "@aidex/ui/components";
import {
  BotIcon,
  ChatIcon,
  ClipboardCheckIcon,
  GlobeIcon,
  PlusIcon,
  SettingsIcon,
  SparklesIcon,
} from "@aidex/ui/icons";
import type { SkillId, SkillSummary } from "@aidex/client/dto/skills";
import { detectComposerTrigger, replaceRange, type ComposerTrigger } from "./composerText";
import { searchSkills, skillScopeLabels } from "./skillSearch";
import { searchSlashMatches, slashCommands, type SlashCommand, type SlashCommandId } from "./slashCommands";

/** A built-in `/` command as the conversation can run it right now. */
export type SlashCommandState = {
  id: SlashCommandId;
  disabled?: boolean;
  /** Replaces the default description, e.g. to say why the command is unavailable. */
  description?: string;
};

type Options = {
  draft: string;
  setDraft: (value: string) => void;
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  anchorRef: RefObject<HTMLElement | null>;
  /** Invocable skills for the conversation's project (see `invocableSkills`). */
  skills: readonly SkillSummary[];
  skillsLoading: boolean;
  /** False for external runtimes, which find their own skills: `$` then stays plain text. */
  skillsEnabled: boolean;
  commands: readonly SlashCommandState[];
  onCommand: (id: SlashCommandId) => void;
  /** Opens Extensions → Skills. */
  onManageSkills: () => void;
  /** Called when a menu opens, e.g. to refresh the skill list. */
  onOpen?: (kind: ComposerTrigger["kind"]) => void;
  disabled: boolean;
};

export type ComposerMenu = {
  /** The `CommandMenu`, for the composer's `menu` slot. */
  menu: ReactNode;
  /** For the composer's `suggestions` prop. */
  suggestions: CommandMenuFieldState;
  /** For the composer's `onSelectionChange` prop. */
  onSelectionChange: (caret: number) => void;
  /** For the composer's `onKeyDown` prop: Enter never sends while the menu's highlighted row can't run. */
  onKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  /** Skills picked from a menu, by name, so a send keeps the exact skill that was picked. */
  picked: ReadonlyMap<string, SkillId>;
  /** Forgets picked skills after a send. */
  clearPicked: () => void;
};

type MenuView = { sections: CommandMenuSection[]; emptyText: string; open: boolean };

const MANAGE_SKILLS_ID = "manage-skills";

const commandIcons: Record<SlashCommandId, ReactNode> = {
  chat: <ChatIcon />,
  plan: <ClipboardCheckIcon />,
  agent: <BotIcon />,
  web: <GlobeIcon />,
  new: <PlusIcon />,
};

function skillItem(skill: SkillSummary, id: string, slash: boolean): CommandMenuItem {
  return {
    id,
    title: skill.name,
    prefix: slash ? "/skill:" : "$",
    description: skill.description,
    icon: <SparklesIcon />,
    badge: skillScopeLabels[skill.scope],
    tooltip: [
      `${slash ? "/skill:" : "$"}${skill.name}`,
      skill.description,
      skill.plugin ? `From the ${skill.plugin} plugin` : `${skillScopeLabels[skill.scope]} skill`,
    ].join(" — "),
  };
}

/**
 * The composer's `$` skills menu and `/` command menu: detects the trigger before the caret, ranks
 * rows, and applies a pick (inserting `$name ` for a skill, running a built-in command otherwise).
 */
export function useComposerMenu({
  draft,
  setDraft,
  textareaRef,
  anchorRef,
  skills,
  skillsLoading,
  skillsEnabled,
  commands,
  onCommand,
  onManageSkills,
  onOpen,
  disabled,
}: Options): ComposerMenu {
  const listId = useId();
  const [caret, setCaret] = useState<number | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  // Escape or a press outside closes the menu until the caret leaves that trigger.
  const [dismissed, setDismissed] = useState<{ kind: ComposerTrigger["kind"]; start: number } | null>(null);
  const [picked, setPicked] = useState<ReadonlyMap<string, SkillId>>(() => new Map());
  const pendingCaret = useRef<number | null>(null);
  const lastOpenKind = useRef<ComposerTrigger["kind"] | null>(null);

  const trigger = caret === null ? null : detectComposerTrigger(draft, caret);
  const isDismissed =
    !!trigger && !!dismissed && dismissed.kind === trigger.kind && dismissed.start === trigger.start;

  const commandList = useMemo<SlashCommand[]>(
    () =>
      slashCommands.flatMap((command) => {
        const state = commands.find((candidate) => candidate.id === command.id);
        return state
          ? [{ ...command, description: state.description ?? command.description, disabled: !!state.disabled }]
          : [];
      }),
    [commands],
  );

  const { sections, emptyText, open } = useMemo<MenuView>(() => {
    const closed: MenuView = { sections: [], emptyText: "", open: false };
    if (!trigger || disabled || isDismissed) return closed;
    if (trigger.kind === "skill") {
      if (!skillsEnabled) return closed;
      const ranked = searchSkills(skills, trigger.query);
      if (ranked.length > 0)
        return {
          open: true,
          emptyText: "",
          sections: [
            {
              id: "skills",
              label: "Skills",
              items: ranked.map((skill) => skillItem(skill, `skill:${skill.id}`, false)),
            },
          ],
        };
      if (!trigger.query && !skillsLoading)
        return {
          open: true,
          emptyText: "",
          sections: [
            {
              id: "manage",
              items: [
                {
                  id: MANAGE_SKILLS_ID,
                  title: "Manage skills",
                  description:
                    skills.length === 0
                      ? "No skills are enabled here. Enable or create skills in Extensions → Skills."
                      : "Open Extensions → Skills.",
                  icon: <SettingsIcon />,
                },
              ],
            },
          ],
        };
      return {
        open: true,
        emptyText: "No skills match. Manage skills in Extensions → Skills.",
        sections: [],
      };
    }
    // A typed command that can't run here (e.g. `/w` for a disabled `/web`) ranks first and the menu
    // highlights it even though it is disabled, so Enter neither runs a lower row nor sends the text.
    const commandItems: CommandMenuItem[] = [];
    const skillItems: CommandMenuItem[] = [];
    for (const match of searchSlashMatches(commandList, skillsEnabled ? skills : [], trigger.query)) {
      if (match.type === "command")
        commandItems.push({
          id: `command:${match.command.id}`,
          title: match.command.command,
          prefix: "/",
          description: match.command.description,
          icon: commandIcons[match.command.id],
          disabled: match.command.disabled,
        });
      else skillItems.push(skillItem(match.skill, `slash-skill:${match.skill.id}`, true));
    }
    return {
      open: true,
      emptyText: "No matching command.",
      sections: [
        { id: "commands", label: "Commands", items: commandItems },
        { id: "skills", label: "Skills", items: skillItems },
      ],
    };
  }, [trigger?.kind, trigger?.query, disabled, isDismissed, skillsEnabled, skills, skillsLoading, commandList]);

  // Tell the page a menu opened (once per opening), e.g. to refresh skills.
  const openKind = open ? (trigger?.kind ?? null) : null;
  useLayoutEffect(() => {
    if (openKind && openKind !== lastOpenKind.current) onOpen?.(openKind);
    lastOpenKind.current = openKind;
  }, [openKind, onOpen]);

  // A pick replaces the draft; put the caret after the inserted text once it renders.
  useLayoutEffect(() => {
    const position = pendingCaret.current;
    const field = textareaRef.current;
    if (position === null || !field) return;
    pendingCaret.current = null;
    field.focus();
    field.setSelectionRange(position, position);
    setCaret(position);
  }, [draft, textareaRef]);

  const replaceTrigger = (replacement: string) => {
    if (!trigger) return;
    const next = replaceRange(draft, trigger.start, trigger.end, replacement);
    pendingCaret.current = next.caret;
    setDraft(next.text);
    setCaret(next.caret);
  };

  const select = (item: CommandMenuItem) => {
    if (!trigger) return;
    if (item.id === MANAGE_SKILLS_ID) {
      replaceTrigger("");
      onManageSkills();
      return;
    }
    const [kind, ...rest] = item.id.split(":");
    const value = rest.join(":");
    if (kind === "command") {
      replaceTrigger("");
      onCommand(value as SlashCommandId);
      return;
    }
    const skill = skills.find((candidate) => candidate.id === value);
    if (!skill) return;
    setPicked((current) => new Map(current).set(skill.name, skill.id));
    replaceTrigger(`$${skill.name} `);
  };

  const onSelectionChange = useCallback((position: number) => setCaret(position), []);

  // A dismissal lasts only while the caret stays on that trigger.
  useLayoutEffect(() => {
    if (dismissed && (!trigger || trigger.kind !== dismissed.kind || trigger.start !== dismissed.start))
      setDismissed(null);
  }, [dismissed, trigger?.kind, trigger?.start]);

  const clearPicked = useCallback(() => setPicked(new Map()), []);

  // The menu swallows a plain Enter on a highlighted row that can't run, but lets Ctrl, Alt, and Meta +
  // Enter through to the composer, which would send the command text as a message; those do nothing
  // too. With an enabled row highlighted, or no rows listed (e.g. `/context` while Plan can't run),
  // Enter works as before.
  const activeDisabled =
    open &&
    activeId !== null &&
    sections.some((section) => section.items.some((item) => item.id === activeId && item.disabled));
  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
      if (activeDisabled) event.preventDefault();
    },
    [activeDisabled],
  );

  const menu = (
    <CommandMenu
      id={listId}
      open={open}
      anchorRef={anchorRef}
      inputRef={textareaRef}
      query={trigger?.query ?? ""}
      sections={sections}
      activeId={activeId}
      onActiveChange={setActiveId}
      onSelect={select}
      onClose={() => trigger && setDismissed({ kind: trigger.kind, start: trigger.start })}
      label={trigger?.kind === "command" ? "Commands" : "Skills"}
      loading={trigger?.kind === "skill" && skillsLoading}
      loadingText="Loading skills…"
      emptyText={emptyText}
    />
  );

  return {
    menu,
    suggestions: { listId, open, activeId },
    onSelectionChange,
    onKeyDown,
    picked,
    clearPicked,
  };
}
