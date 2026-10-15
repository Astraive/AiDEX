"use client";

import { useId, useRef, useState, type ReactNode } from "react";
import { IconButton } from "../primitives/IconButton";
import { Popover } from "../primitives/Popover";
import { StatusDot, type StatusDotTone } from "../primitives/StatusDot";
import styles from "./TopBar.module.css";

export type TopBarProps = {
  /** Leading controls and the page title; text here ellipsizes before the end slot gives way. */
  start?: ReactNode;
  /** Centered slot, typically a `SearchField`; capped to a comfortable width. */
  center?: ReactNode;
  end?: ReactNode;
};

/** Three-slot bar for the top of the main region. */
export function TopBar({ start, center, end }: TopBarProps) {
  return (
    <header className={styles.topBar}>
      <div className={styles.start}>{start}</div>
      <div className={styles.center}>{center}</div>
      <div className={styles.end}>{end}</div>
    </header>
  );
}

export type TopBarStatusProps = {
  /** The state in a few words, e.g. "Drive connected": the button's name and the panel's first line. */
  label: string;
  tone: StatusDotTone;
  /** Names the details panel, e.g. "Drive connection". */
  title: string;
  /** Details under the state, e.g. the drive path. */
  children?: ReactNode;
};

/**
 * A status dot for the top bar. Pressing it opens a small non-modal panel with the details; Escape,
 * a press outside or focus leaving closes it.
 */
export function TopBarStatus({ label, tone, title, children }: TopBarStatusProps) {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  return (
    <>
      <IconButton
        ref={anchorRef}
        size="sm"
        label={label}
        icon={<StatusDot tone={tone} />}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen((current) => !current)}
      />
      <Popover id={panelId} open={open} onOpenChange={setOpen} anchorRef={anchorRef} placement="bottom-end" label={title}>
        <div className={styles.statusPanel}>
          <p className={styles.statusLabel}>{label}</p>
          {children ? <div className={styles.statusDetail}>{children}</div> : null}
        </div>
      </Popover>
    </>
  );
}
