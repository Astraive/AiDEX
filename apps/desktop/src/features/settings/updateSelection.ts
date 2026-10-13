// Pure checks behind the Updates panel: which manual sources it sends to `update.check`, when a checked
// package may be applied, and whether a re-check still describes the package the user reviewed.
import type { BundleTarget, CheckReport } from "@aidex/client/dto/updates";

/**
 * Whether `source` is an absolute local path on a host of `target`: a drive-letter path on Windows, a
 * `/` path elsewhere. URLs, relative paths, and control characters are refused, and so are Windows
 * network shares and device paths (`\\server\share`, `\\?\`, `\\.\`): a package on a share is copied to
 * this computer, or the share mapped to a drive letter, first. There is no online release catalog.
 */
export function absoluteUpdateSource(source: string, target: BundleTarget): boolean {
  if (!source || /[\x00-\x1f]/.test(source) || /^[a-z][a-z0-9+.-]*:\/\//i.test(source)) return false;
  return target.startsWith("windows-") ? /^[A-Za-z]:[\\/]/.test(source) : source.startsWith("/");
}

/** Why a checked package cannot be applied, or `undefined` when it can. */
export function applySelectionReason(
  check: CheckReport | undefined,
  target: BundleTarget | undefined,
  pending: boolean,
): string | undefined {
  if (pending) return "An update is unfinished. AiDEX Core must recover it before another package is applied.";
  if (!check || !target) return "Check a signed package first.";
  if (check.candidate.signature.status !== "verified") return "A trusted publisher signature is required.";
  if (check.candidate.target !== target) return "This package is built for a different computer.";
  if (check.action === "up_to_date") return "This signed package is already the selected version.";
  return undefined;
}

/** Whether a re-check describes the same signed package (path, target, version, manifest, and signer). */
export function sameCandidate(before: CheckReport, after: CheckReport): boolean {
  return (
    before.package === after.package &&
    before.candidate.target === after.candidate.target &&
    before.candidate.version === after.candidate.version &&
    before.candidate.manifest_sha256 === after.candidate.manifest_sha256 &&
    before.candidate.signature.status === "verified" &&
    after.candidate.signature.status === "verified" &&
    before.candidate.signature.key_id === after.candidate.signature.key_id
  );
}
