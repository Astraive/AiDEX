// Sample data until AiDEX Core exposes the policy engine, vault, and audit log through the daemon.
// The sandbox capability matrix is live (`sandbox_capabilities`); the selected level is local state.
// Secret values are never present: only masked fingerprints that could not be used as credentials.
import type { BackendKind, SandboxLevel } from "@aidex/client/dto/sandbox";

/** The sandbox levels of crates/sandbox (`direct` 0 through `ephemeral` 3). */
export type IsolationLevel = SandboxLevel;

export type ActivePermission = { id: string; agent: string; permission: string; scope: string; granted: string };

export type AuditResult = "Allowed" | "Approved" | "Denied" | "Sandboxed" | "Awaiting approval";

export type AuditEntry = { id: string; time: string; actor: string; action: string; target: string; result: AuditResult };

export type SecretType = "API key" | "Token" | "SSH key";

export type VaultSecret = { id: string; name: string; type: SecretType; scope: string; lastUsed: string; masked: string };

export type ConnectedAccount = { id: string; name: string; detail: string; status: { label: string; tone: "success" | "warning" } };

export type SandboxBackend = { id: BackendKind; platform: string; mechanism: string; detail: string };

export type FilesystemRule = { id: string; path: string; access: "Read" | "Read and write" | "Deny"; scope: string };

export type NetworkRule = { id: string; domain: string; policy: "Allow" | "Deny" | "Ask"; scope: string };

export type SecurityAlert = { id: string; label: string; tab: "overview" | "vault" | "sandbox" | "approvals" };

export const policyMode = { value: "Standard", hint: "Destructive and remote actions need approval" };

export const isolationLabels: Record<IsolationLevel, { value: string; hint: string }> = {
  direct: { value: "Direct", hint: "Commands run directly as you" },
  brokered: { value: "Brokered", hint: "Broker, cleared environment, tree kill" },
  restricted: { value: "Restricted", hint: "Native filesystem confinement · fails closed" },
  ephemeral: { value: "Ephemeral", hint: "Disposable environment · fails closed" },
};

export const samplePermissions: readonly ActivePermission[] = [
  { id: "p-native-edit", agent: "AiDEX Native", permission: "Edit files", scope: "ai-dex workspace", granted: "Today" },
  { id: "p-native-shell", agent: "AiDEX Native", permission: "Run shell commands, with approval", scope: "FinApp workspace", granted: "Today" },
  { id: "p-codex-net", agent: "Codex", permission: "Network: github.com", scope: "lattice workspace", granted: "Yesterday" },
  { id: "p-claude-secret", agent: "Claude Code", permission: "Use secret handle GITHUB_TOKEN", scope: "This session", granted: "Today" },
  { id: "p-test-read", agent: "Test agent", permission: "Read files", scope: "tony workspace", granted: "Tuesday" },
];

export const sampleAuditLog: readonly AuditEntry[] = [
  { id: "a1", time: "10:42 AM", actor: "AiDEX Native", action: "Edit file", target: "src/auth/session.ts", result: "Allowed" },
  { id: "a2", time: "10:39 AM", actor: "AiDEX Native", action: "Shell command", target: "git push origin feature/auth", result: "Awaiting approval" },
  { id: "a3", time: "10:31 AM", actor: "Codex", action: "Network request", target: "pastebin.com", result: "Denied" },
  { id: "a4", time: "10:12 AM", actor: "AiDEX Native", action: "Shell command", target: "cargo test --workspace", result: "Sandboxed" },
  { id: "a5", time: "9:41 AM", actor: "Claude Code", action: "File write", target: "~/.ssh/config", result: "Denied" },
  { id: "a6", time: "9:12 AM", actor: "Claude Code", action: "Secret use", target: "GITHUB_TOKEN (handle)", result: "Approved" },
];

