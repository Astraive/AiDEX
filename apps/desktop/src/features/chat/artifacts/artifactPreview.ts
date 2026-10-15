// What the artifact viewer holds for the artifact it shows, and when a vault lock takes it off the
// screen, as pure functions so the rules are tested without a daemon. The viewer reads content with
// imperative `artifact.read` calls rather than `useRpc`, so it follows vault locks itself: content
// read before the vault's latest lock (`useVaultLocks`) is never shown, and a read refused as
// `vault_locked` keeps nothing.
import { ErrorKind } from "@aidex/client/errors";
import type { ArtifactData } from "./artifactContent";

/**
 * How often a view the vault locked out reads again while it stays open (through `usePoll`, so only
 * while the user is present); each try is refused at once while the vault stays locked.
 */
export const LOCKED_RETRY_MS = 5000;

/** What the viewer shows: still reading, the preview, locked out by the vault, or another failure. */
export type PreviewView =
  | { status: "loading" }
  | { status: "ready"; data: ArtifactData }
  | { status: "locked" }
  | { status: "failed"; message: string };

/** The viewer's state: what it holds for artifact `id`, read after the vault had locked `locks` times. */
export type PreviewState = PreviewView & { id: string; locks: number };

/** Nothing read yet. */
export const EMPTY_PREVIEW: PreviewState = { id: "", locks: 0, status: "loading" };

/**
 * A read of `id` starts while the vault has locked `locks` times. A new artifact starts empty. For the
 * same artifact, content read before a later lock is dropped at once, and a preview the vault locked
 * out stays locked out (no "Loading" flicker on each retry) until the read answers.
 */
export function previewStarted(previous: PreviewState, id: string, locks: number): PreviewState {
  if (previous.id === id && (previous.status === "locked" || previous.locks !== locks))
    return { id, locks, status: "locked" };
  return { id, locks, status: "loading" };
}

/** The read of `id`, started after `locks` vault locks, answered with `data`. */
export function previewRead(id: string, locks: number, data: ArtifactData): PreviewState {
  return { id, locks, status: "ready", data };
}

/** The read of `id`, started after `locks` vault locks, failed: a refusal as locked keeps nothing. */
export function previewFailed(id: string, locks: number, error: { kind?: string; message: string }): PreviewState {
  return error.kind === ErrorKind.vaultLocked
    ? { id, locks, status: "locked" }
    : { id, locks, status: "failed", message: error.message };
}

/**
 * What the viewer shows for `id` while the vault has locked `locks` times: nothing held for another
 * artifact, and nothing read before the latest lock, which counts as locked out until a read after
 * it answers. Decrypted content therefore leaves the screen in the render that sees the lock, before
 * any re-read.
 */
export function shownPreview(state: PreviewState, id: string, locks: number): PreviewView {
  if (state.id !== id) return { status: "loading" };
  if (state.locks !== locks) return { status: "locked" };
  switch (state.status) {
    case "ready":
      return { status: "ready", data: state.data };
    case "failed":
      return { status: "failed", message: state.message };
    default:
      return { status: state.status };
  }
}
