// Keyboard chords written as "Mod+Shift+p": modifiers, then one key, joined by "+". `Mod` is ⌘ on
// Apple platforms and Ctrl elsewhere. Matching ignores letter case and requires the exact modifiers.

export type KeyChord = {
  mod: boolean;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
  /** Lowercase `KeyboardEvent.key`, e.g. "p", ",", "arrowup". */
  key: string;
};

type Modifier = "mod" | "ctrl" | "meta" | "alt" | "shift";

const MODIFIERS: Record<string, Modifier> = {
  mod: "mod",
  cmd: "meta",
  command: "meta",
  meta: "meta",
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  shift: "shift",
};

/** Parses a chord; null when it is empty, names an unknown modifier, or has no key. */
export function parseKeyChord(chord: string): KeyChord | null {
  const parts = chord
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  const key = parts.pop()?.toLowerCase();
  if (!key || key in MODIFIERS) return null;
  const parsed: KeyChord = { mod: false, ctrl: false, meta: false, alt: false, shift: false, key };
  for (const part of parts) {
    const modifier = MODIFIERS[part.toLowerCase()];
    if (!modifier) return null;
    parsed[modifier] = true;
  }
  return parsed;
}

type KeyEventLike = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">;

export function matchesKeyChord(event: KeyEventLike, chord: KeyChord, apple: boolean): boolean {
  const meta = chord.meta || (chord.mod && apple);
  const ctrl = chord.ctrl || (chord.mod && !apple);
  if (
    event.metaKey !== meta ||
    event.ctrlKey !== ctrl ||
    event.altKey !== chord.alt ||
    event.shiftKey !== chord.shift
  )
    return false;
  // Synthetic events (e.g. autofill) can arrive without a key.
  if (event.key?.toLowerCase() === chord.key) return true;
  // Layouts and Alt can change `key`; fall back to the physical letter or digit.
  if (/^[a-z]$/.test(chord.key)) return event.code === `Key${chord.key.toUpperCase()}`;
  if (/^[0-9]$/.test(chord.key)) return event.code === `Digit${chord.key}`;
  return false;
}

/** Key caps for a chord, e.g. ["Ctrl", "Shift", "M"], with `Mod` shown as `modifierLabel`. */
export function keyChordParts(chord: string, modifierLabel: string): string[] {
  const parsed = parseKeyChord(chord);
  if (!parsed) return [chord];
  return [
    ...(parsed.mod ? [modifierLabel] : []),
    ...(parsed.ctrl ? ["Ctrl"] : []),
    ...(parsed.meta ? ["⌘"] : []),
    ...(parsed.alt ? ["Alt"] : []),
    ...(parsed.shift ? ["Shift"] : []),
    parsed.key.length === 1 ? parsed.key.toUpperCase() : parsed.key[0].toUpperCase() + parsed.key.slice(1),
  ];
}

/** A chord as one label, e.g. "Ctrl+Shift+M" or "⌘+Shift+M". */
export function formatKeyChord(chord: string, modifierLabel: string): string {
  return keyChordParts(chord, modifierLabel).join("+");
}
