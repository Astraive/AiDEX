"use client";

import { Children, isValidElement, useState, type ComponentPropsWithRef, type ReactNode } from "react";
import { ChevronRightIcon, LoaderIcon } from "../../icons/interface";
import { Collapsible, CollapsiblePanel, CollapsibleTrigger } from "../../primitives/Collapsible";
import { StatusDot } from "../../primitives/StatusDot";
import { cx } from "../../utils/cx";
import a11y from "./a11y.module.css";
import styles from "./WorkLogRow.module.css";

export type WorkLogStatus = "running" | "completed" | "failed";

type DisclosureProps = {
  /** Controlled expansion; leave undefined to let the row manage it. */
  expanded?: boolean;
  defaultExpanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
};

export type WorkLogRowProps = Omit<ComponentPropsWithRef<"div">, "children"> &
  DisclosureProps & {
    /** Tool glyph, e.g. a globe for web search; tinted for failures. */
    icon?: ReactNode;
    /** What happened, e.g. "Searched the web" or "Read example.com". */
    label: ReactNode;
    /** Muted text after the label, e.g. the query; truncated to one line. */
    secondary?: ReactNode;
    /** `running` shows a spinner, `failed` a danger dot and `statusText`; `completed` adds nothing. */
    status?: WorkLogStatus;
    /** Status words; defaults to "Running" (spoken only) and "Failed" (shown). */
    statusText?: string;
    /** Non-collapsing content after the header, e.g. a timestamp or a button. */
    trailing?: ReactNode;
    /** Expandable detail; without it the row is a static line. */
    children?: ReactNode;
  };

const DEFAULT_STATUS_TEXT: Record<WorkLogStatus, string | null> = {
  running: "Running",
  completed: null,
  failed: "Failed",
};

function useDisclosure({ expanded, defaultExpanded = false, onExpandedChange }: DisclosureProps) {
  const [internal, setInternal] = useState(defaultExpanded);
  const open = expanded ?? internal;
  const setOpen = (next: boolean) => {
    if (next === open) return;
    if (expanded === undefined) setInternal(next);
    onExpandedChange?.(next);
  };
  return [open, setOpen] as const;
}

/**
 * One tool call in a transcript's work log (web search, page fetch, MCP call, skill load): icon,
 * label, muted detail, and status. With `children` the header is a disclosure button
 * (`aria-expanded`) that reveals the detail below it. The detail mounts the first time the row
 * opens, so detail that loads more (the rest of a truncated tool result) waits until someone asks
 * for it. After that it stays in the page, hidden until found, so find-in-page reaches it and
 * reopens the row.
 */
export function WorkLogRow({
  icon,
  label,
  secondary,
  status = "completed",
  statusText,
  trailing,
  children,
  expanded,
  defaultExpanded,
  onExpandedChange,
  className,
  ...props
}: WorkLogRowProps) {
  const [open, setOpen] = useDisclosure({ expanded, defaultExpanded, onExpandedChange });
  // Set during render, not in an effect, so the first open already has its detail to show.
  const [visited, setVisited] = useState(open);
  if (open && !visited) setVisited(true);
  const hasDetail = children !== undefined && children !== null && children !== false;
  const words = statusText ?? DEFAULT_STATUS_TEXT[status];
  const line = (
    <>
      {icon ? (
        <span className={styles.icon} aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <span className={styles.label}>{label}</span>
      {/* Spoken separators: inline parts would otherwise run together in the accessible name. */}
      {secondary ? (
        <span className={styles.secondary}>
          <span className={a11y.visuallyHidden}>, </span>
          {secondary}
        </span>
      ) : null}
      {status === "running" ? (
        <>
          <LoaderIcon className={styles.spinner} />
          {words ? <span className={a11y.visuallyHidden}>{`, ${words}`}</span> : null}
        </>
      ) : null}
      {status === "failed" ? (
        <span className={styles.failed}>
          <StatusDot tone="danger" size="sm" />
          <span className={a11y.visuallyHidden}>, </span>
          {words}
        </span>
      ) : null}
      {status === "completed" && words ? <span className={a11y.visuallyHidden}>{`, ${words}`}</span> : null}
      {hasDetail ? <ChevronRightIcon className={cx(styles.chevron, open && styles.chevronOpen)} /> : null}
    </>
  );

  const trailingPart = trailing ? <div className={styles.trailing}>{trailing}</div> : null;
  if (!hasDetail) {
    return (
      <div className={cx(styles.row, className)} data-status={status} {...props}>
        <div className={styles.headerRow}>
          <div className={styles.header}>{line}</div>
          {trailingPart}
        </div>
      </div>
    );
  }
  return (
    <Collapsible
      className={cx(styles.row, className)}
      data-status={status}
      {...props}
      open={open}
      onOpenChange={(next) => setOpen(next)}
    >
      <div className={styles.headerRow}>
        <CollapsibleTrigger className={cx(styles.header, styles.toggle)}>{line}</CollapsibleTrigger>
        {trailingPart}
      </div>
      <CollapsiblePanel className={styles.panel}>
        {visited ? <div className={styles.detail}>{children}</div> : null}
      </CollapsiblePanel>
    </Collapsible>
  );
}

export type WorkLogGroupProps = Omit<ComponentPropsWithRef<"div">, "children"> &
  DisclosureProps & {
    icon?: ReactNode;
    /** Summary, e.g. `webSearchSummary(3)` ("Searched the web 3 times"). */
    label: ReactNode;
    /** Aggregate status: `running` while any member runs, `failed` when one failed. */
    status?: WorkLogStatus;
    statusText?: string;
    trailing?: ReactNode;
    /** The member `WorkLogRow`s, shown indented when expanded. */
    children: ReactNode;
  };

/** Collapsible summary row over several `WorkLogRow`s of one kind, e.g. repeated web searches. */
export function WorkLogGroup({ children, className, ...props }: WorkLogGroupProps) {
  return (
    <WorkLogRow className={cx(styles.group, className)} {...props}>
      <div role="list" className={styles.members}>
        {Children.toArray(children).map((child, index) => (
          <div role="listitem" key={isValidElement(child) && child.key !== null ? child.key : index}>
            {child}
          </div>
        ))}
      </div>
    </WorkLogRow>
  );
}

/** "Searched the web" for one search, "Searched the web N times" for more. */
export function webSearchSummary(count: number): string {
  return count > 1 ? `Searched the web ${count} times` : "Searched the web";
}