export const sampleSecrets: readonly VaultSecret[] = [
  { id: "sec-openai", name: "OPENAI_API_KEY", type: "API key", scope: "All workspaces", lastUsed: "Today, 10:20 AM", masked: "sk-…3f9a" },
  { id: "sec-github", name: "GITHUB_TOKEN", type: "Token", scope: "All workspaces", lastUsed: "Today, 9:12 AM", masked: "ghp_…8c21" },
  { id: "sec-hf", name: "HF_TOKEN", type: "Token", scope: "tony", lastUsed: "Tuesday", masked: "hf_…71de" },
  { id: "sec-npm", name: "NPM_TOKEN", type: "Token", scope: "FinApp", lastUsed: "Yesterday", masked: "npm_…09fe" },
  { id: "sec-stripe", name: "STRIPE_TEST_KEY", type: "API key", scope: "FinApp", lastUsed: "3 days ago", masked: "sk_test_…a41c" },
  { id: "sec-lattice-deploy", name: "lattice-deploy", type: "SSH key", scope: "lattice", lastUsed: "Last week", masked: "SHA256:…Q2mT" },
  { id: "sec-olympus", name: "olympus-server", type: "SSH key", scope: "Olympus", lastUsed: "2 weeks ago", masked: "SHA256:…Vt4k" },
];

export const sampleAccounts: readonly ConnectedAccount[] = [
  { id: "openai", name: "OpenAI", detail: "Model provider · uses OPENAI_API_KEY", status: { label: "Connected", tone: "success" } },
  { id: "github", name: "GitHub", detail: "Git hosting · uses GITHUB_TOKEN · expires in 5 days", status: { label: "Expires soon", tone: "warning" } },
];

/** How each OS backend isolates processes (crates/sandbox/README.md enforcement matrix). */
export const sandboxBackends: readonly SandboxBackend[] = [
  {
    id: "windows_job_object",
    platform: "Windows",
    mechanism: "Job Objects",
    detail:
      "Each process tree runs in a Job Object with a cleared environment; process, memory, and CPU limits are watchdog-enforced. No filesystem or network confinement yet, so the maximum level is Brokered.",
  },
  {
    id: "linux_landlock",
    platform: "Linux",
    mechanism: "Landlock + namespaces",
    detail:
      "Landlock filesystem rules (Linux 6.2+), rlimits, and private user and network namespaces where the kernel allows; up to Restricted.",
  },
  {
    id: "macos_seatbelt",
    platform: "macOS",
    mechanism: "Seatbelt",
    detail: "A sandbox-exec profile that denies writes outside the mounts and network when requested, plus rlimits; up to Brokered.",
  },
];

export const sampleFilesystemRules: readonly FilesystemRule[] = [
  { id: "fs-workspace", path: "/workspace", access: "Read and write", scope: "Current workspace" },
  { id: "fs-tmp", path: "/tmp", access: "Read and write", scope: "Session scratch, discarded after" },
  { id: "fs-tools", path: "/tools", access: "Read", scope: "AiDEX tool bundle" },
  { id: "fs-ssh", path: "~/.ssh", access: "Deny", scope: "All agents" },
  { id: "fs-aws", path: "~/.aws", access: "Deny", scope: "All agents" },
  { id: "fs-home", path: "~/ (rest of home folder)", access: "Deny", scope: "All agents" },
];

export const sampleNetworkRules: readonly NetworkRule[] = [
  { id: "net-github", domain: "github.com", policy: "Allow", scope: "All workspaces" },
  { id: "net-openai", domain: "api.openai.com", policy: "Allow", scope: "Provider calls only" },
  { id: "net-npm", domain: "registry.npmjs.org", policy: "Allow", scope: "FinApp" },
  { id: "net-hf", domain: "huggingface.co", policy: "Ask", scope: "All workspaces" },
  { id: "net-local", domain: "Local network (LAN)", policy: "Deny", scope: "All agents" },
  { id: "net-other", domain: "Any other domain", policy: "Ask", scope: "All agents" },
];

export const sampleAlerts: readonly SecurityAlert[] = [
  { id: "al-pastebin", label: "Blocked request to pastebin.com", tab: "overview" },
  { id: "al-ssh", label: "Denied write to ~/.ssh/config", tab: "approvals" },
  { id: "al-token", label: "GITHUB_TOKEN expires in 5 days", tab: "vault" },
];
