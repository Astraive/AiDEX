// Paging for the approval center. Without a `run_id`, `approval.list` lists approvals grouped by
// conversation, newest conversation first (`conversation.list` order), and each conversation's in request
// order. So the panel pages forward from offset 0, which holds the newest conversations, and orders each
// page by approval id: ids are `approval_<ULID>`, which sort by creation time.
import type { Approval, ApprovalList } from "@aidex/client/dto/sessions";

/** Approvals per page. */
export const APPROVAL_PAGE_SIZE = 20;

/** The `offset` and `limit` sent to `approval.list`. */
export type ApprovalRange = { offset: number; limit: number };

/** The first page: offset 0, the newest conversations. */
export const firstApprovalRange: ApprovalRange = { offset: 0, limit: APPROVAL_PAGE_SIZE };

export type ApprovalPageView = {
  /** The page's approvals, newest request first. */
  approvals: Approval[];
  /** Position of the first listed approval in the daemon's order, from 0, and one past the last. */
  first: number;
  end: number;
  total: number;
  /** The range before this page (toward offset 0), or `undefined` on the first page. */
  previous?: ApprovalRange;
  /** The range after this page, or `undefined` on the last page. */
  next?: ApprovalRange;
};

/** `approvals` newest first by id. Ids share the `approval_` prefix, so plain string order is time order. */
export const newestApprovalsFirst = (approvals: readonly Approval[]): Approval[] =>
  [...approvals].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));

/**
 * Positions and neighbours of one `approval.list` page. A page trimmed to fit one message loses its
 * first items, so it is counted from its end; Previous then ends where the shown items start, so a
 * trimmed item is always reachable and no request falls between pages.
 */
export function approvalPageView(page: ApprovalList, size: number = APPROVAL_PAGE_SIZE): ApprovalPageView {
  const total = page.total_approvals;
  const end = page.next_offset ?? total;
  const first = Math.max(0, end - page.approvals.length);
  return {
    approvals: newestApprovalsFirst(page.approvals),
    first,
    end,
    total,
    previous: first > 0 ? { offset: Math.max(0, first - size), limit: Math.min(size, first) } : undefined,
    next: page.next_offset == null ? undefined : { offset: page.next_offset, limit: size },
  };
}
