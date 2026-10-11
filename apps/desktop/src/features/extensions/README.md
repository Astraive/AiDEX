# Extensions

Skills, plugins, and MCP instances use the `extensions.*` daemon API documented in `crates/extensions/API.md`. Lists contain installed records only; enable switches await persisted writes and refetch on success.

- Skills import complete SKILL.md content with explicit trust/workspace/profile grants. Editing sends the saved digest to detect concurrent changes. Plugin-contributed skills are read-only here and updated by replacing their bundle.
- Plugins browse a versioned first-party catalog bundled with AiDEX; installation binds the reviewed package version/digest and requires explicit trust plus nonempty workspace/profile grants. External bundles can also be imported from reviewed JSON manifests and declared skill-content maps. Enable/remove actions affect the installed contributions. The bundled catalog is distinct from an online vendor marketplace.
- MCP configuration contains literal stdio executable/argv or an HTTPS URL, bounds, grants, and vault secret references. Connect/test/discover/restart/disconnect/cancel operate on the selected registered workspace and exact permission profile. Full access is explicit. Tools, invocation names, and schemas come from actual discovery; connection errors are shown directly. HTTPS and stdio call tools expose their separate approval and permission requirements.
- `WebSearchPanel` connects Brave or Tavily search for native agent tools. Metadata and API keys use `web.*`; keys are cleared from the form after saving and never stored in preferences. Credential presence remains distinct from live provider verification.

Empty grants deny access. `*` explicitly grants all workspaces or profiles. Lists and sidebars do not start MCP processes. Browser OAuth and fabricated server logs are not presented as available MCP features.

MCP connect/discover may return an asynchronous `connecting` summary. The panel polls only while a connection job is pending, keeps cancellation available after the start RPC returns, and distinguishes a requested connection from a completed tool catalog.
