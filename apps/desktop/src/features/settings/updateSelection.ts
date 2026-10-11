import type { BundleTarget, CheckReport } from "@aidex/client/dto/updates";
export function absoluteUpdateSource(
  source: string,
  target: BundleTarget,
): boolean {
  if (!source || /[\x00-\x1f]/.test(source) || /^[a-z]+:\/\//i.test(source))
    return false;
  return target.startsWith("windows-")
    ? /^[A-Za-z]:[\\/]/.test(source) || /^\\\\[^\\/]+[\\/][^\\/]+/.test(source)
    : source.startsWith("/");
}
export function applySelectionReason(
  check: CheckReport | undefined,
  target: BundleTarget | undefined,
  pending: boolean,
): string | undefined {
  if (pending) return "An update is pending recovery or completion.";
  if (!check || !target) return "Check a signed package first.";
  if (check.candidate.signature.status !== "verified")
    return "A trusted signature is required.";
  if (check.candidate.target !== target)
    return "The package targets a different host.";
  if (check.action === "up_to_date")
    return "This signed package is already active and verified.";
}
export function sameCandidate(
  before: CheckReport,
  after: CheckReport,
): boolean {
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
