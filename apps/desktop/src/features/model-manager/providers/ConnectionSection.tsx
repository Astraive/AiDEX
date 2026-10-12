import { useId, useState } from "react";
import {
  InlineCode,
  KeyValueList,
  Notice,
  SettingRow,
  SettingsSection,
} from "@aidex/ui/components";
import { GlobeIcon, RefreshIcon } from "@aidex/ui/icons";
import { Button, Stack, StatusIndicator, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { HealthStatus, ProviderHealth } from "@aidex/client/dto/models";
import { isRuntimeAccount } from "../catalog/providerAccounts";
import { relativeTime, type AccountItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

const HEALTH_LABELS: Record<HealthStatus, { label: string; tone: "success" | "warning" | "danger" | "neutral" }> = {
  ready: { label: "Ready", tone: "success" },
  auth_required: { label: "Sign-in required", tone: "warning" },
  unavailable: { label: "Unavailable", tone: "danger" },
  refused: { label: "Refused", tone: "neutral" },
};

/** Kinds whose models are always declared: their APIs have no usable model listing. */
const DECLARED_ONLY = new Set(["azure-openai", "bedrock"]);

/** Endpoint facts, model discovery, and an on-demand connection check. */
export function ConnectionSection({ item }: { item: AccountItem }) {
  const { account } = item;
  const { descriptor } = account;
  const actions = useProviderActions();
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const discoverId = useId();
  const managed = isRuntimeAccount(account);
  const declaredOnly = DECLARED_ONLY.has(descriptor.kind);
  // The daemon refuses an account that neither discovers nor declares models, so discovery stays on
  // until a custom model exists. Turning it back on is always allowed.
  const needsCustomModel = account.discover_models && account.declared_models.length === 0;
  const check = async () => {
    const result = await actions.run("health", () => call("provider.health", { id: descriptor.id }));
    if (result) setHealth(result);
  };
  const healthLabel = health ? (HEALTH_LABELS[health.status] ?? { label: health.status, tone: "neutral" as const }) : null;
  return (
    <SettingsSection
      title="Connection"
      icon={<GlobeIcon />}
      actions={
        <Button
          variant="outline"
          icon={<RefreshIcon />}
          disabled={actions.busy !== null}
          onClick={() => void check()}
        >
          {actions.busy === "health" ? "Checking…" : "Check connection"}
        </Button>
      }
    >
      <KeyValueList
        items={[
          {
            label: "Endpoint",
            value: descriptor.base_url ? <InlineCode>{descriptor.base_url}</InlineCode> : "Set by the provider",
          },
          { label: "Account ID", value: <InlineCode>{descriptor.id}</InlineCode> },
          { label: "Protocol", value: <InlineCode>{descriptor.kind}</InlineCode> },
          { label: "Runs", value: descriptor.locality === "local" ? "On this computer" : "Remote service" },
          ...(descriptor.auth.type === "vault_secret"
            ? [{ label: "Vault secret", value: <InlineCode>{descriptor.auth.secret_name}</InlineCode> }]
            : []),
        ]}
      />
      {!managed && (
        <SettingRow
          title="Discover models automatically"
          description={
            declaredOnly
              ? "This provider does not list its models. Declare deployments under Custom models."
              : needsCustomModel
                ? "List the models the provider serves. Add a custom model before turning discovery off."
                : "List the models the provider serves. When off, only custom models are offered."
          }
          controlId={discoverId}
          control={
            <Switch
              id={discoverId}
              aria-describedby={`${discoverId}-description`}
              checked={account.discover_models}
              disabled={declaredOnly || needsCustomModel || actions.busy !== null}
              onCheckedChange={(checked) => void actions.updateAccount(account, { discover_models: checked })}
            />
          }
        />
      )}
      {health && healthLabel && (
        <Stack gap={1} role="status">
          <Stack direction="row" gap={3} align="center" wrap>
            <StatusIndicator tone={healthLabel.tone} label={healthLabel.label} />
            {health.latency_ms !== undefined && <Text as="span" tone="muted">{health.latency_ms} ms</Text>}
            <Text as="span" tone="muted">
              Checked {relativeTime(health.checked_at, Date.now()) ?? "just now"}
            </Text>
          </Stack>
          {health.message && <Text tone="muted">{health.message}</Text>}
        </Stack>
      )}
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
    </SettingsSection>
  );
}
