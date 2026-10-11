/** Mirrors aidexd web.* metadata; secret values are never returned. */
export type SearchProvider = "brave" | "tavily";
export type WebStatus = {
  settings: { enabled: boolean; provider: SearchProvider; revision: number };
  credential_present: boolean;
  network_allowed: boolean;
  fetch_available: boolean;
};
