import { test } from "node:test";
import assert from "node:assert/strict";
import type { Approval, ApprovalList } from "@aidex/client/dto/sessions";
import { APPROVAL_PAGE_SIZE, approvalPageView, firstApprovalRange, newestApprovalsFirst } from "./approvalPage";

const approval = (id: string): Approval => ({
  id,
  run_id: "run-1",
  tool_name: "write_file",
  arguments: {},
  arguments_bytes: 2,
  arguments_truncated: false,
  fingerprint: `fingerprint-${id}`,
  status: "pending",
  preview: null,
  preview_truncated: false,
  runtime_id: null,
  runtime_request_id: null,
  runtime_ref_omitted: true,
});

const page = (ids: string[], offset: number, total: number, nextOffset: number | null, trimmed = false): ApprovalList => ({
  approvals: ids.map(approval),
  offset,
  next_offset: nextOffset,
  total_approvals: total,
  truncated: trimmed || nextOffset != null || offset > 0,
  ...(trimmed ? { approvals_truncated: true as const } : {}),
});

// ULIDs: the first 10 characters are the creation millisecond, so later requests sort higher.
const older = "approval_01J9Z0000000000000000000AA";
const middle = "approval_01J9Z0000100000000000000AA";
const newer = "approval_01J9Z0000200000000000000AA";

test("each page lists its requests newest first, across the conversations it holds", () => {
  // Conversation B (newer) first, its requests in request order; then conversation A.
  const view = approvalPageView(page([middle, newer, older], 0, 3, null));
  assert.deepEqual(
    view.approvals.map((item) => item.id),
    [newer, middle, older],
  );
  assert.deepEqual(newestApprovalsFirst([]), []);
});

test("paging starts at offset 0, the newest conversations, and moves forward", () => {
  assert.deepEqual(firstApprovalRange, { offset: 0, limit: APPROVAL_PAGE_SIZE });
  const ids = Array.from({ length: 20 }, (_, index) => `approval_${String(index).padStart(26, "0")}`);

  const firstPage = approvalPageView(page(ids, 0, 45, 20));
  assert.equal(firstPage.first, 0);
  assert.equal(firstPage.end, 20);
  assert.equal(firstPage.previous, undefined);
  assert.deepEqual(firstPage.next, { offset: 20, limit: 20 });

  const secondPage = approvalPageView(page(ids, 20, 45, 40));
  assert.deepEqual(secondPage.previous, { offset: 0, limit: 20 });
  assert.deepEqual(secondPage.next, { offset: 40, limit: 20 });

  const lastPage = approvalPageView(page(ids.slice(0, 5), 40, 45, null));
  assert.equal(lastPage.first, 40);
  assert.equal(lastPage.end, 45);
  assert.equal(lastPage.next, undefined);
  assert.deepEqual(lastPage.previous, { offset: 20, limit: 20 });
});

test("a page trimmed to fit one message keeps its dropped requests reachable through Previous", () => {
  // Offset 20, limit 20: the daemon dropped the first five items of [20, 40) to fit the wire budget.
  const trimmed = approvalPageView(page(Array.from({ length: 15 }, (_, index) => `approval_${index}`), 20, 60, 40, true));
  assert.equal(trimmed.first, 25);
  assert.equal(trimmed.end, 40);
  assert.deepEqual(trimmed.previous, { offset: 5, limit: 20 });
  assert.deepEqual(trimmed.next, { offset: 40, limit: 20 });

  // Near the start, Previous covers only what lies before the shown items.
  const nearStart = approvalPageView(page(["approval_a", "approval_b"], 0, 10, 5, true));
  assert.equal(nearStart.first, 3);
  assert.deepEqual(nearStart.previous, { offset: 0, limit: 3 });
});

test("an empty or out-of-range page still points back toward the newest requests", () => {
  assert.deepEqual(approvalPageView(page([], 0, 0, null)), {
    approvals: [],
    first: 0,
    end: 0,
    total: 0,
    previous: undefined,
    next: undefined,
  });
  // Decisions shrank a filtered view under the open page: the daemon clamps the offset to the total.
  const emptied = approvalPageView(page([], 12, 12, null));
  assert.equal(emptied.first, 12);
  assert.deepEqual(emptied.previous, { offset: 0, limit: 12 });
});
