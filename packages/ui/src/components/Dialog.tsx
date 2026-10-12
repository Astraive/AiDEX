"use client";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { XIcon } from "../icons/interface";
import { IconButton } from "../primitives/IconButton";
import { cx } from "../utils/cx";
import { FileDropZoneContext } from "./attachments/FileDropZone";
import { containFileDrag } from "./attachments/fileDropGuard";
import styles from "./Dialog.module.css";

export type DialogProps = {
  open: boolean;
  title: string;
  onClose: () => void;
  /** Supporting text under the title, linked as the dialog's description. */
  description?: ReactNode;
  /** Width: `md` 480px (default), `lg` 720px, `xl` 960px; always clamped to the window. */
  size?: "md" | "lg" | "xl";
  children: ReactNode;
};

/**
 * Native modal: traps focus, supports Escape, and restores focus to its trigger. Files dragged over
 * it never reach a drop target behind it.
 */
export function Dialog({ open, title, onClose, description, size = "md", children }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const openRef = useRef(open);
  // Bumped when the browser closes the modal by itself, so it opens again while `open` stays true.
  const [reopen, setReopen] = useState(0);
  const titleId = useId();
  const descriptionId = useId();
  useEffect(() => {
    openRef.current = open;
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open, reopen]);
  return (
    <dialog
      ref={ref}
      className={cx(styles.dialog, styles[size])}
      aria-labelledby={titleId}
      aria-describedby={description ? descriptionId : undefined}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClose={(event) => {
        // The browser can close a modal itself (a repeated Escape skips `cancel` or cannot cancel
        // it). Tell the owner, and show the dialog again if the owner keeps it open, e.g. while busy.
        // A close the owner asked for, or one already followed by a reopen, needs nothing.
        if (!openRef.current || event.currentTarget.open) return;
        onClose();
        setReopen((count) => count + 1);
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      onDragEnter={containFileDrag}
      onDragOver={containFileDrag}
      onDrop={containFileDrag}
    >
      {/* No page-level drop zone receives drops from inside a modal, so composers here take their own. */}
      <FileDropZoneContext.Provider value={false}>
        <div className={styles.header}>
          <div className={styles.heading}>
            <h2 id={titleId}>{title}</h2>
            {description ? (
              <div id={descriptionId} className={styles.description}>
                {description}
              </div>
            ) : null}
          </div>
          <IconButton
            label="Close dialog"
            size="sm"
            icon={<XIcon />}
            onClick={onClose}
          />
        </div>
        <div className={styles.body}>{children}</div>
      </FileDropZoneContext.Provider>
    </dialog>
  );
}
