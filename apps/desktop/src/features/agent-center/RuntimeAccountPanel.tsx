import { useEffect, useRef, useState } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import { Button, Stack, Text, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { RuntimeLoginStatus } from "@aidex/client/dto/runtimes";
import { useRpc } from "../../hooks/useRpc";

export function RuntimeAccountPanel({
  id,
  onChanged,
  onPendingChange,
}: {
  id: string;
  onChanged: () => Promise<void>;
  onPendingChange: (pending: boolean) => void;
}) {
  const account = useRpc("runtime.account.status", { id });
  const [login, setLogin] = useState<RuntimeLoginStatus>();
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const pending = login?.state === "pending";
  useEffect(() => {
    onPendingChange(pending || busy);
    return () => onPendingChange(false);
  }, [pending, busy, onPendingChange]);
  useEffect(() => {
    if (!login || login.state !== "pending") return;
    let closed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const status = await call("runtime.login.status", {
          id,
          flow_id: login.flow_id,
        });
        if (closed) return;
        setLogin(status);
        if (status.state !== "pending") {
          account.refetch();
          if (status.state === "connected") {
            setBusy(true);
            try {
              await changed.current();
            } finally {
              setBusy(false);
            }
          }
          return;
        }
      } catch (caught) {
        if (!closed) setError(toAidexError(caught).message);
      }
      if (!closed) timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => {
      closed = true;
      clearTimeout(timer);
    };
  }, [id, login?.flow_id, login?.state, account.refetch]);
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      account.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Stack gap={4}>
      <SectionHeader title="Official Codex account" />
      <Text>
        The installed Codex CLI manages browser sign-in in an isolated home.
        Account files are encrypted on the portable drive.
      </Text>
      {(error || account.error) && (
        <Notice tone="danger">{error ?? account.error?.message}</Notice>
      )}
      {account.data && (
        <Notice>
          {account.data.status} · Credential{" "}
          {account.data.credential_present ? "present" : "absent"} · Official
          login status {account.data.authenticated ? "verified" : "unverified"}{" "}
          · Profile {account.data.profile ?? "none"}. {account.data.evidence}
        </Notice>
      )}
      <Text tone="muted">
        Credential and login-status evidence is cached account state. Live
        inference eligibility still requires an actual request.
      </Text>
      <Stack direction="row" gap={2} wrap>
        <Button
          disabled={busy || pending}
          onClick={() =>
            void act(async () => {
              const status = await call("runtime.login.start", { id });
              setLogin(status);
              if (status.state === "connected") await changed.current();
            })
          }
        >
          Sign in through Codex CLI
        </Button>
        <Button
          variant="outline"
          disabled={busy || pending}
          onClick={account.refetch}
        >
          Check official login status
        </Button>
        {login && pending && (
          <Button
            variant="outline"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                setLogin(
                  await call("runtime.login.cancel", {
                    id,
                    flow_id: login.flow_id,
                  }),
                );
              })
            }
          >
            Cancel CLI sign-in
          </Button>
        )}
        <Button
          variant="ghost"
          disabled={busy || pending || !account.data?.credential_present}
          onClick={() => {
            if (
              window.confirm(
                "Remove this runtime's saved account profile and native session snapshots?",
              )
            )
              void act(async () => {
                await call("runtime.login.logout", { id });
                setLogin(undefined);
                await changed.current();
              });
          }}
        >
          Log out and clear saved profile
        </Button>
      </Stack>
      {login && (
        <Notice tone={login.state === "failed" ? "danger" : "info"}>
          {login.state} · {login.message}
          {login.error_code ? ` · ${login.error_code}` : ""}
        </Notice>
      )}
      {pending && <Text>Continue in the browser opened by Codex.</Text>}
      {pending && login?.authorization_url && (
        <a href={login.authorization_url} target="_blank" rel="noreferrer">
          Continue the pending Codex sign-in
        </a>
      )}
      <TextField
        label="Explicit auth.json file or containing directory"
        value={source}
        disabled={busy || pending}
        onChange={(event) => setSource(event.target.value)}
        hint="Enter the absolute path yourself. Only that credential file is imported; host-global credentials are never discovered automatically."
      />
      <Button
        variant="outline"
        disabled={busy || pending || !source.trim()}
        onClick={() =>
          void act(async () => {
            await call("runtime.profile.import", { id, source: source.trim() });
            setSource("");
            await changed.current();
          })
        }
      >
        Import this native account file
      </Button>
    </Stack>
  );
}
