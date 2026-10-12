// URL helpers for content that came from a model, a web page, or another untrusted source.
// Pure functions only; `URL` exists in browsers and in Node, so these are safe during prerendering.

export type SafeUrlOptions = {
  /** Also accept `mailto:` addresses (default true). */
  allowMailto?: boolean;
};

/**
 * Returns the normalized URL when `raw` is an absolute `http:`, `https:` (or `mailto:`) URL without
 * embedded credentials, and null otherwise. Relative URLs, `javascript:`, `data:`, `file:`, and every
 * other scheme are refused. The WHATWG parser strips the tabs and newlines browsers ignore, so
 * obfuscated schemes such as `java\tscript:` are recognised and refused too.
 */
export function safeExternalUrl(raw: string | null | undefined, { allowMailto = true }: SafeUrlOptions = {}): string | null {
  if (typeof raw !== "string") return null;
  const value = raw.trim();
  if (!value || value.length > 8192) return null;
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return null;
  }
  if (parsed.protocol === "http:" || parsed.protocol === "https:") {
    // `https://user:pass@host` is a classic phishing disguise; links that need credentials are refused.
    if (parsed.username || parsed.password || !parsed.hostname) return null;
    return parsed.href;
  }
  if (allowMailto && parsed.protocol === "mailto:" && parsed.pathname.length > 0) return parsed.href;
  return null;
}

/** The host (without `www.`) of an http(s) URL, or null when the URL is not a safe external URL. */
export function urlHost(raw: string | null | undefined): string | null {
  const safe = safeExternalUrl(raw, { allowMailto: false });
  if (!safe) return null;
  const host = new URL(safe).host;
  return host.startsWith("www.") ? host.slice(4) : host;
}
