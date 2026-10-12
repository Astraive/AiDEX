import { DataTable, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { Kbd, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { usePreferences } from "../../app/providers/preferences";
import { useModifierKeyLabel } from "../../hooks/useModifierKeyLabel";
import { keyChordParts } from "../../lib/keyChord";
import {
  DEFAULT_NEW_SESSION_SHORTCUT,
  DEFAULT_PROVIDERS_SHORTCUT,
  MODEL_PICKER_SHORTCUT,
} from "../../lib/shortcuts";
import { plannedShortcuts, type Shortcut } from "./sample";

/** Keyboard shortcuts, marking which ones this build already handles. */
export function ShortcutsPanel() {
  const modifierKey = useModifierKeyLabel();
  const { settings } = usePreferences();
  const shortcuts: readonly Shortcut[] = [
    {
      id: "new-session",
      action: "New chat",
      chord: settings.shortcuts?.new_session || DEFAULT_NEW_SESSION_SHORTCUT,
      wired: true,
    },
    { id: "model-picker", action: "Choose a model (in a conversation)", chord: MODEL_PICKER_SHORTCUT, wired: true },
    {
      id: "providers",
      action: "Open Providers",
      chord: settings.shortcuts?.providers || DEFAULT_PROVIDERS_SHORTCUT,
      wired: true,
    },
    ...plannedShortcuts,
  ];

  const columns: readonly DataTableColumn<Shortcut>[] = [
    { key: "action", header: "Action", rowHeader: true },
    {
      key: "keys",
      header: "Keys",
      render: (shortcut) => (
        <Stack direction="row" gap={1} align="center">
          {keyChordParts(shortcut.chord, modifierKey).map((key) => (
            <Kbd key={key}>{key}</Kbd>
          ))}
        </Stack>
      ),
    },
    {
      key: "status",
      header: "Status",
      render: (shortcut) => (
        <StatusIndicator tone={shortcut.wired ? "success" : "neutral"} label={shortcut.wired ? "Works today" : "Planned"} />
      ),
    },
  ];

  return (
    <section>
      <SectionHeader title="Keyboard shortcuts" meta={`${shortcuts.filter((s) => s.wired).length} of ${shortcuts.length} active`} />
      <Stack gap={4}>
        <DataTable caption="Keyboard shortcuts" hideCaption columns={columns} rows={shortcuts} rowKey={(row) => row.id} />
        <Text tone="muted">
          {`While the model picker is open, ${modifierKey}+1 to ${modifierKey}+9 choose a listed model and ${modifierKey}+Shift+Up or Down switch providers. Planned shortcuts start working as each screen is wired up.`}
        </Text>
      </Stack>
    </section>
  );
}
