// Mirrors `aidex_auth::{StartResult, AuthStatus, AuthState}` (crates/auth/src/service.rs), returned
// by the `auth.*` methods (crates/aidexd/src/methods/auth.rs) and by `runtime.auth.*` for a Codex
// runtime that shares a ChatGPT sign-in. Safe metadata only: no token values or upstream error
// bodies. Hand-written until schema generation exists.

export type AuthState =
  | "pending"
  | "connected"
  | "failed"
  | "cancelled"
  | "expired"
  | "disconnected";

/** Result of `auth.start` and `runtime.auth.start`. `expires_at` is Unix milliseconds. */
export type AuthFlowStart = {
  flow_id: string;
  provider_id: string;
  authorization_url: string;
  expires_at: number;
};

/**
 * Result of `auth.status`, `auth.cancel`, `auth.disconnect`, `runtime.auth.status` and
 * `runtime.auth.cancel`. Poll `status` until `state` leaves `pending`.
 */
export type AuthFlowStatus = {
  state: AuthState;
  provider_id: string;
  account_label: string | null;
  error: string | null;
};
