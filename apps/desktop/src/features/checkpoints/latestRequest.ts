/**
 * Hands out request tokens where only the newest is current, so an answer that arrives after the
 * user picked something else (another checkpoint, conversation, or content page) is dropped
 * instead of replacing what is on screen.
 */
export type LatestRequest = {
  /** Start a request; its token is current until the next `begin` or `invalidate`. */
  begin(): number;
  isCurrent(token: number): boolean;
  /** Nothing started so far is current any more. */
  invalidate(): void;
};

export function createLatestRequest(): LatestRequest {
  let latest = 0;
  return {
    begin: () => ++latest,
    isCurrent: (token) => token === latest,
    invalidate: () => {
      latest += 1;
    },
  };
}
