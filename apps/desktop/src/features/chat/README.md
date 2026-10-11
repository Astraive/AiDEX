# Conversations

`ChatPage` and `AgentPage` share daemon-backed `ConversationPage`. New sessions inherit encrypted drive defaults; the task settings dialog overrides instructions, provider fallback, permissions, and step/token limits for one conversation.

Native chat/plan/agent modes use `model.list` and the managed local inference state. External Agent sessions select an enabled runtime instance and an exact runtime-owned model from `runtime.models` or explicit input. They use the reserved `provider_id: runtime` binding, independently of cloud provider accounts and native model defaults. Descriptor modes/permissions gate submission; no unsupported scope is silently promoted.

Conversation messages, once-only fingerprinted approvals, cancellation, and interrupted resume use typed Core RPCs. Scoped events refresh the saved snapshot, with bounded polling as a fallback. Full content and older history use paginated `history.list/read`. Project-file attachments are read from the registered workspace and rejected when binary, truncated, or invalid UTF-8.

The composer skill button selects installed, trusted, user-invocable skills for the chosen project and permission profile. IDs persist through conversation configuration and native defaults; unavailable or changed grants block use instead of replacing a skill. Projectless chat sends an empty selection and receives no agent skill/tool injection. Automatic prompt handoff waits for saved defaults and the selected skill catalog.
