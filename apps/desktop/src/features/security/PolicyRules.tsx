import { useEffect, useState } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import { Button, Select, Stack, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";

const permissionOptions = [
  { value: "supervised", label: "Supervised" },
  { value: "auto_accept_edits", label: "Auto-accept edits" },
  { value: "auto", label: "Auto · supervised fallback" },
  { value: "full_access", label: "Full access" },
];
export function PolicyRules() {
  const preferences = usePreferences();
  const network = useRpc("network.status");
  const vault = useRpc("vault.status");
  const execution = useRpc("execution.status");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    const refresh = () => {
      network.refetch();
      vault.refetch();
      execution.refetch();
    };
    window.addEventListener("aidex:security-changed", refresh);
    return () => window.removeEventListener("aidex:security-changed", refresh);
  }, [network.refetch, vault.refetch, execution.refetch]);
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      network.refetch();
      execution.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Stack gap={5}>
      <SectionHeader title="Configured policy controls" />
      {(error || network.error || execution.error) && (
        <Notice tone="danger">
          {error ?? network.error?.message ?? execution.error?.message}
        </Notice>
      )}
      <Select
        label="Default permission for new native conversations"
        value={preferences.settings.permission_mode ?? "supervised"}
        options={permissionOptions}
        disabled={
          busy || !preferences.loaded || vault.data?.state !== "unlocked"
        }
        onChange={(event) =>
          void act(() =>
            preferences.updateSettings({
              permission_mode: event.target.value as PermissionMode,
            }),
          )
        }
        hint="Saved in encrypted preferences. Existing conversations keep their own configuration. Unlock the vault to change defaults."
      />
      {preferences.settings.permission_mode === "full_access" && (
        <Notice tone="warning">
          Full access runs permitted processes as your current user; it does not
          imply filesystem or network confinement.
        </Notice>
      )}
      {execution.data && (
        <Text tone="muted">
          Automatic reviewer:{" "}
          {execution.data.capabilities.automatic_reviewer
            ? "available"
            : "unavailable"}
          . Auto uses {execution.data.capabilities.auto_falls_back_to}.{" "}
          {execution.data.capabilities.shell_requires_full_access_on_windows
            ? "Shell execution on this Windows host requires full access."
            : "Process scope remains subject to this host's capability report."}
        </Text>
      )}
      <Stack direction="row" gap={2} align="center">
        <Switch
          label="AiDEX offline network gate"
          checked={network.data?.offline ?? false}
          disabled={busy || !network.data}
          onCheckedChange={(enabled) =>
            void act(() => call("network.offline", { enabled }))
          }
        />
        <Text>
          {network.data
            ? network.data.offline
              ? "AiDEX network gate is offline"
              : "AiDEX network gate permits online operations"
            : "Network gate status unavailable"}
        </Text>
      </Stack>
      <Text tone="muted">
        Offline mode gates AiDEX-managed provider, search, and extension
        requests. It does not replace operating-system network confinement for a
        full-access process.
      </Text>
      <SectionHeader title="Filesystem mounts and network allowlists" />
      <Notice>
        Core validates each tool against its selected project and permission
        scope. This build exposes no administrator RPC to edit mount policies or
        domain allowlists; no editable rules are inferred here.
      </Notice>
      <Stack direction="row" gap={2}>
        <Button
          variant="outline"
          disabled
          title="No editable filesystem policy RPC is exposed"
        >
          Edit filesystem policy · unavailable
        </Button>
        <Button
          variant="outline"
          disabled
          title="No editable network allowlist RPC is exposed"
        >
          Edit domain allowlist · unavailable
        </Button>
      </Stack>
    </Stack>
  );
}
