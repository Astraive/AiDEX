# aidex-web

Web search and guarded page fetch for AiDEX. The crate holds no policy and no
credentials of its own: the embedder (the daemon) passes a credential resolver
and an egress check, and both run for every request.

## Settings: `private_config/web.toml`

A closed schema (`deny_unknown_fields`), version 1, written atomically. A
missing file means the defaults.

| Field | Default | Meaning |
| --- | --- | --- |
| `version` | `1` | File format version. |
| `enabled` | `false` | Master switch: web tools are offered to agents only when on. |
| `provider` | `"duckduckgo"` | `brave`, `tavily`, `exa`, `searxng`, or `duckduckgo`. |
| `endpoint` | none | SearXNG base URL (`https://`, or `http://` to a loopback host); no query or fragment. Kept, but unused, while another provider is chosen. |
| `secret_name` | none | Vault secret *name* of the provider key (never the value). Refused for DuckDuckGo. |
| `max_results` | `5` | Results per search, 1 to 10. |
| `fetch_enabled` | `true` | Whether `web.fetch` is offered alongside search. |

`WebSettingsPatch` applies a partial change (`null` clears `endpoint` or
`secret_name`; changing the provider clears `secret_name` unless the patch
names one, so a key never follows a provider change).
Unknown fields, such as a pasted `api_key`, are refused, and parse errors never
quote values from the file.

Keys live in the vault with the consumer `process:web`
(`aidex vault set <name> --consumer process:web`).

## Search providers

| Provider | Request | Key |
| --- | --- | --- |
| Brave | `GET https://api.search.brave.com/res/v1/web/search?q=&count=` | `X-Subscription-Token` header |
| Tavily | `POST https://api.tavily.com/search` (JSON) | `api_key` in the JSON body |
| Exa | `POST https://api.exa.ai/search` (JSON, highlights as snippets) | `x-api-key` header |
| SearXNG | `GET <endpoint>/search?q=&format=json&pageno=1` | optional, `Authorization: Bearer` |
| DuckDuckGo | `GET https://html.duckduckgo.com/html/?q=` (HTML) | none |

`WebSettings::search_url()` is the URL a search contacts: the provider's
built-in URL, or `<endpoint>/search` for SearXNG. Callers that check the
destination ahead of time (the harness's `network.request` policy check) use
it, so the checked host is the host the request goes to.

`WebClient::search(query, max_results, cancel)` returns at most 10
`SearchResult { title, url, snippet, published?, source }`. Titles and snippets
are plain text (tags stripped, entities decoded once, bounded), URLs are
absolute `http(s)` without credentials, and duplicates are dropped. The
DuckDuckGo page is parsed with the crate's tokenizer: `result__a` anchors and
`result__snippet` elements, ads (`result--ad`, `y.js`) skipped, `/l/?uddg=`
links unwrapped; a bot check (HTTP 202 or a challenge page) is `rate_limited`.

Order of a search: query and count validation, destination check, egress
check, then the key is released, then DNS and the connection. A refusal at any
step releases no key and opens no socket. HTTP 401/403 are `unauthorized`, 429
`rate_limited`; response bodies are never quoted in errors.

## Fetch

`WebClient::fetch(url, cancel)` returns
`FetchedPage { url, final_url, status, content_type, title?, text, truncated }`:

- `https` only; plain `http` only to loopback hosts. No userinfo, at most
  8 KiB, fragment dropped.
- At most five redirects, followed manually. Every hop is re-validated
  (scheme, credentials, address class) and re-approved by the egress check
  with its hop number. A page whose URL is not itself an explicitly local
  address can never redirect to a local one.
- Destinations: IP literals and `localhost` are classified before any I/O;
  names are resolved before connecting and refused if any address is
  loopback, private (RFC 1918, CGNAT, ULA), link-local (cloud metadata), or
  reserved; IPv4-mapped, NAT64, and 6to4 forms are classified by the embedded
  address. Explicitly local URLs are allowed only when
  `WebOptions::local_targets` permits them; link-local and reserved addresses
  never are. The connection is pinned to the checked addresses (no DNS
  rebinding), with no proxy, no cookies, and no automatic redirects.
- 2 MiB raw body cap (the rest is not read; `truncated`), 256 KiB of
  extracted text. Content types `text/*`, `application/json`, and
  `application/xhtml+xml` only.
- HTML becomes readable text: title, `#` headings, paragraphs, nested lists,
  fenced `pre` blocks, inline code, table rows, and links as `[text](url)`
  resolved against the page (or the first `<base href>`, as in browsers); a
  link or base URL over 8 KiB stays plain text. Scripts, styles, hidden
  elements, forms, and media are dropped. Latin-1/Windows-1252 bodies are
  decoded by their declared charset; everything else as UTF-8.
- Extraction is linear in the body size whatever the markup (a new `<a>`
  closes an open one, as in HTML parsing; a link's URL is resolved only when
  the link has text; list indentation stops growing after 16 levels), and it
  checks the fetch's cancel token and deadline every few hundred tokens: a
  fetch cancelled or timed out during extraction fails as `cancelled` or
  `timeout`.

## Embedding

```rust
let client = WebClient::new(settings, credentials, egress)?;
```

- `credentials: Fn(&CredentialRequest) -> Result<Secret, WebError>`: the
  daemon evaluates `secret.use` for `process:web` and releases a 30-second
  vault handle.
- `egress: Fn(&EgressRequest) -> Result<(), WebError>`: the daemon checks the
  offline switch, an emergency stop, and a `network.request` intent for the
  host under the drive's policy (profile, privacy, network mode, documents).
  An approval verdict is refused, since a synchronous request cannot wait.

`Secret` never prints, request headers and bodies holding a key are zeroized
after sending, and `WebError` messages name hosts and providers but never a
full URL, a header, or a key.

Page text and results are untrusted data; callers must wrap them before they
reach a model. `validate_open_url` checks a result link before a client hands
it to the system browser.
