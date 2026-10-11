# Native extensions contract

`aidex-extensions` owns portable skills, plugin manifests and an MCP client. The daemon owns this service; the frontend only receives metadata and secret names. Catalog reads never launch processes, contact servers, or resolve credentials. This Rust implementation follows T3 Code's scoped service/tool boundary; it does not reuse T3 source code.

## Rust composition

```rust
ExtensionService::open(paths: DrivePaths, vault: Arc<VaultService>, live_network_gate: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<Arc<ExtensionService>>;
ExtensionService::with_connector(paths, secrets: Arc<dyn ExtensionSecrets>, connector: Arc<dyn McpConnector>) -> Result<Arc<ExtensionService>>;
service.tool_adapters() -> Vec<Box<dyn aidex_tools::Tool>>;
service.connect_mcp_async(id: &str, scope: &CallScope) -> Result<McpServerSummary>;
service.discover_mcp_async(id: &str, scope: &CallScope) -> Result<McpServerSummary>;
```

`CallScope { workspace_id: String, workspace_root: PathBuf, profile: String, full_access: bool, network_allowed: bool, selected_skill_ids: Vec<String> }` is constructed by the daemon from authenticated session configuration, never supplied by model tool arguments. `selected_skill_ids` defaults to an empty list when deserializing old scopes. Call `bind_scope(session_id, scope)` before each native request or tool plan. The harness uses `catalog_for(&scope)` and `skill_context(&scope)` to inject enabled skill metadata. Model access to a manual-only skill requires both explicit user selection and `user_invocable`; guessing its ID is refused. Skill pins do not change MCP workspace/profile grants or connection ownership. `skill.read`, `mcp.tools`, `mcp.call` (HTTPS) and `mcp.stdio_call` are sealed-plan tool adapters. MCP execution checks the configured workspace and profiles again after approval and reads the live network gate. Stdio requires `full_access` on Windows and its adapter advertises `permissions:full_access`; HTTPS calls retain ordinary supervised approval. The first executing native session owns connected server state until explicit reconnect.

## DTOs and suggested RPC mapping

- `extensions.skills.list {}` -> `SkillSummary[]`; `extensions.skills.read {id}` -> `SkillDocument`; `extensions.skills.save {id, content, expected_digest?}` -> `SkillSummary`; `extensions.skills.import {content, trusted, workspace_ids, profiles}` -> `SkillSummary`; `extensions.skills.enable {id, enabled}` -> `SkillSummary`; `extensions.skills.remove {id}` -> `{removed: bool}`.
- `extensions.plugins.list {}` -> `PluginSummary[]`; `extensions.plugins.install {manifest, files}` -> `PluginSummary`; `extensions.plugins.enable {id, enabled}` -> `PluginSummary`; `extensions.plugins.remove {id}` -> `{removed: bool}`.
- `extensions.mcp.list {}` -> `McpServerSummary[]`; `extensions.mcp.upsert {config}` -> `McpServerSummary`; `extensions.mcp.remove {id}` -> `{removed: bool}`; `extensions.mcp.connect {id, workspace_id, profile, full_access}` -> `McpServerSummary`; `extensions.mcp.disconnect {id}` -> `McpServerSummary`; `extensions.mcp.discover {id, ...scope}` -> `McpServerSummary`.
- `extensions.catalog {workspace_id, profile, full_access}` -> `CatalogSnapshot`.
- `extensions.marketplace.list {}` -> `MarketplaceCatalog`; `extensions.marketplace.install {id, version, package_digest, trusted, workspace_ids, profiles}` -> `PluginSummary` through `install_marketplace(MarketplaceInstall)`.

The daemon resolves `workspace_root` from its workspace registry for connect/discover and overwrites scope fields from selected session configuration for agent calls. Administrative imports and explicit connect commands require user-originated RPC; never register secret read or extension trust mutation as model tools.

`SkillSummary` contains `id`, `name`, `description`, `enabled`, `trusted`, `user_invocable`, `model_invocable`, `workspace_ids`, `profiles`, `digest`, `plugin_id?`. `SkillDocument` adds `content`. Imports accept bounded SKILL.md scalar frontmatter; unsupported relevant YAML structures fail explicitly.

`PluginManifest` is closed JSON: `id`, `name`, `version`, `description`, `trusted`, `enabled`, `workspace_ids`, `profiles`, `skills: [{path}]`, `mcp_servers: McpServerConfig[]`. `files` is a map from declared relative skill paths to UTF-8 content. No arbitrary executable plugin code is loaded into the daemon. Plugin contributions inherit trust, enable state and intersect parent scope. Plugin installation is a portable manifest plus skills/MCP contribution install, not an online marketplace download.

