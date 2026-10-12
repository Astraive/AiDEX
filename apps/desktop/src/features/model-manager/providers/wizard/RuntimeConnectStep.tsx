import { useRef, useState, type FormEvent } from "react";
import { Notice } from "@aidex/ui/components";
import { DownloadIcon } from "@aidex/ui/icons";
import {
  Button,
  PasswordField,
  SegmentedControl,
  Select,
  Stack,
  Text,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { RuntimeAuth, RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { parseArgv, RUNTIME_VAULT_ENV, runtimeDriverName } from "../../catalog/runtimes";
import { storeRuntimeKey } from "../runtimeSecret";
import { RUNTIME_ID, slugify, uniqueId } from "./wizardLogic";

type AuthChoice = RuntimeAuth["kind"];

type Props = {
  driver: RuntimeDriverInfo;
  /** Ids already used by runtimes. */
  takenIds: ReadonlySet<string>;
  /** ChatGPT accounts Codex can share. */
  chatGptAccounts: readonly { id: string; title: string }[];
  /** Set when this wizard already saved the runtime and the user came back to fix it. */
  existing: RuntimeConfig | null;
  onSaved: (runtime: RuntimeConfig) => void;
};

type Errors = { id?: string; executable?: string; secret?: string; env?: string; key?: string };

/** Step 2 for an agent runtime: id, executable, arguments, and how it signs in. */
export function RuntimeConnectStep({ driver, takenIds, chatGptAccounts, existing, onSaved }: Props) {
  const name = runtimeDriverName(driver.driver);
  const fixedEnv = RUNTIME_VAULT_ENV[driver.driver];
  const codex = driver.driver === "codex";
  const [id, setId] = useState(() => existing?.id ?? uniqueId(slugify(driver.driver, "runtime"), takenIds));
  const [executable, setExecutable] = useState(existing?.executable ?? "");
  const [argv, setArgv] = useState(existing?.argv.join("\n") ?? "");
  const [auth, setAuth] = useState<AuthChoice>(existing?.auth.kind ?? (codex ? "codex_oauth" : "none"));
  const [secretName, setSecretName] = useState(
    existing?.auth.kind === "vault" ? existing.auth.secret_name : `runtime-${id}`,
  );
  const [envName, setEnvName] = useState(
    existing?.auth.kind === "vault" ? existing.auth.env_name : (fixedEnv ?? ""),
  );
  const [oauthAccount, setOauthAccount] = useState(
    existing?.auth.kind === "codex_oauth"
      ? existing.auth.provider_id
      : (chatGptAccounts[0]?.id ?? "openai"),
  );
  const [apiKey, setApiKey] = useState("");
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState<"save" | "install" | null>(null);
  // `<runtime id>/<secret name>` this step already stored a key for, so a retry need not ask again.
  const storedKey = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<string | null>(null);

  const authOptions: { value: AuthChoice; label: string }[] = [
    ...(codex ? [{ value: "codex_oauth" as const, label: "ChatGPT sign-in" }] : []),
    { value: "vault", label: "Vault secret" },
    { value: "none", label: "None" },
  ];

  const runtimeId = existing?.id ?? id.trim();
  // A key already in the vault for this runtime and secret: saved earlier by this wizard.
  const haveKey =
    storedKey.current === `${runtimeId}/${secretName.trim()}` ||
    (existing?.auth.kind === "vault" && existing.auth.secret_name === secretName.trim());

  const validate = (): Errors => ({
    // Only new ids are checked; a saved runtime keeps its id.
    id: existing
      ? undefined
      : !RUNTIME_ID.test(id.trim())
        ? "Use 1-64 lowercase letters, digits, dashes, or underscores, starting with a letter or digit."
        : takenIds.has(id.trim())
          ? "A runtime with this ID already exists."
          : undefined,
    executable: executable.trim() ? undefined : "Enter the executable path, or install the bundled runtime.",
    secret: auth === "vault" && !secretName.trim() ? "Enter the vault secret name." : undefined,
    env: auth === "vault" && !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(envName.trim()) ? "Enter an environment variable name." : undefined,
    key: auth === "vault" && !apiKey && !haveKey ? "Paste the API key." : undefined,
  });

  const runtimeAuth = (): RuntimeAuth =>
    auth === "vault"
      ? { kind: "vault", secret_name: secretName.trim(), env_name: envName.trim() }
      : auth === "codex_oauth"
        ? { kind: "codex_oauth", provider_id: oauthAccount }
        : { kind: "none" };

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const found = validate();
    setErrors(found);
    if (Object.values(found).some(Boolean)) return;
    setBusy("save");
    setError(null);
    const value = auth === "vault" ? apiKey : "";
    // The key leaves React state as soon as it is sent.
    setApiKey("");
    try {
      if (value) {
        await storeRuntimeKey(runtimeId, secretName.trim(), value);
        storedKey.current = `${runtimeId}/${secretName.trim()}`;
      }
      const config: RuntimeConfig = {
        id: runtimeId,
        driver: driver.driver,
        executable: executable.trim(),
        argv: parseArgv(argv),
        profile: existing?.profile ?? null,
        enabled: true,
        auth: runtimeAuth(),
      };
      const saved = await call(existing ? "runtime.update" : "runtime.add", config);
      onSaved(saved);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(null);
    }
  };

  const install = async () => {
    setBusy("install");
    setError(null);
    try {
      const result = await call("runtime.install", { driver: driver.driver });
      setExecutable(result.executable);
      setInstalled(`Installed and verified ${result.executable}.`);
    } catch (caught) {
      // The daemon's own words: what is missing and why.
      setError(toAidexError(caught).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <form onSubmit={(event) => void save(event)} noValidate aria-label={`Add ${name}`}>
      <Stack gap={4}>
        <Text tone="muted">
          {name} runs as an agent with full access inside a chosen project. AiDEX launches it with{" "}
          {driver.protocol}.
        </Text>
        <TextField
          label="Runtime ID"
          size="md"
          spellCheck={false}
          autoCapitalize="off"
          disabled={!!existing}
          value={existing?.id ?? id}
          error={errors.id}
          onChange={(event) => {
            setId(event.target.value);
            if (secretName === `runtime-${id}`) setSecretName(`runtime-${event.target.value}`);
          }}
        />
        <Stack gap={2}>
          <TextField
            label="Executable path"
            size="md"
            spellCheck={false}
            value={executable}
            error={errors.executable}
            hint="An absolute path, or a path relative to the AiDEX drive."
            onChange={(event) => setExecutable(event.target.value)}
          />
          <Stack direction="row">
            <Button
              variant="ghost"
              icon={<DownloadIcon />}
              disabled={busy !== null}
              onClick={() => void install()}
            >
              {busy === "install" ? "Installing…" : "Install bundled runtime"}
            </Button>
          </Stack>
          {installed && <Notice tone="success">{installed}</Notice>}
        </Stack>
        <TextArea
          label="Arguments"
          hint="One per line, placed before the driver's own arguments. Never put credentials here."
          monospace
          rows={3}
          spellCheck={false}
          value={argv}
          onChange={(event) => setArgv(event.target.value)}
        />
        <SegmentedControl label="Sign-in" options={authOptions} value={auth} onValueChange={setAuth} />
        {auth === "vault" && (
          <Stack gap={3}>
            <TextField
              label="Vault secret name"
              size="md"
              spellCheck={false}
              value={secretName}
              error={errors.secret}
              hint="AiDEX stores the key in the drive vault under this name and releases it only to this runtime."
              onChange={(event) => setSecretName(event.target.value)}
            />
            <PasswordField
              label={haveKey ? "API key (leave empty to keep the saved key)" : "API key"}
              revealLabel="Show key"
              size="md"
              autoComplete="off"
              hint="Encrypted in the drive vault and never shown again."
              error={errors.key}
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
            />
            <TextField
              label="Environment variable"
              size="md"
              spellCheck={false}
              disabled={!!fixedEnv}
              value={envName}
              error={errors.env}
              hint={fixedEnv ? `${name} reads its key from ${fixedEnv}.` : undefined}
              onChange={(event) => setEnvName(event.target.value)}
            />
          </Stack>
        )}
        {auth === "codex_oauth" &&
          (chatGptAccounts.length > 1 ? (
            <Select
              label="ChatGPT account"
              value={oauthAccount}
              options={chatGptAccounts.map((account) => ({ value: account.id, label: account.title }))}
              onChange={(event) => setOauthAccount(event.target.value)}
            />
          ) : (
            <Text tone="muted">
              {chatGptAccounts.length
                ? `Codex uses the ChatGPT sign-in of ${chatGptAccounts[0].title}.`
                : "Sign in with ChatGPT in this runtime's settings after adding it."}
            </Text>
          ))}
        {auth === "none" && (
          <Text tone="muted">{name} uses its own sign-in on this computer; AiDEX passes it no credentials.</Text>
        )}
        {error && (
          <Notice tone="danger" role="alert">
            {error}
          </Notice>
        )}
        <Stack direction="row" justify="end">
          <Button type="submit" variant="primary" disabled={busy !== null}>
            {busy === "save" ? "Adding…" : existing ? "Save and check again" : "Add runtime"}
          </Button>
        </Stack>
      </Stack>
    </form>
  );
}
