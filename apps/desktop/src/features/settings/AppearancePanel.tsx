import {
  APPEARANCE_DEFAULTS,
  APPEARANCE_LIMITS,
  CHAT_WIDTH_PX,
  UI_SCALE_STEPS,
  type AppPreferences,
  type ChatWidth,
  type MessageSpacing,
  type UiDensity,
} from "@aidex/client/dto/preferences";
import { Notice, SectionHeader, SettingRow, type SettingRowReset } from "@aidex/ui/components";
import { RotateCcwIcon } from "@aidex/ui/icons";
import { Button, SegmentedControl, Select, Stack, SwatchPicker, Switch } from "@aidex/ui/primitives";
import { DEFAULT_ACCENT, accentPresets } from "@aidex/ui/themes";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useAppearance } from "../../app/providers/appearance";
import { isDefaultAppearance } from "../../app/providers/appearanceState";
import { usePreferences } from "../../app/providers/preferences";
import { useToast } from "../../app/providers/toast";
import { useModifierKeyLabel } from "../../hooks/useModifierKeyLabel";

/** Defaults of the base preferences shown here (aidex-preferences `AppPreferences::default`). */
const PREFERENCE_DEFAULTS = {
  accent: DEFAULT_ACCENT,
  theme: "dark",
  reduce_motion: false,
  keyboard_hints: true,
} as const satisfies Partial<AppPreferences>;

/** Shell sidebar width when none is saved (`--aidex-sidebar-width` in @aidex/ui tokens.css). */
const AUTOMATIC_SIDEBAR_WIDTH = 256;
const SIDEBAR_PRESETS = [
  { value: "auto", label: `Automatic (${AUTOMATIC_SIDEBAR_WIDTH}px)`, width: null },
  { value: "224", label: "Narrow (224px)", width: 224 },
  { value: "320", label: "Wide (320px)", width: 320 },
] as const;

const THEME_OPTIONS = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "Follow system" },
] as const;

const DENSITY_OPTIONS: readonly { value: UiDensity; label: string }[] = [
  { value: "compact", label: "Compact" },
  { value: "comfortable", label: "Comfortable" },
  { value: "spacious", label: "Spacious" },
];

const CHAT_WIDTH_OPTIONS: readonly { value: ChatWidth; label: string }[] = [
  { value: "narrow", label: "Narrow" },
  { value: "comfortable", label: "Comfortable" },
  { value: "wide", label: "Wide" },
  { value: "full", label: "Full" },
];

const MESSAGE_SPACING_OPTIONS: readonly { value: MessageSpacing; label: string }[] = [
  { value: "compact", label: "Compact" },
  { value: "comfortable", label: "Comfortable" },
  { value: "relaxed", label: "Relaxed" },
];

/** Control ids: each SettingRow title is the `<label>` of its control and describes it by `${id}-description`. */
const ID = {
  theme: "appearance-theme",
  uiScale: "appearance-ui-scale",
  chatFontSize: "appearance-chat-font-size",
  codeFontSize: "appearance-code-font-size",
  sidebarWidth: "appearance-sidebar-width",
  sidebarAutoCollapse: "appearance-sidebar-auto-collapse",
  reduceMotion: "appearance-reduce-motion",
  keyboardHints: "appearance-keyboard-hints",
} as const;

const describedBy = (id: string) => `${id}-description`;

/** `n px` options for every whole size in a range. */
function pixelOptions(range: { min: number; max: number }) {
  return Array.from({ length: range.max - range.min + 1 }, (_, index) => {
    const size = range.min + index;
    return { value: String(size), label: `${size}px` };
  });
}

const CHAT_FONT_SIZE_OPTIONS = pixelOptions(APPEARANCE_LIMITS.chat_font_size);
const CODE_FONT_SIZE_OPTIONS = pixelOptions(APPEARANCE_LIMITS.code_font_size);

/** Scale steps, plus the saved value when it is between steps (another build may have saved it). */
function scaleOptions(current: number) {
  const steps: number[] = [...UI_SCALE_STEPS];
  if (!steps.includes(current)) steps.push(current);
  return steps.sort((a, b) => a - b).map((step) => ({ value: String(step), label: `${step}%` }));
}

