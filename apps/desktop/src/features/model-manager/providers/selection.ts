// The providers screen keeps its selected row in the route id (`provider:<id>` or `runtime:<id>`),
// so the selection survives refetches and other screens can link straight to an account.

export type ProviderSelectionKind = "provider" | "runtime";
export type ProviderSelection = { kind: ProviderSelectionKind; id: string };

export function providerSelectionId(kind: ProviderSelectionKind, id: string): string {
  return `${kind}:${id}`;
}

export function parseProviderSelection(routeId: string | undefined): ProviderSelection | null {
  if (!routeId) return null;
  const colon = routeId.indexOf(":");
  if (colon <= 0) return null;
  const kind = routeId.slice(0, colon);
  const id = routeId.slice(colon + 1);
  if (!id || (kind !== "provider" && kind !== "runtime")) return null;
  return { kind, id };
}
