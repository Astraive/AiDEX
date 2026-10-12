"use client";

import { useId, useMemo, useState, type ComponentPropsWithRef, type ReactNode } from "react";
import { CheckIcon, ChevronDownIcon } from "../../icons/interface";
import { ClipboardCheckIcon } from "../../icons/system";
import { Badge, type BadgeTone } from "../../primitives/Badge";
import { cx } from "../../utils/cx";
import { Markdown, type MarkdownCodeBlock } from "../content/Markdown";
import { planLineCount, planPreview, splitPlanMarkdown } from "./planText";
import styles from "./PlanCard.module.css";

export type PlanStatus = "active" | "superseded" | "completed";

const STATUS: Record<PlanStatus, { label: string; tone: BadgeTone }> = {
  active: { label: "Active", tone: "accent" },
  completed: { label: "Completed", tone: "success" },
  superseded: { label: "Superseded", tone: "neutral" },
};

export type PlanCardProps = Omit<ComponentPropsWithRef<"article">, "children" | "title"> & {
  /** The plan's markdown. A leading heading becomes the card title unless `title` is set. */
  markdown: string;
  /** Overrides the title taken from the plan's first heading ("Proposed plan" when there is none). */
  title?: string;
  status?: PlanStatus;
  /** Footer buttons, e.g. Implement, Implement in a new chat, Copy, Download. */
  actions?: ReactNode;
  /** Compact header controls, e.g. a copy `IconButton`. */
  headerActions?: ReactNode;
  /** Passed to `Markdown`: links in the plan call it and never navigate. */
  onOpenLink?: (url: string) => void;
  codeBlockActions?: (block: MarkdownCodeBlock) => ReactNode;
  /** Plans longer than this many (estimated) lines start collapsed. Default 20. */
  collapseAfterLines?: number;
  /** Lines shown while collapsed. Default 10. */
  previewLines?: number;
  /** Controlled expansion; leave undefined to let the card manage it. */
  expanded?: boolean;
  defaultExpanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  titleLevel?: 2 | 3 | 4;
};

/**
 * A proposed plan: "Plan" label, title, status badge, the plan rendered with `Markdown`, and
 * actions. Long plans show a preview with "Show full plan".
 */
export function PlanCard({
  markdown,
  title,
  status = "active",
  actions,
  headerActions,
  onOpenLink,
  codeBlockActions,
  collapseAfterLines = 20,
  previewLines = 10,
  expanded,
  defaultExpanded = false,
  onExpandedChange,
  titleLevel = 3,
  className,
  ...props
}: PlanCardProps) {
  const titleId = useId();
  const bodyId = useId();
  const Title = `h${titleLevel}` as const;
  const parts = useMemo(() => splitPlanMarkdown(markdown), [markdown]);
  const canCollapse = planLineCount(parts.body) > collapseAfterLines;
  const preview = useMemo(
    () => (canCollapse ? planPreview(parts.body, Math.min(previewLines, collapseAfterLines)) : parts.body),
    [canCollapse, parts.body, previewLines, collapseAfterLines],
  );
  const [internal, setInternal] = useState(defaultExpanded);
  const open = expanded ?? internal;
  const collapsed = canCollapse && !open;
  const toggle = () => {
    const next = !open;
    if (expanded === undefined) setInternal(next);
    onExpandedChange?.(next);
  };
  const statusInfo = STATUS[status];

  return (
    <article
      aria-labelledby={titleId}
      data-status={status}
      className={cx(styles.card, status === "superseded" && styles.superseded, className)}
      {...props}
    >
      <header className={styles.header}>
        <Badge tone="neutral" size="sm" icon={<ClipboardCheckIcon />} className={styles.kind}>
          Plan
        </Badge>
        <Title id={titleId} className={styles.title}>
          {title ?? parts.title ?? "Proposed plan"}
        </Title>
        <Badge
          tone={statusInfo.tone}
          size="sm"
          icon={status === "completed" ? <CheckIcon /> : undefined}
          className={styles.status}
        >
          {statusInfo.label}
        </Badge>
        {headerActions ? <div className={styles.headerActions}>{headerActions}</div> : null}
      </header>
      {parts.body ? (
        <div id={bodyId} className={cx(styles.body, collapsed && styles.collapsed)}>
          <Markdown
            source={collapsed ? preview : parts.body}
            headingLevelOffset={titleLevel}
            onOpenLink={onOpenLink}
            codeBlockActions={codeBlockActions}
          />
        </div>
      ) : null}
      {parts.body && canCollapse ? (
        <button
          type="button"
          className={styles.toggle}
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={toggle}
        >
          {open ? "Show less" : "Show full plan"}
          <ChevronDownIcon size={16} className={cx(styles.toggleIcon, open && styles.toggleIconOpen)} />
        </button>
      ) : null}
      {actions ? <div className={styles.actions}>{actions}</div> : null}
    </article>
  );
}
