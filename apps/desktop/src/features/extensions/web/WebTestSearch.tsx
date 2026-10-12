import { useState, type FormEvent } from "react";
import { Notice, SettingsSection, WebResultList } from "@aidex/ui/components";
import { SearchIcon } from "@aidex/ui/icons";
import { Button, Stack, Text, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { WebTestResult } from "@aidex/client/dto/web";
import { describeError, useOpenLink } from "../data";

/** Runs one search with the saved settings (`web.test`) and previews the results. */
export function WebTestSearch({ providerName, disabled }: { providerName: string; disabled: boolean }) {
  const openLink = useOpenLink();
  const [query, setQuery] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<WebTestResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const text = query.trim();
    if (!text) return;
    setRunning(true);
    setError(null);
    setResult(null);
    try {
      setResult(await call("web.test", { query: text }));
    } catch (caught) {
      setError(describeError(toAidexError(caught)));
    } finally {
      setRunning(false);
    }
  };

  return (
    <SettingsSection
      titleLevel={2}
      title="Test search"
      icon={<SearchIcon />}
      description={`Runs one search through ${providerName} with the saved settings. Nothing is sent to a model.`}
    >
      <form onSubmit={(event) => void run(event)} aria-label="Test web search">
        <Stack direction="row" gap={3} align="end">
          <TextField
            label="Query"
            labelVisibility="hidden"
            size="md"
            placeholder="Search the web…"
            leadingIcon={<SearchIcon />}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <Button type="submit" variant="outline" disabled={disabled || running || !query.trim()}>
            {running ? "Searching…" : "Test search"}
          </Button>
        </Stack>
      </form>
      {error ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}
      {result ? (
        <Stack gap={3}>
          <Text tone="muted" role="status">
            {result.results.length === 1 ? "1 result" : `${result.results.length} results`} in {result.elapsed_ms} ms
          </Text>
          <WebResultList
            results={result.results.map((entry) => ({
              title: entry.title,
              url: entry.url,
              snippet: entry.snippet,
              published: entry.published,
            }))}
            onOpenLink={openLink}
            emptyText="The provider returned no results for this query."
          />
        </Stack>
      ) : null}
    </SettingsSection>
  );
}
