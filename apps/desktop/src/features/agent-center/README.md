# Agent center

Agents and runtime instances are read from AiDEX Core. `AgentCenterPage` shows AiDEX Native and saved `runtime.list` instances. Runtime installation uses verified bundled components; executable presence is displayed independently from protocol and authentication readiness.

`AgentConfigForm` saves native instructions, provider model/fallback, permissions, and limits through encrypted `settings.set`. `RuntimeConfigForm` saves literal executable/argv configuration and vault references through `runtime.add/update`; readiness/model discovery and ChatGPT sign-in use the runtime RPCs. Set-as-default persists `runtime_id` and `runtime_model_id`, independently of the native provider `model_id`.

Codex also offers official CLI-managed sign-in, safe flow progress/cancel/logout, and import of an explicitly entered absolute auth.json path through `RuntimeAccountPanel`. The CLI owns browser login in an isolated home; no host credential directory is discovered. Runtime settings are frozen while its account flow is pending. Profile/config/model views refresh after account changes. Shared SIWC remains a separate authentication alternative.

Native defaults persist explicit `skill_ids`. Skill selection requires enabled, trusted, user-invocable metadata; task workspace and permission grants are checked again when those defaults are used. Manual-only skills remain selectable by the user.

The conversation composer selects the instance and its own model catalog, uses `execution.status.runtime_descriptors` to constrain modes and permissions, and sends external sessions in the reserved `provider_id: runtime` namespace. Full access is an explicit user choice. Unknown or disabled instances cannot silently fall back to another runtime.

Live account sign-in and inference remain account-specific verification; configuration and a binary/model catalog alone do not prove inference eligibility.