`McpServerConfig` is closed JSON: `id`, `name`, `enabled`, `trusted`, `workspace_ids`, `profiles`, `transport`, `secret_refs`, `timeout_ms`, `max_output_bytes`. `transport` is tagged `{kind:"stdio", command, args, cwd?}` or `{kind:"https", url}`. `secret_refs` maps environment/header names to vault names, resolved only as consumer `mcp:<id>`. Empty workspace/profile grants mean no access; callers configure explicit grants (wildcard `*` is an explicit all-scope grant). HTTPS uses bounded Streamable HTTP JSON/SSE responses and validates endpoint scheme/origin; no OAuth or ambient cookie flows. Stdio runs only through the owned Sandbox helper and explicit lifecycle connections.

`McpServerSummary` adds `connected`, `connecting`, `catalog_revision`, `tools`, `last_error?`, `plugin_id?`, `owner_session_id?`, `call_tool_name`; tools serialize actual `name`, `description`, `inputSchema`, `outputSchema?`, `annotations?`. `connecting` and sanitized `last_error` are authoritative job state. Administrative connect/discover RPCs should call the asynchronous methods, return immediately, and poll `extensions.mcp.list` or `server_summary`; a user-started job deliberately outlives its initiating RPC response. At most sixteen active/closing connection jobs are allowed. Generation and cancellation latches prevent an older job from publishing after replacement, cancel, disconnect or shutdown. Metadata is cached only in memory; restart requires explicit connect/discovery. `shutdown()` permanently stops the service, cancels pending discoveries and active calls, and disconnects owned processes. `disconnect_mcp(id)` also cancels pending discovery for that ID. `disconnect_all()` cancels pending jobs as well as active calls; call it during vault lock and when entering offline mode.

`ExtensionSecrets` supplies value-free `version(server_id, name)` and atomic `resolve_versioned(server_id, name, expected)` methods; production `VaultSecrets` uses `VaultService::secret_version` and `resolve_versioned` with exact `mcp:<id>` consumers. Credential versions are sealed in tool plans, never exposed in the model catalog. Rotation invalidates a connected credential binding and requires explicit reconnect. Discovery rejects reflected credential values in server metadata and schemas; tool result strings and keys redact echoed values. Every HTTP exchange checks the live network/vault admission gate before resolving headers, before sending, while reading, and while waiting. A denied gate stops the caller promptly; an already blocked HTTP worker remains bounded by its configured timeout.

`MarketplaceCatalog` contains `schema_version: 1`, `catalog_version`, `entries: [{id,name,version,description,package_digest,skill_names,permissions}]`. Shipped packages are first-party instruction bundles, not downloaded third-party code. Browsing is passive. Version/digest validation and explicit trust plus nonempty scope grants precede installation. `PluginSummary` adds `digest` and `manifest_digest`; both are SHA-256 identifiers, with the latter used to preserve modified files during cleanup.

Stdio command/argv/cwd paths under `DrivePaths.root` store as `drive:relative/path` and resolve against the current root before launch. `RealConnector::for_drive(sandbox, &paths)` supplies this resolution; `new(sandbox, scratch)` is a host-command/test composition without a portable drive root. Embedded connector composition must use `.with_network_gate(live_gate)`; `ExtensionService::open` supplies the live network and vault gate automatically.

HTTPS disconnect cancels local work and forgets the session; remote HTTP DELETE session termination is not implemented. Remote session resources remain subject to the server's own expiry/cleanup. OAuth, bidirectional sampling/elicitation, legacy SSE and resumable streams are not advertised.

Model tools:

```json
{"name":"skill_read","parameters":{"id":"string"}}
{"name":"mcp_tools","parameters":{"server_id":"string optional"}}
{"name":"mcp_call","parameters":{"server_id":"string","tool_name":"string","arguments":"object","catalog_revision":"string"}}
{"name":"mcp_stdio_call","parameters":{"server_id":"string","tool_name":"string","arguments":"object","catalog_revision":"string"}}
```

Remote input schemas support a conservative recursive subset of JSON Schema: object/properties/required/additionalProperties, array/items/maxItems/minItems, scalar type/enum/const, lengths and numeric bounds. Unsupported constraints and references are rejected at discovery, never silently ignored. Server annotations remain untrusted hints; all MCP calls are conservatively treated as potentially mutating and network-capable.
