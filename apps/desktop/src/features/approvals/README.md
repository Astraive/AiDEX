# Approvals feature

Human decisions on sensitive operations requested through the execution broker. The Approvals Center renders inside the Security page's Approvals tab.

| File | Shows |
| --- | --- |
| `ApprovalsPanel.tsx` | Pill tabs for Pending and History. Pending requests render as cards, with an empty state when none are left; history is a table of decisions. |
| `ApprovalCard.tsx` | One request: summary, requested command, effect, agent, workspace, risk, request time, "Remember for this session", and Deny / Approve. |
| `RiskBadge.tsx` | Risk level as text and icon (High, Medium, Low), never color alone. |
| `useApprovals.ts` | Local queue: deciding moves a request to history with its decision. The Security page owns it so the dashboard and tab count reflect decisions. |
| `sample.ts` | Sample requests until AiDEX Core exposes the approval service (`crates/approvals`). |

Decisions are local UI state only and show a "Preview: …" toast; nothing reaches the broker until Core is connected. Remote approvals (mobile, CLI) will need scoped, authenticated commands.
