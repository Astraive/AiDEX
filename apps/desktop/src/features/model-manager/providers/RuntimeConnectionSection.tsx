import { useState, type FormEvent } from "react";
import { InlineCode, KeyValueList, Notice, SettingsSection } from "@aidex/ui/components";
import { DownloadIcon, RefreshIcon, TerminalIcon } from "@aidex/ui/icons";
import { Button, Stack, Text, TextArea, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { parseArgv } from "../catalog/runtimes";
import type { RuntimeItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

/**
 * Executable and arguments of a runtime, with status and install helpers. Its account profile is
 * managed by sign-in and import in the Account section, never typed here: Save applies only these
 * fields to the stored configuration, and waits while `accountBusy` (an import, sign-in, or sign-out
 * in the Account section is about to bind or unbind the account).
 */
export function RuntimeConnectionSection({ item, accountBusy = false }: { item: RuntimeItem; accountBusy?: boolean }) {
  const catalog = useProviderCatalog();
  const actions = useProviderActions();
  const { runtime, driver, health } = item;
  const [executable, setExecutable] = useState(runtime.executable);
  const [argv, setArgv] = useState(runtime.argv.join("\n"));
  const [installed, setInstalled] = useState<string | null>(null);
  const dirty = executable !== runtime.executable || argv !== runtime.argv.join("\n");

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!executable.trim()) return;
    if (accountBusy) return;
    // Applied to the configuration as stored now, so its account profile and sign-in stay as they are.
    await actions.updateRuntime(runtime.id, { executable: executable.trim(), argv: parseArgv(argv) });
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
          <Stack direction="row" gap={2} wrap>
            <Button
              type="submit"
              variant="primary"
              disabled={!dirty || !executable.trim() || actions.busy !== null || accountBusy}
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
