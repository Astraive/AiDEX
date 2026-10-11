import { useEffect, useState, type FormEvent } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import {
  Button,
  PasswordField,
  Select,
  Stack,
  Switch,
  Text,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type {
  AuthStart,
  RuntimeConfig,
  RuntimeDriverInfo,
  RuntimeHealth,
  RuntimeModel,
} from "@aidex/client/dto/runtimes";
import { usePreferences } from "../../app/providers/preferences";
import { RuntimeAccountPanel } from "./RuntimeAccountPanel";

const defaultEnv: Record<string, string> = {
  codex: "OPENAI_API_KEY",
  claude_code: "ANTHROPIC_API_KEY",
  cursor: "CURSOR_API_KEY",
  grok: "XAI_API_KEY",
  antigravity: "GEMINI_API_KEY",
  muse: "META_API_KEY",
};
export function RuntimeConfigForm({
  initial,
  catalog,
  onSaved,
}: {
  initial?: RuntimeConfig;
  catalog: RuntimeDriverInfo[];
  onSaved: (id: string) => void;
}) {
  const preferences = usePreferences();
  const [config, setConfig] = useState<RuntimeConfig>(
    () =>
      initial ?? {
        id: "",
        driver: catalog[0]?.driver ?? "codex",
        executable: "",
        argv: [],
        profile: null,
        enabled: true,
        auth: { kind: "none" },
      },
  );
  const [argv, setArgv] = useState(() => JSON.stringify(config.argv, null, 2));
  const [credential, setCredential] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [health, setHealth] = useState<RuntimeHealth>();
  const [models, setModels] = useState<RuntimeModel[]>();
  const [modelId, setModelId] = useState(() =>
    preferences.settings.runtime_id === initial?.id
      ? (preferences.settings.runtime_model_id ?? "")
      : "",
  );
  const [flow, setFlow] = useState<AuthStart>();
  const [authState, setAuthState] = useState<string>();
  const [cliPending, setCliPending] = useState(false);
  const selected = catalog.find((entry) => entry.driver === config.driver);
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      await operation();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (!flow || !initial) return;
    let mounted = true;
    const check = async () => {
      try {
        const status = await call("runtime.auth.status", {
          id: initial.id,
          flow_id: flow.flow_id,
        });
        if (!mounted) return;
        setAuthState(
          `${status.state}${status.account_label ? ` · ${status.account_label}` : ""}`,
        );
        if (status.error) setError(status.error);
        if (status.state !== "pending") {
          setFlow(undefined);
          setHealth(undefined);
        }
      } catch (caught) {
        if (mounted) setError(toAidexError(caught).message);
      }
    };
    void check();
    const timer = setInterval(() => void check(), 1000);
    return () => {
      mounted = false;
      clearInterval(timer);
    };
  }, [flow, initial?.id]);
  const save = (event: FormEvent) => {
    event.preventDefault();
    void act(async () => {
      if (flow || cliPending)
        throw new Error(
          "Cancel the pending sign-in before changing this runtime configuration.",
        );
      if (config.id === "native")
        throw new Error("The instance ID native is reserved for AiDEX Native.");
      const parsed: unknown = JSON.parse(argv);
      if (
        !Array.isArray(parsed) ||
        parsed.some((item) => typeof item !== "string")
      )
        throw new Error("Launcher arguments must be a JSON array of strings.");
      if (config.auth.kind === "vault" && credential) {
        await call("vault.secret.set", {
          name: config.auth.secret_name,
          value: credential,
          consumers: [`process:runtime-${config.id}`],
          kind: "api_key",
        });
        setCredential("");
      }
      const saved = await call(initial ? "runtime.update" : "runtime.add", {
        ...config,
        argv: parsed as string[],
      });
      setConfig(saved);
      setHealth(undefined);
      setModels(undefined);
      setNotice("Runtime configuration saved.");
      onSaved(saved.id);
    });
  };
  return (
    <Stack gap={5}>
      {error && <Notice tone="danger">{error}</Notice>}
      {notice && <Notice tone="success">{notice}</Notice>}
      <form onSubmit={save}>
        <fieldset
          disabled={busy || cliPending || !!flow}
          style={{ border: 0, padding: 0, margin: 0 }}
        >
          <Stack gap={4}>
            <TextField
              label="Instance ID"
              required
              disabled={!!initial || busy}
              maxLength={64}
              pattern="[A-Za-z0-9_-]+"
              value={config.id}
              onChange={(event) =>
                setConfig({ ...config, id: event.target.value })
              }
              hint="Use a separate instance for each account or workspace."
            />
            <Select
              label="Runtime driver"
              disabled={busy}
              value={config.driver}
              options={catalog.map((entry) => ({
                value: entry.driver,
                label: entry.name,
              }))}
              onChange={(event) =>
                setConfig({
                  ...config,
                  driver: event.target.value as RuntimeConfig["driver"],
                  auth: { kind: "none" },
                })
              }
            />
            <Text tone="muted">
              {selected?.protocol} · Adapter arguments:{" "}
              {selected?.command.join(" ") || "none"}
            </Text>
            {selected && (
              <Notice>
                {selected.supports_mcp_bridge === true
                  ? "Scoped AiDEX MCP bridge is supported by this adapter."
                  : selected.supports_mcp_bridge === false
                    ? "Scoped AiDEX MCP bridge is unavailable for this adapter; its official integration has not been verified."
                    : "This Core has not reported the adapter's scoped MCP bridge capability."}{" "}
                External vendor tools remain owned by the runtime.
              </Notice>
            )}
            <TextField
              label="Executable path"
              required
              value={config.executable}
              onChange={(event) =>
                setConfig({ ...config, executable: event.target.value })
              }
              hint="Absolute path or path relative to the portable drive. Do not paste a shell command."
            />
            <TextArea
              label="Literal launcher arguments (JSON array)"
              rows={3}
              monospace
              value={argv}
              onChange={(event) => setArgv(event.target.value)}
              hint="These precede the driver's adapter arguments. Keep credentials in the vault."
            />
            <TextField
              label="Managed profile ID (optional)"
              value={config.profile ?? ""}
              onChange={(event) =>
                setConfig({ ...config, profile: event.target.value || null })
              }
              hint="A managed encrypted profile ID, rather than a host login directory."
            />
            <Stack direction="row" gap={2} align="center">
              <Switch
                label="Runtime enabled"
                checked={config.enabled}
                onCheckedChange={(enabled) => setConfig({ ...config, enabled })}
              />
              <Text>Enabled</Text>
            </Stack>
            <Select
              label="Authentication"
              value={config.auth.kind}
              options={[
                {
                  value: "none",
                  label:
                    config.driver === "codex"
                      ? "Official Codex account / managed native profile"
                      : "No credential supplied",
                },
                { value: "vault", label: "Vault API key" },
                ...(selected?.auth_methods.includes("codex_oauth")
                  ? [{ value: "codex_oauth", label: "Shared ChatGPT account" }]
                  : []),
              ]}
              onChange={(event) =>
                setConfig({
                  ...config,
                  auth:
                    event.target.value === "vault"
                      ? {
                          kind: "vault",
                          secret_name: `runtime-${config.id || "account"}-key`,
                          env_name:
                            defaultEnv[config.driver] ?? "OPENAI_API_KEY",
                        }
                      : event.target.value === "codex_oauth"
                        ? { kind: "codex_oauth", provider_id: "" }
                        : { kind: "none" },
                })
              }
            />
            {config.auth.kind === "vault" && (
              <>
                <TextField
                  label="Vault secret name"
                  required
                  value={config.auth.secret_name}
                  onChange={(event) =>
                    setConfig({
                      ...config,
                      auth: {
                        ...(config.auth as Extract<
                          RuntimeConfig["auth"],
                          { kind: "vault" }
                        >),
                        secret_name: event.target.value,
                      },
                    })
                  }
                />
                <TextField
                  label="Documented API key environment variable"
                  required
                  value={config.auth.env_name}
                  onChange={(event) =>
                    setConfig({
                      ...config,
                      auth: {
                        ...(config.auth as Extract<
                          RuntimeConfig["auth"],
                          { kind: "vault" }
                        >),
                        env_name: event.target.value,
                      },
                    })
                  }
                />
                <PasswordField
                  label="New API key (optional)"
                  value={credential}
                  onChange={(event) => setCredential(event.target.value)}
                  hint={`Stored for process:runtime-${config.id}. Leave empty to keep the existing vault secret.`}
                />
              </>
            )}
            {config.auth.kind === "codex_oauth" && (
              <TextField
                label="Shared ChatGPT account instance ID"
                required
                value={config.auth.provider_id}
                onChange={(event) =>
                  setConfig({
                    ...config,
                    auth: {
                      kind: "codex_oauth",
                      provider_id: event.target.value,
                    },
                  })
                }
                hint="Use the same account instance ID as Models and accounts, or a separate ID for a new account."
              />
            )}
            {selected?.limitations.map((limitation) => (
              <Notice key={limitation}>{limitation}</Notice>
            ))}
            <Button type="submit" disabled={busy || cliPending}>
              {busy ? "Working…" : "Save runtime"}
            </Button>
          </Stack>
        </fieldset>
      </form>
      {initial?.driver === "codex" && initial.auth.kind === "none" && (
        <RuntimeAccountPanel
          id={initial.id}
          onPendingChange={setCliPending}
          onChanged={async () => {
            const refreshed = await call("runtime.config.get", {
              id: initial.id,
            });
            setConfig(refreshed);
            setArgv(JSON.stringify(refreshed.argv, null, 2));
            setHealth(undefined);
            setModels(undefined);
            onSaved(initial.id);
            try {
              setModels(
                (await call("runtime.models", { id: initial.id })).models,
              );
            } catch (caught) {
              setError(toAidexError(caught).message);
            }
          }}
        />
      )}
      <Button
        variant="outline"
        disabled={busy}
        onClick={() =>
          void act(async () => {
            const installed = await call("runtime.install", {
              driver: config.driver,
            });
            setConfig((current) => ({
              ...current,
              executable: installed.executable,
            }));
            setNotice(
              "Verified bundled executable installed. Save the runtime configuration to use it.",
            );
          })
        }
      >
        Install verified bundled component
      </Button>
      {initial && (
        <Stack gap={4}>
          <SectionHeader title="Saved runtime readiness" />
          <Stack direction="row" gap={2} wrap>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() =>
                void act(async () =>
                  setHealth(await call("runtime.health", { id: initial.id })),
                )
              }
            >
              Check readiness
            </Button>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() =>
                void act(async () =>
                  setModels(
                    (await call("runtime.models", { id: initial.id })).models,
                  ),
                )
              }
            >
              Discover models
            </Button>
            {initial.auth.kind === "codex_oauth" && (
              <Button
                variant="outline"
                disabled={busy || !!flow}
                onClick={() =>
                  void act(async () => {
                    const started = await call("runtime.auth.start", {
                      id: initial.id,
                    });
                    setFlow(started);
                    setAuthState("pending");
                    await call("auth.open", {
                      authorization_url: started.authorization_url,
                    });
                  })
                }
              >
                Sign in with ChatGPT
              </Button>
            )}
            {flow && (
              <Button
                variant="ghost"
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    await call("runtime.auth.cancel", {
                      id: initial.id,
                      flow_id: flow.flow_id,
                    });
                    setFlow(undefined);
                    setAuthState("cancelled");
                  })
                }
              >
                Cancel sign-in
              </Button>
            )}
          </Stack>
          {flow && (
            <a href={flow.authorization_url} target="_blank" rel="noreferrer">
              Open the pending sign-in link
            </a>
          )}
          {authState && <Text>{authState}</Text>}
          {health && (
            <Notice>
              Binary: {health.installed ? "present" : "missing"} · Protocol:{" "}
              {health.protocol_ready ? "verified" : "unverified"} ·
              Authentication:{" "}
              {health.authenticated ? "account connected" : "unverified"} ·{" "}
              {health.status}
            </Notice>
          )}
          {models && (
            <Select
              label="Discovered runtime model"
              value={modelId}
              options={[
                { value: "", label: "Choose a model" },
                ...models.map((model) => ({
                  value: model.id,
                  label: model.name,
                })),
              ]}
              onChange={(event) => setModelId(event.target.value)}
            />
          )}
          <TextField
            label="Exact runtime model ID"
            value={modelId}
            onChange={(event) => setModelId(event.target.value)}
            hint="For protocols without discovery, enter a documented model ID. Pi and OMP use provider/model."
          />
          <Button
            disabled={busy || !modelId.trim() || !initial.enabled}
            onClick={() =>
              void act(async () => {
                await preferences.updateSettings({
                  runtime_id: initial.id,
                  runtime_model_id: modelId.trim(),
                });
                setNotice(
                  "Runtime and model saved as defaults for new agent tasks. Choose supported permissions in the task composer.",
                );
              })
            }
          >
            Use runtime and model for new tasks
          </Button>
        </Stack>
      )}
    </Stack>
  );
}
