import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { Dialog, InlineCode, KeyValueList, Notice, SettingsSection, safeExternalUrl } from "@aidex/ui/components";
import { ExternalLinkIcon, RefreshIcon, UploadIcon, UserIcon } from "@aidex/ui/icons";
import { Button, Stack, StatusIndicator, Text, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import type { RuntimeAccountStatus, RuntimeLoginStatus } from "@aidex/client/dto/runtime";
import { useRpc } from "../../../hooks/useRpc";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import type { RuntimeItem } from "./providerItems";
import { shouldOpenSignInPage } from "./runtimeLogin";

/** Driver auth methods that mean AiDEX manages the runtime's own account (`runtime.login.*`). */
const MANAGED_METHODS = ["native_cli", "profile_import"];

/** Whether AiDEX can sign a runtime in with its official CLI or import its account file. */
export function managesAccount(item: RuntimeItem): boolean {
  return !!item.driver?.auth_methods.some((method) => MANAGED_METHODS.includes(method));
}

const STATUS: Record<string, { label: string; tone: "success" | "warning" | "danger" | "neutral" }> = {
  authenticated_cached: { label: "Signed in", tone: "success" },
  auth_pending: { label: "Signing in", tone: "warning" },
  auth_required: { label: "Not signed in", tone: "warning" },
  signed_out: { label: "Signed out", tone: "neutral" },
};

function reasonOf(caught: unknown): string | undefined {
  const data = toAidexError(caught).data;
  if (typeof data !== "object" || data === null) return undefined;
  const reason = (data as Record<string, unknown>).reason;
  return typeof reason === "string" ? reason : undefined;
}

/** The driver has no managed account, or this AiDEX Core predates `runtime.account.*` and `runtime.login.*`. */
function accountUnsupported(caught: unknown): boolean {
  return reasonOf(caught) === "auth_unsupported" || toAidexError(caught).is(ErrorKind.methodNotFound);
}

/** How often a pending sign-in is checked, and how many failed checks in a row end it. */
const POLL_MS = 1000;
const MAX_POLL_FAILURES = 3;

/** A failed check that retrying cannot fix: the vault locked, or the flow or runtime is gone. */
function pollFailureIsTerminal(caught: unknown): boolean {
  const error = toAidexError(caught);
  return error.is(ErrorKind.vaultLocked) || error.code === "not_found";
}

type Props = {
  item: RuntimeItem;
  /**
   * Told when an import, sign-in, or sign-out starts and settles: AiDEX Core binds or unbinds the
   * runtime's account when one finishes, so the editor holds back other configuration writes meanwhile.
   */
  onBusyChange?: (busy: boolean) => void;
};

/**
 * The runtime's own account, kept encrypted on the drive (Codex): status, sign-in through the
 * runtime's official CLI, import of an `auth.json` the user names, and sign-out. Credential bytes
 * never reach this screen. Other drivers say AiDEX does not manage their account.
 */
export function RuntimeAccountSection({ item, onBusyChange }: Props) {
  if (!managesAccount(item))
    return (
      <SettingsSection title="Account" icon={<UserIcon />}>
        <Text tone="muted">
          AiDEX manages the account of Codex only. {item.title} uses the sign-in above, or its own sign-in on this
          computer.
        </Text>
      </SettingsSection>
    );
  return <ManagedAccount item={item} onBusyChange={onBusyChange} />;
}

function ManagedAccount({ item, onBusyChange }: Props) {
  const catalog = useProviderCatalog();
  const { runtime } = item;
  const id = runtime.id;
  const account = useRpc("runtime.account.status", { id }, { enabled: runtime.enabled });
  const [busy, setBusy] = useState<"login" | "import" | "logout" | null>(null);
  const [error, setError] = useState<string>();
  const [unsupported, setUnsupported] = useState(false);
  const [source, setSource] = useState("");
  const [confirming, setConfirming] = useState(false);
  const refetchAccount = account.refetch;
  const checkRuntime = catalog.checkRuntime;
  const reloadCatalog = catalog.reload;
  // A sign-in, import, or sign-out changes the runtime's configuration (its profile) and status.
  const changed = useCallback(() => {
    refetchAccount();
    reloadCatalog();
    void checkRuntime(id);
  }, [refetchAccount, reloadCatalog, checkRuntime, id]);
  const markUnsupported = useCallback(() => setUnsupported(true), []);
  const login = useRuntimeLogin(id, changed, markUnsupported);
  const accountError = account.error;
  useEffect(() => {
    if (accountError && accountUnsupported(accountError)) setUnsupported(true);
  }, [accountError]);
  const working = busy !== null || login.pending || login.starting;
  const onBusyChangeRef = useRef(onBusyChange);
  onBusyChangeRef.current = onBusyChange;
  useEffect(() => {
    onBusyChangeRef.current?.(working);
  }, [working]);
  useEffect(() => () => onBusyChangeRef.current?.(false), []);

  const act = async (kind: "import" | "logout", operation: () => Promise<unknown>) => {
    setBusy(kind);
    setError(undefined);
    try {
      await operation();
      changed();
      return true;
    } catch (caught) {
      if (accountUnsupported(caught)) setUnsupported(true);
      setError(toAidexError(caught).message);
      return false;
    } finally {
      setBusy(null);
    }
  };

  const importFile = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const path = source.trim();
    if (!path || busy || login.pending) return;
    if (await act("import", () => call("runtime.profile.import", { id, source: path }))) setSource("");
  };

  if (unsupported)
    return (
      <SettingsSection title="Account" icon={<UserIcon />}>
        <Text tone="muted">
          This version of AiDEX Core cannot manage the {item.title} account. It uses the sign-in above.
        </Text>
      </SettingsSection>
    );

  const status: RuntimeAccountStatus | undefined = account.data;
  const shown = status ? (STATUS[status.status] ?? { label: status.status, tone: "neutral" as const }) : null;
  const locked = busy !== null || login.pending;
  // Until it has a profile, the runtime uses the sign-in in Setup; a saved account replaces it.
  const setupSignIn = !runtime.profile && runtime.auth.kind !== "none";
  return (
    <SettingsSection
      title="Account"
      icon={<UserIcon />}
      description="Sign in with the Codex CLI. AiDEX keeps the account encrypted on this drive and gives it only to this runtime."
      actions={
        <Button
          variant="outline"
          icon={<RefreshIcon />}
          disabled={!runtime.enabled || account.loading || locked}
          onClick={refetchAccount}
        >
          {account.loading ? "Checking…" : "Check account"}
        </Button>
      }
    >
      {!runtime.enabled ? (
        <Text tone="muted">Enable this runtime to manage its account.</Text>
      ) : account.error ? (
        <Notice tone="danger">{account.error.message}</Notice>
      ) : status && shown ? (
        <>
          <KeyValueList
            items={[
              { label: "Status", value: <StatusIndicator tone={shown.tone} label={shown.label} /> },
              {
                label: "Saved account",
                value: status.credential_present ? "Encrypted on this drive" : "None",
              },
              {
                label: "Codex login check",
                value: status.authenticated ? "Passed" : "Not verified",
              },
              ...(status.profile ? [{ label: "Profile", value: <InlineCode>{status.profile}</InlineCode> }] : []),
            ]}
          />
          <Text tone="muted">
            Signed in means Codex has saved credentials and its own login check passed. A request can still fail if
            the account cannot use the model.
          </Text>
        </>
      ) : (
        <Text tone="muted">Checking the account…</Text>
      )}
      {runtime.enabled && setupSignIn && (
        <Notice tone="warning">
          {item.title} uses the sign-in in Setup above for now. Signing in with Codex or importing an account file
          replaces it: from then on AiDEX gives this runtime only the account saved here.
        </Notice>
      )}
      {login.state && <LoginProgress login={login.state} onOpen={login.openPage} />}
      {(error || login.error) && (
        <Notice tone="danger" role="alert">
          {error ?? login.error}
        </Notice>
      )}
      <Stack direction="row" gap={2} wrap>
        {login.pending ? (
          <Button variant="outline" onClick={() => void login.cancel()}>
            Cancel sign-in
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={!runtime.enabled || busy !== null || login.starting}
            onClick={() => void login.start()}
          >
            {login.starting ? "Starting…" : status?.credential_present ? "Sign in again" : "Sign in with Codex"}
          </Button>
        )}
        {status?.credential_present && (
          <Button variant="ghost" disabled={locked} onClick={() => setConfirming(true)}>
            Sign out
          </Button>
        )}
      </Stack>
      <form onSubmit={(event) => void importFile(event)} noValidate aria-label={`Import a ${item.title} account`}>
        <Stack gap={3}>
          <TextField
            label="Import an account file"
            size="md"
            spellCheck={false}
            autoComplete="off"
            hint="The absolute path of a Codex auth.json, or its folder. Only that file is read; AiDEX never looks for credentials on its own."
            value={source}
            disabled={!runtime.enabled || locked}
            onChange={(event) => setSource(event.target.value)}
          />
          <Stack direction="row" gap={2}>
            <Button
              type="submit"
              variant="outline"
              icon={<UploadIcon />}
              disabled={!runtime.enabled || locked || !source.trim()}
            >
              {busy === "import" ? "Importing…" : "Import"}
            </Button>
          </Stack>
        </Stack>
      </form>
      <Dialog open={confirming} title={`Sign out of ${item.title}?`} onClose={() => setConfirming(false)}>
        <Stack gap={4}>
          <Text>
            AiDEX deletes the saved account and this runtime's saved conversation state. Conversations stay, and their
            next message starts a new Codex session.
          </Text>
          {error && (
            <Notice tone="danger" role="alert">
              {error}
            </Notice>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={busy !== null}
              onClick={() =>
                void act("logout", () => call("runtime.login.logout", { id })).then((done) => {
                  if (done) setConfirming(false);
                })
              }
            >
              {busy === "logout" ? "Signing out…" : "Sign out"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </SettingsSection>
  );
}

function LoginProgress({ login, onOpen }: { login: RuntimeLoginStatus; onOpen: () => void }) {
  if (login.state === "pending")
    return (
      <Notice tone="info" role="status">
        <Stack gap={3} align="start">
          <Text as="span">
            {login.authorization_url
              ? "Finish signing in on the sign-in page AiDEX opened in your browser. This page updates when you are done."
              : "Starting the sign-in. AiDEX opens the sign-in page in your browser when Codex provides it."}
          </Text>
          {login.authorization_url && (
            <Button variant="outline" size="md" icon={<ExternalLinkIcon />} onClick={onOpen}>
              Open the sign-in page again
            </Button>
          )}
        </Stack>
      </Notice>
    );
  if (login.state === "connected")
    return (
      <Notice tone="success" role="status">
        Signed in. The account is saved encrypted on this drive.
      </Notice>
    );
  if (login.state === "cancelled")
    return (
      <Notice role="status">Sign-in cancelled. The account saved before, if any, is unchanged.</Notice>
    );
  return (
    <Notice tone="danger" role="alert">
      {login.message || "Sign-in failed."} The account saved before, if any, is unchanged.
    </Notice>
  );
}

/**
 * A sign-in run by the runtime's official CLI (`runtime.login.start`), checked about every second,
 * one check at a time, until it connects, fails, or is cancelled; `onSettled` then refreshes the
 * account and the runtime. A failed check is retried; the flow is cancelled only after
 * `MAX_POLL_FAILURES` failures in a row or one retrying cannot fix (vault locked, flow gone).
 * `onUnsupported` reports an AiDEX Core without `runtime.login.*`. Leaving the screen cancels a
 * pending sign-in, as provider sign-ins do, so no CLI process outlives it. The sign-in page (an
 * OpenAI address the adapter checked) opens once per flow, as soon as the flow reports it, through
 * `web.open`: AiDEX Core starts the browser outside the sandbox job that runs the CLI, so ending the
 * sign-in never closes it. "Open the sign-in page again" opens it once more.
 */
function useRuntimeLogin(id: string, onSettled: () => void, onUnsupported: () => void) {
  const [state, setState] = useState<RuntimeLoginStatus | null>(null);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string>();
  const onSettledRef = useRef(onSettled);
  onSettledRef.current = onSettled;
  const onUnsupportedRef = useRef(onUnsupported);
  onUnsupportedRef.current = onUnsupported;
  const flowId = state?.state === "pending" ? state.flow_id : null;
  const pendingRef = useRef<string | null>(null);
  pendingRef.current = flowId;
  // The flow whose sign-in page was opened automatically, so it opens once.
  const openedFor = useRef<string | null>(null);
  // Bumped by every start, cancel, and unmount, so a start that resolves late cancels its own flow.
  const attemptRef = useRef(0);
  const mountedRef = useRef(true);

  const finish = (status: RuntimeLoginStatus) => {
    pendingRef.current = null;
    setState(status);
    if (status.state !== "pending") onSettledRef.current();
  };

  useEffect(() => {
    if (!flowId) return;
    let live = true;
    let failures = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // Each check schedules the next only once it settles, so checks never overlap.
    const next = () => {
      timer = setTimeout(() => void check(), POLL_MS);
    };
    const check = async () => {
      try {
        const status = await call("runtime.login.status", { id, flow_id: flowId });
        if (!live) return;
        failures = 0;
        if (status.state === "pending") {
          // The authorization URL appears once the CLI prints it.
          setState(status);
          next();
          return;
        }
        live = false;
        finish(status);
      } catch (caught) {
        if (!live) return;
        failures += 1;
        if (failures < MAX_POLL_FAILURES && !pollFailureIsTerminal(caught)) {
          next();
          return;
        }
        // Polling stops here, so end the flow too; a pending one refuses Codex turns.
        live = false;
        pendingRef.current = null;
        void call("runtime.login.cancel", { id, flow_id: flowId }).catch(() => undefined);
        setState(null);
        setError(toAidexError(caught).message);
      }
    };
    next();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [id, flowId]);

  useEffect(() => {
    mountedRef.current = true; // StrictMode runs this effect twice
    return () => {
      mountedRef.current = false;
      attemptRef.current += 1;
      const pending = pendingRef.current;
      if (pending) void call("runtime.login.cancel", { id, flow_id: pending }).catch(() => undefined);
    };
  }, [id]);

  const start = async () => {
    attemptRef.current += 1;
    const attempt = attemptRef.current;
    setStarting(true);
    setError(undefined);
    setState(null);
    try {
      const status = await call("runtime.login.start", { id });
      if (attempt !== attemptRef.current || !mountedRef.current) {
        if (status.state === "pending")
          await call("runtime.login.cancel", { id, flow_id: status.flow_id }).catch(() => undefined);
        return;
      }
      if (status.state === "pending") {
        pendingRef.current = status.flow_id;
        setState(status);
      } else finish(status);
    } catch (caught) {
      if (attempt !== attemptRef.current || !mountedRef.current) return;
      if (accountUnsupported(caught)) onUnsupportedRef.current();
      setError(toAidexError(caught).message);
    } finally {
      if (attempt === attemptRef.current && mountedRef.current) setStarting(false);
    }
  };

  const cancel = async () => {
    attemptRef.current += 1;
    setStarting(false);
    const pending = pendingRef.current;
    pendingRef.current = null;
    if (!pending) return;
    try {
      finish(await call("runtime.login.cancel", { id, flow_id: pending }));
    } catch (caught) {
      setError(toAidexError(caught).message);
      setState(null);
    }
  };

  const openUrl = (address: string | null | undefined) => {
    const url = address ? safeExternalUrl(address, { allowMailto: false }) : null;
    if (!url) {
      setError("The sign-in page address could not be opened.");
      return;
    }
    call("web.open", { url }).catch((caught) => setError(toAidexError(caught).message));
  };
  const openPage = () => openUrl(state?.authorization_url);

  // Open the sign-in page as soon as the pending flow reports it.
  const autoOpen = state?.state === "pending" && state.authorization_url ? state : null;
  useEffect(() => {
    if (!autoOpen || !shouldOpenSignInPage(openedFor.current, autoOpen)) return;
    openedFor.current = autoOpen.flow_id;
    openUrl(autoOpen.authorization_url);
    // `openUrl` only reads its argument and stable setters.
  }, [autoOpen?.flow_id, autoOpen?.authorization_url]);

  return { state, starting, pending: !!flowId, error, start, cancel, openPage };
}
