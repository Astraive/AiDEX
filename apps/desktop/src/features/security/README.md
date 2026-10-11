# Security

Every Security panel reads actual Core or host reports. The approvals tab remains `LiveApprovalsPanel`; the overview uses service totals, bounded active-run summaries, network gate state, and the host capability report.

`VaultPanel` exposes encrypted-vault status, create/unlock/lock, and value-free secret metadata. `AddSecretForm` stores or replaces user-provided UTF-8 values through authenticated IPC and clears input state after submission. There is no frontend secret-read or handle-resolution API. Exact consumer policies mirror `aidex-vault`: provider, MCP, process, or Git host; no wildcard, prompt, model, agent, or shell consumers. Configured provider/MCP/runtime instances supply optional scope suggestions.

Known service encryption keys, encrypted preferences, and `oauth.*` account records are read-only in generic administration and managed through their domain lifecycle. User credentials remain editable. Lock requests cancel managed work and revoke handles. Locked/unavailable vaults cannot display secret metadata or retain an open credential form.

`AuditPanel` projects durable events to a small metadata-only shape before storing UI state. It excludes commands, messages, names, and event bodies; exports contain only displayed metadata. Cursor paging and subscriptions resume from the last accepted durable event and refresh retained bounds after gaps. `secret.stored/deleted` records contain only opaque references; the view does not expose their bodies.

`SandboxPanel` displays the actual backend, helper, per-requirement enforcement, mechanisms, and limitations. Watchdogs and broker mediation are distinct from native filesystem/network confinement. Capability maxima are not advertised as every process's active protection. Editable mount/allowlist/global-isolation APIs are unavailable and their controls are disabled with reasons. `PolicyRules` exposes the existing application offline gate and durable defaults for new native conversations instead of fabricated rules or grants.
