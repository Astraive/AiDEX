import { useState, type FormEvent } from "react";
import { InlineCode, KeyValueList, Notice, SettingsSection } from "@aidex/ui/components";
import { DownloadIcon, RefreshIcon, TerminalIcon } from "@aidex/ui/icons";
import { Button, Stack, Text, TextArea, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { parseArgv } from "../catalog/runtimes";
import type { RuntimeItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

/** Profiles are managed encrypted profile ids, not host directories. */
const PROFILE = /^[A-Za-z0-9_-]{1,64}$/;

/** Executable, arguments, and profile of a runtime, with status and install helpers. */
export function RuntimeConnectionSection({ item }: { item: RuntimeItem }) {
  const catalog = useProviderCatalog();
  const actions = useProviderActions();
  const { runtime, driver, health } = item;
  const [executable, setExecutable] = useState(runtime.executable);
  const [argv, setArgv] = useState(runtime.argv.join("\n"));
  const [profile, setProfile] = useState(runtime.profile ?? "");
  const [installed, setInstalled] = useState<string | null>(null);
  const profileError = profile.trim() && !PROFILE.test(profile.trim())
    ? "Use 1-64 letters, digits, dashes, or underscores."
    : undefined;
  const dirty =
    executable !== runtime.executable ||
    argv !== runtime.argv.join("\n") ||
    profile !== (runtime.profile ?? "");

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!executable.trim() || profileError) return;
    await actions.updateRuntime({
      ...runtime,
      executable: executable.trim(),
      argv: parseArgv(argv),
      profile: profile.trim() || null,
    });
  };

  const install = async () => {
    const result = await actions.run("install", () => call("runtime.install", { driver: runtime.driver }));
    if (!result) return;
    setExecutable(result.executable);
    setInstalled(`Installed and verified ${result.executable}. Save to use it.`);
  };

  const report = health?.state === "ready" ? health.health : null;
  return (
    <SettingsSection
      title="Connection"
      icon={<TerminalIcon />}
      actions={
        <Button
          variant="outline"
          icon={<RefreshIcon />}
          disabled={health?.state === "loading"}
          onClick={() => void catalog.checkRuntime(runtime.id)}
        >
          {health?.state === "loading" ? "Checking…" : "Check status"}
        </Button>
      }
    >
      <KeyValueList
        items={[
          { label: "Runtime ID", value: <InlineCode>{runtime.id}</InlineCode> },
          { label: "Protocol", value: <InlineCode>{driver?.protocol ?? runtime.driver}</InlineCode> },
          ...(driver?.command.length
            ? [{ label: "Launch arguments", value: <InlineCode>{driver.command.join(" ")}</InlineCode> }]
            : []),
          ...(report
            ? [
                { label: "Installed", value: report.installed ? "Yes" : "No" },
                { label: "Protocol check", value: report.protocol_ready ? "Passed" : "Not verified" },
                { label: "Signed in", value: report.authenticated ? "Yes" : "Not verified" },
              ]
            : []),
        ]}
      />
      {health?.state === "error" && <Notice tone="danger">{health.message}</Notice>}
      {report?.limitations.length ? (
        <Stack gap={1}>
          <Text variant="label" tone="muted">
            Limitations
          </Text>
          {report.limitations.map((limitation) => (
            <Text key={limitation} tone="muted">
              {limitation}
            </Text>
          ))}
        </Stack>
      ) : null}
      <form onSubmit={(event) => void save(event)} noValidate aria-label={`${item.title} launch settings`}>
        <Stack gap={3}>
          <TextField
            label="Executable path"
            size="md"
            spellCheck={false}
            value={executable}
            hint="An absolute path, or a path relative to the AiDEX drive."
            error={executable.trim() ? undefined : "Enter the runtime's executable."}
            onChange={(event) => setExecutable(event.target.value)}
          />
          <TextArea
            label="Extra arguments"
            hint="One per line, placed before the driver's own arguments. Never put credentials here."
            monospace
            rows={3}
            spellCheck={false}
            value={argv}
            onChange={(event) => setArgv(event.target.value)}
          />
          <TextField
            label="Profile (optional)"
            size="md"
            spellCheck={false}
            value={profile}
            error={profileError}
            hint="A managed, encrypted profile ID on the drive."
            onChange={(event) => setProfile(event.target.value)}
          />
          <Stack direction="row" gap={2} wrap>
            <Button
              type="submit"
              variant="primary"
              disabled={!dirty || !executable.trim() || !!profileError || actions.busy !== null}
            >
              {actions.busy === "update" ? "Saving…" : "Save"}
            </Button>
            <Button
              variant="ghost"
              icon={<DownloadIcon />}
              disabled={actions.busy !== null}
              onClick={() => void install()}
            >
              {actions.busy === "install" ? "Installing…" : "Install bundled runtime"}
            </Button>
          </Stack>
        </Stack>
      </form>
      {installed && <Notice tone="success">{installed}</Notice>}
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
    </SettingsSection>
  );
}
