import type { CSSProperties } from "react";
import { PaperclipIcon } from "../../icons/actions";
import { AlertTriangleIcon } from "../../icons/interface";
import { cx } from "../../utils/cx";
import styles from "./DropOverlay.module.css";

export type DropOverlayProps = {
  label: string;
  /** Replaces the label and shows the drop as refused. */
  blockedReason?: string;
  /** `inset` fills its positioned parent (the composer); `fixed` is placed by `style` (a page). */
  placement: "inset" | "fixed";
  style?: CSSProperties;
};

/**
 * Dashed highlight with a centered paperclip pill shown while files are dragged over a drop target.
 * It ignores the pointer, so the drag keeps targeting the content under it, and it is hidden from
 * assistive technology: dropping is a pointer-only shortcut for the attach button.
 */
export function DropOverlay({ label, blockedReason, placement, style }: DropOverlayProps) {
  const blocked = blockedReason !== undefined;
  return (
    <div
      className={cx(styles.overlay, styles[placement], blocked && styles.blocked)}
      style={style}
      aria-hidden="true"
      data-aidex-drop-overlay=""
    >
      <span className={styles.pill}>
        {blocked ? <AlertTriangleIcon size={16} /> : <PaperclipIcon size={16} />}
        <span className={styles.text}>{blocked ? blockedReason : label}</span>
      </span>
    </div>
  );
}