function sidebarOptions(width: number | null) {
  const options: { value: string; label: string }[] = SIDEBAR_PRESETS.map(({ value, label }) => ({ value, label }));
  if (width !== null && !SIDEBAR_PRESETS.some((preset) => preset.width === width)) {
    options.push({ value: String(width), label: `Custom (${width}px)` });
  }
  return options;
}

function chatWidthDescription(width: ChatWidth): string {
  const px = CHAT_WIDTH_PX[width];
  return px === null ? "The conversation fills the main area." : `The conversation is at most ${px}px wide.`;
}

const TAB_STOPS = "button, a[href], input, select, textarea, [tabindex]";

/**
 * Focuses the first enabled control in `section`, trying the element with `preferredId` first.
 * Restore uses it when disabling itself would otherwise drop focus to the body.
 */
function focusFirstControl(section: HTMLElement | null, preferredId: string) {
  if (!section?.isConnected) return;
  const preferred = document.getElementById(preferredId);
  const candidates = [...(preferred ? [preferred] : []), ...section.querySelectorAll<HTMLElement>(TAB_STOPS)];
  for (const target of candidates) {
    if (target.tabIndex < 0 || target.matches(":disabled") || target.getClientRects().length === 0) continue;
    target.focus();
    if (document.activeElement === target) return;
  }
}

/**
 * A refused change. `role="alert"` announces it; when the notice is scrolled out of view (the change
 * was made further down the panel) it is also shown as a toast, so it is seen where the user is.
 */
