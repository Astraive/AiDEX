# Extensions feature

The Plugins screen (route `extensions`, tabs `skills`, `mcp`, `plugins`, `web`) and its sidebar. Everything is composed from `@aidex/ui` and reads live AiDEX Core data through the `skill.*`, `mcp.*`, `plugin.*`, `web.*`, and `vault.secret.*` methods (`packages/client` DTOs). The route `id` selects the item a tab shows (a skill id, an MCP server id, or a plugin name), so sidebar shortcuts open a tab with that item selected.

| File | Shows |
| --- | --- |
| `ExtensionsPage.tsx` | Header, the AiDEX Core status notice, and underline tabs with live counts. Panels render only while AiDEX Core runs. |
| `ExtensionsSidebar.tsx` | Browse (the four tabs with counts), plus shortcuts to running MCP servers and enabled plugins. |
| `data.ts` | `useExtensionsQuery` (one shared answer per request for the page, panels, and sidebar; kept while reloading; optional polling), `invalidateExtensions` after every change, `useAction` (busy and error state per control group), `useOpenLink` (`web.open`), `useVaultSecrets`, and the shared workspace choice for project and repo skills. |
| `vaultSecrets.ts` | Lists vault secret names and consumers, and stores a value for one consumer while keeping the consumers and kind the secret already has. |
| `WorkspaceSelect.tsx` | Workspace picker used by the Skills tab and the `.mcp.json` import. |
| `skills/SkillsPanel.tsx` | Search (ranked like t3code: name prefix, name, then description), scope chips (All, Drive, Project, Repo, Plugin), workspace choice, skill rows with scope badge, `$name`, error state and enable switch, and the detail pane. |
| `skills/SkillDetailPane.tsx` | Who can invoke the skill, its source, location and size, front matter, and the instructions rendered with `Markdown`; Edit and Delete (with confirmation) for drive skills. Edit is off, with a notice to edit the SKILL.md file instead, when `skill.show` reports `body_redacted` (the shown text hides credential-like text, so saving it would overwrite the original). |
| `skills/SkillEditor.tsx` | "New skill" / "Edit" dialog: name, version, one-line description, and Markdown instructions beside a live preview; saves with `skill.save`, which keeps the skill's other front matter keys (listed under the form). |
| `mcp/McpPanel.tsx` | Server rows with a brand glyph (or the MCP glyph), status, tool count, source badge, and enable switch; Add server and Import from `.mcp.json`. Polls quickly while a server starts. |
| `mcp/McpServerDetail.tsx` | Start, Stop, Restart, Edit configuration, Remove; endpoint, source, workspaces, server identity; the tool list (with auto-approved tools marked; a `mcp.tools` answer is dropped when the configuration changes or the run it listed stops, fails, or restarts); secrets and log. |
| `mcp/McpServerForm.tsx` | Add or edit a server: stdio command, arguments (one per line), environment allowlist, `VARIABLE=secret-name` mappings; HTTP URL and `Header=secret-name` mappings; workspaces; tools that run without asking. Secret values are never part of the file. |
| `mcp/McpSecrets.tsx` | Each mapped vault secret, whether it is stored and released to `mcp:<id>`, and a field that stores a value. |
| `mcp/McpLogs.tsx` | The server's recent log in a `Terminal`, polled while it runs. |
| `mcp/McpImportDialog.tsx` | Previews a workspace's `.mcp.json` (`dry_run`) and imports its servers turned off. |
| `plugins/PluginsPanel.tsx` | Installed plugins with version, trust badge, and enable switch (turning on an untrusted plugin opens the review); the selected plugin's contents; Uninstall with confirmation. |
| `plugins/PluginDialogs.tsx` | Install from a folder or `.zip` path (offering to replace an installed plugin), then the review; the review for an installed plugin; both end in "Trust and enable", which trusts exactly the tree hash shown. |
| `plugins/PluginContents.tsx` | Manifest facts, tree hash, declared permissions, skills, MCP servers with their commands, ignored parts, and the README. |
| `web/WebPanel.tsx` | Enable web search, read web pages, results per search, provider tiles, and the chosen provider's key (stored with `vault.secret.set` for `process:web`, then named in `web.settings.set`) or SearXNG endpoint, with "Get an API key" and documentation links through `web.open`. |
| `web/WebTestSearch.tsx` | Runs `web.test` with the saved settings and shows the results with `WebResultList`. |

Secret values (MCP secrets, web search keys) leave React state as soon as they are sent and go only to `vault.secret.set`. Skill bodies, plugin READMEs, MCP tool descriptions, and search results are untrusted text: they are rendered with `Markdown` or as plain text, and their links open only through `web.open`, which the daemon validates.
