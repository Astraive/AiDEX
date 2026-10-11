# Native web tools

`aidex-web` supplies `web.search` and `web.fetch` to the native tool broker and the
scoped external-runtime MCP bridge. Chat mode receives no agent tools. Search is
disabled until the user configures Brave or Tavily in Extensions → Web search.
Fetch accepts public HTTPS text without a search subscription.

Brave uses its web-search endpoint and `X-Subscription-Token`; Tavily uses its
search endpoint and a bearer header. Each provider has a separate encrypted vault
record scoped to `process:aidex-web-search`. Credentials are resolved only during
execution and never appear in configuration, descriptors, plans, events or RPC
results. Plans bind the provider settings and exact credential revision; rotation
requires a new plan. Known credential reflections in results are redacted.

The broker evaluates actual network destinations and credential consumers.
Cancellation, offline mode, vault locking and bound search configuration/credential
revisions are checked before admission and
during transport. The transport pins public DNS answers, refuses private addresses,
ambient proxies, URI credentials and non-443 ports, and limits fetch redirects to
the original origin. Authenticated search redirects are refused. Up to eight
workers and bounded time/body/text limits prevent indefinite worker accumulation.
HTML is reduced to untrusted text; it is never executed or treated as instructions.

Authenticated desktop RPCs:

- `web.status {}` returns value-free settings and availability.
- `web.configure {provider: "brave" | "tavily", enabled: boolean, api_key?: string}`
  requires an unlocked vault. Switching enabled providers requires a fresh key.
- `web.disconnect {}` disables search and removes the selected provider's key.

Protocol, cancellation, policy facts, credential rotation, reflection, malformed
response, rate-limit, URL and text-content fixtures are in `tests/tools.rs`.
The separate ignored live fixture verifies public DNS/HTTPS and page reduction
through the production transport. Search account entitlement and vendor responses require live verification with a
user-connected subscription. This crate does not implement browser automation,
JavaScript pages, image search, private/intranet fetches or a SearXNG adapter.

Official protocol references:
[Brave](https://api-dashboard.search.brave.com/app/documentation/web-search) and
[Tavily](https://docs.tavily.com/documentation/api-reference/endpoint/search).
