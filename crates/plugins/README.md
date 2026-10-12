# aidex-plugins

Portable plugin bundles of skills and MCP servers, compatible with the Claude Code
plugin layout. A plugin is data, never native code that AiDEX runs: AiDEX loads its
skills and offers its MCP servers as disabled entries; nothing in a plugin executes
because it was installed or enabled.

```text
plugins/<name>/
  aidex-plugin.json   or   .claude-plugin/plugin.json   (manifest)
  skills/<skill>/SKILL.md
  .mcp.json                                             (optional MCP servers)
  README.md
```

## API

- `PluginStore::open(&DrivePaths)` opens the store over `DrivePaths::plugins()` and
  `private_config/plugins.toml`; `with_events(sink)` emits `extension.enabled` /
  `extension.disabled` (kind `plugin`). Operations are serialized by an internal lock.
- `install(source, replace)` installs an absolute folder or `.zip` path, disabled.
  The source is staged under the drive's `tmp/`, validated, hashed, and moved into
  `plugins/<name>/`. An installed name is replaced only with `replace`; trust carries
  over only when the new tree hash is identical, and the plugin is disabled either way.
- `list()` / `show(name)` re-hash every plugin and return `PluginSummary` /
  `PluginDetail` (manifest, `PluginContents`, current, recorded, and trusted hashes).
  Directories under `plugins/` without a provenance record are listed as
  `unregistered` and never contribute.
- `trust(name, tree_sha256, enable)` trusts the exact hash the user reviewed; it fails
  with `hash_mismatch` when the files no longer hash to it. Trusting a changed tree
  re-validates it and makes it the recorded baseline.
- `set_enabled(name, enabled)`: enabling requires trust in the current tree.
- `uninstall(name)` removes the files (also of an unregistered directory) and the record.
- `enabled_contributions()` returns `PluginContribution { name, root, skills_dir,
  mcp_json }` for plugins that are enabled, trusted, and whose files still hash to the
  trusted tree. `aidex-skills` reads `skills_dir` as `plugin`-scope skills;
  `aidex-mcp` imports `mcp_json` as disabled server entries.
- `parse_manifest`, `parse_mcp_json`, and `tree_sha256` are exposed for reuse and tests.

Errors are `PluginError` with a `ServiceErrorCode` and a stable `reason()`
(`invalid_request`, `invalid_manifest`, `unsafe_content`, `executable_refused`,
`too_large`, `plugin_not_found`, `plugin_exists`, `hash_mismatch`, `not_trusted`,
`integrity_failed`, `invalid_config`, `io`, `event_not_recorded`). Messages name
plugin-relative paths, fields, and limits; they never quote manifest values or file
contents.

## Manifest

```json
{ "name": "code-review", "version": "1.2.0", "description": "Reviews code.",
  "author": {"name": "Ada", "url": "https://example.com"},
  "homepage": "https://example.com/plugin", "license": "MIT",
  "permissions": {"network": ["api.github.com"], "secrets": ["github-token"], "shell": false} }
```

`name` is a slug `[a-z0-9][a-z0-9-]{0,63}` (also the directory name); `version` starts
with a digit and uses `0-9 A-Z a-z . + -`; `description` is at most 1 KiB. URLs must be
`http(s)` without user information. Permissions are declarations shown for review; MCP
servers still pass AiDEX policy for every network request, secret, and process. Other
keys (Claude Code's `commands`, `agents`, `hooks`, `mcpServers`, ...) are reported in
`contents.ignored_fields` and never used; `agents/`, `commands/`, `hooks/`, and
`output-styles/` directories are reported as `unsupported_components`. When both
manifest files exist, `aidex-plugin.json` wins.

## Install rules

- Sources: an absolute path to a real folder or a `.zip` file (at most 48 MiB and
  8192 entries). A plugin may sit at the archive root or in its single top-level folder;
  `__MACOSX/` entries are skipped. The drive's private directories are refused.
- Limits: at most 32 MiB of files, 4096 files, and 4096 directories; paths at most 200
  bytes and 16 levels deep.
- Refused: symbolic links and junctions (in the folder, as the source, or as archive
  entries), special files, absolute, drive-qualified, backslash-separated, or
  traversing names, Windows-reserved device names, trailing dots or spaces, names that
  collide ignoring case, and archive entries whose extracted size differs from the
  declared size.
- Executables: files with a launchable extension (`.exe`, `.dll`, `.bat`, `.cmd`,
  `.ps1`, `.msi`, `.so`, `.jar`, ...) or PE, ELF, or Mach-O content are refused unless
  `.mcp.json` names that exact file as a server command (`${CLAUDE_PLUGIN_ROOT}/...`,
  `${AIDEX_PLUGIN_ROOT}/...`, or `./...`). Scripts (`.py`, `.js`, `.sh`) are data.
  Copies never keep permission bits; on Unix only declared server commands are made
  executable.

## Tree hash and trust

```text
SHA-256( "aidex-plugin-tree-v1\n"
         || for each regular file, sorted by its UTF-8 relative path (byte order):
            <path> "\0" <size in decimal> "\0" <SHA-256 of the content, hex> "\n" )
```

Directories carry no content and are not hashed, so a folder and a `.zip` of the
same files hash identically. Every `list`, `show`, `trust`, `set_enabled`, and
`enabled_contributions` re-hashes the plugin: when its files are missing, invalid, or
differ from the recorded tree, its trust is revoked and it is disabled (with
`extension.disabled`). Reverting the files does not restore trust. Files that are
merely unreadable (an I/O error) contribute nothing for that call without losing trust.

## Provenance file

`private_config/plugins.toml` is a new, closed-schema file (version 1; unknown keys
and other versions are refused, and errors never quote values):

```toml
version = 1

[[plugins]]
name = "code-review"
version = "1.2.0"
tree_sha256 = "<64 hex>"            # recorded tree: install, or the last trusted tree
installed_at = "2026-10-11T08:00:00.000Z"
source = "D:\\Downloads\\code-review.zip"
trusted_sha256 = "<64 hex>"         # optional; equals tree_sha256 while trusted
enabled = false
```

## Review contents

`PluginContents` lists skills (`skills/<dir>/SKILL.md`, else `skill.md`, else another
letter case of it, skipping hidden directories: the rule skill discovery loads them by;
with best-effort front-matter `name`/`description`), MCP servers from `.mcp.json`
(`{"mcpServers": {name: {command, args, env, url, headers, type}}}` or the bare map,
which `aidex-mcp` imports for plugins too: name, transport,
command, arguments, URL, the plugin file a command runs, and environment variable and
header *names*), ignored manifest keys, unsupported components, file count, total
bytes, and the start of `README.md`. Environment and header values are never read or
returned; commands, arguments, URLs, and manifest text are redacted with
`aidex_events::redact_text`, and control characters in skill text, arguments, and the
README (other than its line breaks and tabs) are replaced with U+FFFD. A refusal that
names a plugin path shows its control characters escaped (`\r`, `\u{1b}`).

## Daemon and CLI

`aidexd` serves `plugin.list`, `plugin.show`, `plugin.install`, `plugin.trust`,
`plugin.set_enabled`, and `plugin.uninstall` (`protocol/commands/README.md`); the CLI
exposes `aidex plugins list|show|install|enable|disable|uninstall` (`apps/cli/README.md`).

## Limitations

- No signatures: trust is the user's explicit decision on an exact tree hash. Signed
  plugins (an `aidex-bundle`-style trust store) are future work.
- Declared permissions are informational; enforcement is AiDEX policy on each MCP
  server's processes, network requests, and secrets.
- No marketplace or remote install: sources are local folders and archives.
