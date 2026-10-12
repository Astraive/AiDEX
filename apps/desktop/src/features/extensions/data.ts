// Live data for the Extensions screen. The page and its sidebar read the same daemon lists, so a
// change made on one (enable a skill, install a plugin) refreshes both through `invalidateExtensions`.
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { call, type RpcMethod, type RpcParams, type RpcResult } from "@aidex/client/rpc";
import { ErrorKind, toAidexError, type AidexError } from "@aidex/client/errors";
import type { Workspace } from "@aidex/client/dto/workspace";
import { useToast } from "../../app/providers/toast";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { useBridgeQuery, type BridgeQuery } from "../../hooks/useBridgeQuery";
import { useRpc } from "../../hooks/useRpc";
import { listSecrets, type StoredSecret } from "./vaultSecrets";

/** A tiny external store: a value plus listeners, read with `useSyncExternalStore`. */
function createStore<T>(initial: T) {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set: (next: T) => {
      value = next;
      for (const listener of listeners) listener();
    },
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

const revision = createStore(0);

/** Refetches every extensions list on screen, e.g. after a change on the page or in a dialog. */
export function invalidateExtensions(): void {
  revision.set(revision.get() + 1);
}

function useRevision(): number {
  return useSyncExternalStore(revision.subscribe, revision.get, revision.get);
}

/**
 * One shared answer per request key (drive and daemon, method, params), so the page, its panels, and
 * the sidebar show the same data, and a poll in one place updates every view of that list.
 */
type Entry = { data?: unknown; error: AidexError | null; loading: boolean };

const PENDING: Entry = { error: null, loading: true };
const entries = new Map<string, Entry>();
const entryListeners = new Map<string, Set<() => void>>();
/** Newest request per key; an older answer that arrives late is dropped. */
const newestRequest = new Map<string, number>();
/** `key|revision` of requests in flight, so simultaneous readers share one call. */
const inflight = new Set<string>();
let requestSequence = 0;

function setEntry(key: string, entry: Entry): void {
  entries.set(key, entry);
  entryListeners.get(key)?.forEach((listener) => listener());
}

function subscribeEntry(key: string, listener: () => void): () => void {
  const listeners = entryListeners.get(key) ?? new Set<() => void>();
  listeners.add(listener);
  entryListeners.set(key, listeners);
  return () => {
    listeners.delete(listener);
  };
}

function load(key: string, rev: number, request: () => Promise<unknown>): void {
  const flight = `${key}|${rev}`;
  if (inflight.has(flight)) return;
  inflight.add(flight);
  const sequence = ++requestSequence;
  newestRequest.set(key, sequence);
  const previous = entries.get(key);
  setEntry(key, { data: previous?.data, error: previous?.error ?? null, loading: true });
  request()
    .then(
      (data) => {
        if (newestRequest.get(key) === sequence) setEntry(key, { data, error: null, loading: false });
      },
      (caught: unknown) => {
        if (newestRequest.get(key) === sequence)
          setEntry(key, { data: entries.get(key)?.data, error: toAidexError(caught), loading: false });
      },
    )
    .finally(() => inflight.delete(flight));
}

export type LiveQuery<T> = {
  /** The last answer for these params; kept while a refetch is in flight so lists don't flicker. */
  data: T | undefined;
  error: AidexError | null;
  loading: boolean;
  refetch: () => void;
};

/**
 * `useRpc` for the Extensions screen: answers are shared by every reader of the same request,
 * refetched on `invalidateExtensions`, kept while reloading, and optionally polled (MCP status, logs).
 */
export function useExtensionsQuery<M extends RpcMethod>(
  method: M,
  params?: RpcParams<M>,
  options: { enabled?: boolean; pollMs?: number } = {},
): LiveQuery<RpcResult<M>> {
  const { connectionKey, daemonRunning } = useAidexStatus();
  const rev = useRevision();
  const [tick, setTick] = useState(0);
  const key = `${connectionKey}|${method}|${JSON.stringify(params ?? null)}`;
  const active = daemonRunning && (options.enabled ?? true);
  const paramsRef = useRef(params);
  paramsRef.current = params;
  const subscribe = useCallback((listener: () => void) => subscribeEntry(key, listener), [key]);
  const snapshot = useCallback(() => entries.get(key) ?? PENDING, [key]);
  const entry = useSyncExternalStore(subscribe, snapshot, snapshot);

  useEffect(() => {
    if (!active) return;
    load(key, rev, () => call(method, paramsRef.current));
  }, [method, key, active, rev, tick]);

  const pollMs = options.pollMs;
  useEffect(() => {
    if (!active || !pollMs) return;
    const timer = setInterval(() => setTick((value) => value + 1), pollMs);
    return () => clearInterval(timer);
  }, [active, pollMs]);

  const refetch = useCallback(() => setTick((value) => value + 1), []);
  return {
    data: active ? (entry.data as RpcResult<M> | undefined) : undefined,
    error: active ? entry.error : null,
    loading: active && entry.loading,
    refetch,
  };
}

/** Calm wording for a failed extensions call. */
export function describeError(error: AidexError): string {
  if (error.kind === ErrorKind.methodNotFound)
    return "This needs a newer AiDEX Core. Update AiDEX to manage skills, MCP servers, plugins, and web search.";
  if (error.kind === ErrorKind.vaultLocked) return "Unlock the drive vault first.";
  return error.message;
}

/** The stable `data.reason` of a daemon error, when it has one. */
export function reasonOf(error: AidexError | null): string | undefined {
  const data = error?.data;
  if (typeof data !== "object" || data === null) return undefined;
  const reason = (data as { reason?: unknown }).reason;
  return typeof reason === "string" ? reason : undefined;
}

export type Action = {
  /** Label of the change in flight, if any. */
  busy: string | null;
  /** Why the last change failed, worded for the screen. */
  error: string | null;
  /** The last failure itself, for branching on its `data.reason`. */
  failure: AidexError | null;
  clearError: () => void;
  /** Runs `job`; on success refreshes the lists and resolves with its result, else with undefined. */
  run: <T>(label: string, job: () => Promise<T>) => Promise<T | undefined>;
};

/** Busy and error state for one control group's changes, so a failure shows next to its control. */
export function useAction(): Action {
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<AidexError | null>(null);
  const run = useCallback(async <T,>(label: string, job: () => Promise<T>): Promise<T | undefined> => {
    setBusy(label);
    setFailure(null);
    try {
      const result = await job();
      invalidateExtensions();
      return result;
    } catch (caught) {
      setFailure(toAidexError(caught));
      return undefined;
    } finally {
      setBusy(null);
    }
  }, []);
  const clearError = useCallback(() => setFailure(null), []);
  return { busy, error: failure ? describeError(failure) : null, failure, clearError, run };
}

/**
 * Opens a link from a skill, plugin README, or search result in the system browser. The daemon
 * validates the URL (https only) before anything launches; a refusal shows as a toast.
 */
export function useOpenLink(): (url: string) => void {
  const notify = useToast();
  return useCallback(
    (url: string) => {
      call("web.open", { url }).catch((caught: unknown) => {
        notify(`Could not open the link: ${describeError(toAidexError(caught))}`, "danger");
      });
    },
    [notify],
  );
}

/**
 * Names and allowed consumers of the vault's secrets (never values), refreshed with the lists.
 * Fails with `vault_locked` while the vault is locked.
 */
export function useVaultSecrets(): BridgeQuery<Map<string, StoredSecret>> {
  const { connectionKey, daemonRunning } = useAidexStatus();
  const rev = useRevision();
  return useBridgeQuery(listSecrets, `${connectionKey}|vault.secret.list|${rev}`, daemonRunning);
}

/** The skill, server, and plugin lists, for tab counts and sidebar shortcuts. */
export function useExtensionLists() {
  const { workspaceId } = useWorkspaceScope();
  const skills = useExtensionsQuery("skill.list", workspaceId ? { workspace_id: workspaceId } : undefined);
  const servers = useExtensionsQuery("mcp.list");
  const plugins = useExtensionsQuery("plugin.list");
  return {
    skills: skills.data?.skills,
    servers: servers.data?.servers,
    plugins: plugins.data?.plugins,
  };
}

/** `null` follows the globally selected workspace; `""` means no workspace; otherwise a workspace id. */
const workspaceChoice = createStore<string | null>(null);

export type WorkspaceScope = {
  /** Workspaces that can be chosen (available ones only). */
  workspaces: readonly Workspace[];
  /** The workspace whose project and repo skills are shown, if any. */
  workspaceId: string | undefined;
  workspace: Workspace | undefined;
  choose: (id: string) => void;
};

/** The workspace the Skills tab (and the sidebar counts) read project and repo skills from. */
export function useWorkspaceScope(): WorkspaceScope {
  const choice = useSyncExternalStore(workspaceChoice.subscribe, workspaceChoice.get, workspaceChoice.get);
  const listing = useRpc("workspace.list");
  return useMemo(() => {
    const workspaces = (listing.data?.workspaces ?? []).filter(
      (workspace) => workspace.availability.state === "available",
    );
    const wanted = choice === null ? (listing.data?.selected ?? "") : choice;
    const workspace = workspaces.find((entry) => entry.id === wanted);
    return {
      workspaces,
      workspaceId: workspace?.id,
      workspace,
      choose: (id: string) => workspaceChoice.set(id),
    };
  }, [choice, listing.data]);
}