function ErrorNotice({ message }: { message: string }) {
  const notify = useToast();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = ref.current;
    if (!element || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => {
      observer.disconnect();
      if (entry && !entry.isIntersecting) notify(message, "danger");
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [message, notify]);
  return (
    <Notice ref={ref} tone="danger" role="alert">
      {message}
    </Notice>
  );
}

/** Theme, accent, interface scale, density, text sizes and layout. Changes apply at once. */
export function AppearancePanel() {
  const { accent, setAccent, settings, updateSettings, error: preferencesError } = usePreferences();
  const { appearance, ready, unsupported, update, reset, error: appearanceError } = useAppearance();
  const modifierKey = useModifierKeyLabel();
  const updatePreferences = (patch: Partial<AppPreferences>) => void updateSettings(patch).catch(() => undefined);
  const set = (patch: Parameters<typeof update>[0]) => void update(patch);
  // Options the appearance record adds; an older Core cannot store them.
  const newOnlyDisabled = !ready || unsupported;
  const densityDisabled = !ready;

  /** A row's reset, offered only while the value differs from its default. */
  const resetWhen = (changed: boolean, label: string, onReset: () => void): SettingRowReset | undefined =>
    changed ? { label, onReset } : undefined;
  const resetAppearance = <Key extends keyof typeof APPEARANCE_DEFAULTS>(key: Key, label: string, disabled: boolean) =>
    resetWhen(!disabled && appearance[key] !== APPEARANCE_DEFAULTS[key], label, () =>
      set({ [key]: APPEARANCE_DEFAULTS[key] }),
    );

  const sidebarWidth = appearance.sidebar_width;
  const sidebarValue =
    sidebarWidth === null ? "auto" : (SIDEBAR_PRESETS.find((preset) => preset.width === sidebarWidth)?.value ?? String(sidebarWidth));
  // One refusal can set both errors (an older Core saves density through `settings.set`): show it
  // once, in the appearance wording, which names what was not saved.
  const shownPreferencesError =
    preferencesError && !appearanceError?.endsWith(preferencesError) ? preferencesError : null;

  // Restore disables itself once everything is at its default, which drops its focus to the body.
  // After a restore, focus then moves to the first Size control (the interface scale, or density
  // on an older Core). The layout effect also covers an older Core, where it disables only once
  // `settings.set` has answered.
  const restoreDisabled = !ready || isDefaultAppearance(appearance);
  const sizeSection = useRef<HTMLElement>(null);
  const restoreFocusPending = useRef(false);
  useLayoutEffect(() => {
    if (!restoreDisabled || !restoreFocusPending.current) return;
    restoreFocusPending.current = false;
    const active = document.activeElement;
    const dropped =
      active === null ||
      active === document.body ||
      (active instanceof HTMLButtonElement && active.disabled && !!sizeSection.current?.contains(active));
    if (dropped) focusFirstControl(sizeSection.current, ID.uiScale);
  }, [restoreDisabled]);

  return (
    <Stack gap={8}>
      {shownPreferencesError && <ErrorNotice message={shownPreferencesError} />}
      {appearanceError && <ErrorNotice message={appearanceError} />}
      {unsupported && (
        <Notice tone="info">
          Needs a newer AiDEX Core: interface scale, text sizes and layout options are off until AiDEX Core is updated.
          Density offers Compact and Comfortable.
        </Notice>
      )}
      <section>
        <SectionHeader title="Color and theme" />
        <SettingRow
          title="Accent color"
          description="Saved on the portable drive."
          reset={resetWhen(accent.toLowerCase() !== PREFERENCE_DEFAULTS.accent, "Reset accent color", () =>
            setAccent(PREFERENCE_DEFAULTS.accent),
          )}
          control={
            <SwatchPicker
              label="Accent color"
              swatches={accentPresets}
              value={accent}
              onValueChange={setAccent}
              allowCustom
            />
          }
        />
        <SettingRow
          title="Theme"
          controlId={ID.theme}
          reset={resetWhen(settings.theme !== PREFERENCE_DEFAULTS.theme, "Reset theme", () =>
            updatePreferences({ theme: PREFERENCE_DEFAULTS.theme }),
          )}
          control={
            <Select
              id={ID.theme}
              label="Theme"
              labelVisibility="hidden"
              value={settings.theme}
              options={THEME_OPTIONS}
              onChange={(event) => updatePreferences({ theme: event.target.value as AppPreferences["theme"] })}
            />
          }
        />
      </section>
      <section ref={sizeSection}>
        <SectionHeader
          title="Size"
          actions={
            <Button
              variant="ghost"
              size="sm"
              icon={<RotateCcwIcon />}
              disabled={restoreDisabled}
              onClick={() => {
                restoreFocusPending.current = true;
                void reset();
              }}
            >
              Restore appearance defaults
            </Button>
          }
        />
        <SettingRow
          title="Interface scale"
          // An older Core has no interface scale, so the zoom keys do nothing there.
          description={
            unsupported ? "Zooms the whole window." : `Zooms the whole window. Also ${modifierKey} + = / − / 0.`
          }
          controlId={ID.uiScale}
          reset={resetAppearance("ui_scale", "Reset interface scale", newOnlyDisabled)}
          control={
            <Select
              id={ID.uiScale}
              label="Interface scale"
              labelVisibility="hidden"
              aria-describedby={describedBy(ID.uiScale)}
              disabled={newOnlyDisabled}
              value={String(appearance.ui_scale)}
              options={scaleOptions(appearance.ui_scale)}
              onChange={(event) => set({ ui_scale: Number(event.target.value) })}
            />
          }
        />
        <SettingRow
          title="Density"
          description="Spacing and control height in lists, forms and toolbars."
          reset={resetAppearance("density", "Reset density", densityDisabled)}
          control={
            <SegmentedControl
              size="sm"
              label="Density"
              disabled={densityDisabled}
              options={unsupported ? DENSITY_OPTIONS.filter((option) => option.value !== "spacious") : DENSITY_OPTIONS}
              value={appearance.density}
              onValueChange={(density) => set({ density })}
            />
          }
        />
        <SettingRow
          title="Chat text"
          description="Messages and the message box."
          controlId={ID.chatFontSize}
          reset={resetAppearance("chat_font_size", "Reset chat text size", newOnlyDisabled)}
          control={
            <Select
              id={ID.chatFontSize}
              label="Chat text size"
              labelVisibility="hidden"
              aria-describedby={describedBy(ID.chatFontSize)}
              disabled={newOnlyDisabled}
              value={String(appearance.chat_font_size)}
              options={CHAT_FONT_SIZE_OPTIONS}
              onChange={(event) => set({ chat_font_size: Number(event.target.value) })}
            />
          }
        />
        <SettingRow
          title="Code text"
          description="Code blocks, diffs and the terminal."
          controlId={ID.codeFontSize}
          reset={resetAppearance("code_font_size", "Reset code text size", newOnlyDisabled)}
          control={
            <Select
              id={ID.codeFontSize}
              label="Code text size"
              labelVisibility="hidden"
              aria-describedby={describedBy(ID.codeFontSize)}
              disabled={newOnlyDisabled}
              value={String(appearance.code_font_size)}
              options={CODE_FONT_SIZE_OPTIONS}
              onChange={(event) => set({ code_font_size: Number(event.target.value) })}
            />
          }
        />
      </section>
      <section>
        <SectionHeader title="Layout" />
        <SettingRow
          title="Chat width"
          description={chatWidthDescription(appearance.chat_width)}
          reset={resetAppearance("chat_width", "Reset chat width", newOnlyDisabled)}
          control={
            <SegmentedControl
              size="sm"
              label="Chat width"
              disabled={newOnlyDisabled}
              options={CHAT_WIDTH_OPTIONS}
              value={appearance.chat_width}
              onValueChange={(chat_width) => set({ chat_width })}
            />
          }
        />
        <SettingRow
          title="Sidebar width"
          description={`Drag the sidebar edge for any width from ${APPEARANCE_LIMITS.sidebar_width.min} to ${APPEARANCE_LIMITS.sidebar_width.max}px.`}
          controlId={ID.sidebarWidth}
          reset={resetAppearance("sidebar_width", "Reset sidebar width", newOnlyDisabled)}
          control={
            <Select
              id={ID.sidebarWidth}
              label="Sidebar width"
              labelVisibility="hidden"
              aria-describedby={describedBy(ID.sidebarWidth)}
              disabled={newOnlyDisabled}
              value={sidebarValue}
              options={sidebarOptions(sidebarWidth)}
              onChange={(event) =>
                set({ sidebar_width: event.target.value === "auto" ? null : Number(event.target.value) })
              }
            />
          }
        />
        <SettingRow
          title="Collapse sidebar in narrow windows"
          description="When the window is too narrow for the sidebar beside the page, it opens over the page instead."
          controlId={ID.sidebarAutoCollapse}
          reset={resetAppearance("sidebar_auto_collapse", "Reset sidebar collapsing", newOnlyDisabled)}
          control={
            <Switch
              id={ID.sidebarAutoCollapse}
              aria-describedby={describedBy(ID.sidebarAutoCollapse)}
              disabled={newOnlyDisabled}
              checked={appearance.sidebar_auto_collapse}
              onCheckedChange={(sidebar_auto_collapse) => set({ sidebar_auto_collapse })}
            />
          }
        />
        <SettingRow
          title="Message spacing"
          description="Line height and the space between paragraphs in messages."
          reset={resetAppearance("message_spacing", "Reset message spacing", newOnlyDisabled)}
          control={
            <SegmentedControl
              size="sm"
              label="Message spacing"
              disabled={newOnlyDisabled}
              options={MESSAGE_SPACING_OPTIONS}
              value={appearance.message_spacing}
              onValueChange={(message_spacing) => set({ message_spacing })}
            />
          }
        />
      </section>
      <section>
        <SectionHeader title="Interface" />
        <SettingRow
          title="Reduce motion"
          description="Turn off interface transitions and animations."
          controlId={ID.reduceMotion}
          reset={resetWhen(settings.reduce_motion !== PREFERENCE_DEFAULTS.reduce_motion, "Reset reduce motion", () =>
            updatePreferences({ reduce_motion: PREFERENCE_DEFAULTS.reduce_motion }),
          )}
          control={
            <Switch
              id={ID.reduceMotion}
              aria-describedby={describedBy(ID.reduceMotion)}
              checked={settings.reduce_motion}
              onCheckedChange={(reduce_motion) => updatePreferences({ reduce_motion })}
            />
          }
        />
        <SettingRow
          title="Show keyboard hints"
          controlId={ID.keyboardHints}
          reset={resetWhen(settings.keyboard_hints !== PREFERENCE_DEFAULTS.keyboard_hints, "Reset keyboard hints", () =>
            updatePreferences({ keyboard_hints: PREFERENCE_DEFAULTS.keyboard_hints }),
          )}
          control={
            <Switch
              id={ID.keyboardHints}
              checked={settings.keyboard_hints}
              onCheckedChange={(keyboard_hints) => updatePreferences({ keyboard_hints })}
            />
          }
        />
      </section>
    </Stack>
  );
}
