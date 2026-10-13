# Approvals feature

Human decisions on sensitive operations requested through the execution broker. The approval center renders inside the Security page's Approvals tab; conversations show the same requests inline.

| File | Shows |
| --- | --- |
| `LiveApprovalsPanel.tsx` | The approval center. Pill views Pending, Approved, Denied, Expired, and All requests, each filtered by the daemon (`approval.list` with `status`), so page counts and totals match the view. Pages of 20 start at offset 0 and move with Previous and Next; a note under the pager says how requests are ordered. A locked vault shows a warning with Go to unlock instead of an error. Decisions keep a local busy and error state and re-read the list and the Security counts. |
| `approvalPage.ts` | Paging and order for the panel. Without a `run_id`, `approval.list` groups approvals by conversation, newest conversation first, each conversation's in request order, so offset 0 holds the newest conversations. `approvalPageView` sorts a page by approval id, newest first (ids are `approval_<ULID>`, which sort by creation time), and works out the previous and next ranges. A page trimmed to fit one message loses its first items, so it is counted from its end and Previous ends where its shown items start; a trimmed request is never skipped. |
| `ApprovalRequestCard.tsx` | One request as the daemon reports it: tool, origin (AiDEX Native or the external runtime, and the run) when `showOrigin` is set, status, the arguments and change preview as code (untrusted text), a warning when either is truncated, and Deny / Approve once while it is pending. Exported for the conversation (`features/chat`). |
| `approvalDecision.ts` | `approvalDecision(approval, approved, commandId)` builds the `approval.respond` params: the exact id and fingerprint plus a fresh command id. It refuses requests that are no longer pending and refuses to approve (but allows denying) truncated requests; `approvable` says whether Approve once is enabled. |

Nothing is decided locally: every decision goes to `approval.respond`, which re-checks the fingerprint, and the daemon owns expiry and process loss. There is no "remember for this session" grant. Remote approvals (mobile, CLI) will need scoped, authenticated commands.

Pages follow conversations, not request time: an older conversation's new request sits on a later page than a newer conversation's old one. Each page is newest first.

`approvalPage.test.ts` and `approvalDecision.test.ts` run with `bun run --cwd apps/desktop test`.
