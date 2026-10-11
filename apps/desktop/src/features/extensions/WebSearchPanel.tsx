import { useEffect, useState } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import {
  Button,
  Card,
  PasswordField,
  Select,
  Stack,
  Switch,
  Text,
} from "@aidex/ui/primitives";
import type { SearchProvider } from "@aidex/client/dto/web";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { useExtensionAction } from "./useExtensionAction";

export function WebSearchPanel() {
  const status = useRpc("web.status");
  const action = useExtensionAction(status.refetch);
  const [provider, setProvider] = useState<SearchProvider>("brave");
  const [enabled, setEnabled] = useState(false);
  const [key, setKey] = useState("");
  useEffect(() => {
    if (status.data) {
      setProvider(status.data.settings.provider);
      setEnabled(status.data.settings.enabled);
    }
  }, [status.data]);
  return (
    <Card padding="md">
      <Stack gap={4}>
        <SectionHeader title="Native web search" />
        {(status.error || action.error) && (
          <Notice tone="danger">{action.error ?? status.error?.message}</Notice>
        )}
        {action.message && <Notice tone="success">{action.message}</Notice>}
        <Text>
          Connect a search provider for native agent web_search. Web fetch
          availability follows the drive's network policy.
        </Text>
        <Select
          label="Search provider"
          value={provider}
          options={[
            { value: "brave", label: "Brave Search" },
            { value: "tavily", label: "Tavily" },
          ]}
          onChange={(event) => {
            setProvider(event.target.value as SearchProvider);
            setKey("");
          }}
        />
        <PasswordField
          label="Search API key"
          value={key}
          onChange={(event) => setKey(event.target.value)}
          hint="Stored in the drive vault for the web search service. Leave empty to keep the existing credential for the same provider."
        />
        <Stack direction="row" gap={2} align="center">
          <Switch
            label="Enable native web search"
            checked={enabled}
            onCheckedChange={setEnabled}
          />
          <Text>Enable search for native agents</Text>
        </Stack>
        {status.data && (
          <Notice>
            {status.data.credential_present
              ? "Credential configured · live provider verification pending"
              : "Search provider unconfigured"}{" "}
            · Network {status.data.network_allowed ? "allowed" : "disabled"} ·
            Web fetch{" "}
            {status.data.fetch_available ? "available" : "unavailable"}
          </Notice>
        )}
        <Stack direction="row" gap={2}>
          <Button
            disabled={
              action.busy ||
              !status.data ||
              (provider !== status.data.settings.provider && !key)
            }
            onClick={() =>
              void action.act(async () => {
                await call("web.configure", {
                  provider,
                  enabled,
                  ...(key ? { api_key: key } : {}),
                });
                setKey("");
              }, "Search configuration saved. A native search request will verify provider access.")
            }
          >
            Save connection
          </Button>
          <Button
            variant="ghost"
            disabled={action.busy || !status.data?.credential_present}
            onClick={() =>
              void action.act(async () => {
                await call("web.disconnect");
                setKey("");
              }, "Search disconnected.")
            }
          >
            Disconnect
          </Button>
        </Stack>
      </Stack>
    </Card>
  );
}
