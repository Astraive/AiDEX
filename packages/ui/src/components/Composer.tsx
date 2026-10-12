"use client";

import {
  useEffect,
  useId,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type Ref,
  type SyntheticEvent,
} from "react";
import { ArrowUpIcon, PaperclipIcon } from "../icons/actions";
import { InfoIcon, UploadIcon } from "../icons/interface";
import { IconButton } from "../primitives/IconButton";
import { cx } from "../utils/cx";
import { DropOverlay } from "./attachments/DropOverlay";
import { useInsideFileDropZone } from "./attachments/FileDropZone";
import { isLargePaste, LARGE_PASTE_BYTES, shouldClaimFilePaste, type FileIntakeSource } from "./attachments/model";
import { useFileDrop } from "./attachments/useFileDrop";
import { commandMenuFieldProps, type CommandMenuFieldState } from "./CommandMenu/model";
import { ComposerAttachMenu, type ComposerAttachAction } from "./ComposerAttachMenu";
import styles from "./Composer.module.css";

export type { ComposerAttachAction } from "./ComposerAttachMenu";

/** Where files entered the composer. */
export type ComposerFileSource = FileIntakeSource;

/** Ctrl/Cmd+Shift+V marks the next paste as "keep as text" for this long. */
const PLAIN_PASTE_WINDOW_MS = 1000;

export type ComposerProps = {
  value: string;
  onValueChange: (value: string) => void;
  /**
   * Send is enabled only when a handler is provided, the composer is not disabled or blocked, and
   * the draft is not blank (or `canSubmitEmpty` is set).
   */
  onSubmit?: (value: string) => void;
  /**
   * Custom attach action, e.g. the app's own menu; it takes precedence over `attachActions` and the
   * built-in picker. `openFilePicker` opens the hidden file input (only when `onFiles` is set).
   */
  onAttach?: (openFilePicker: () => void) => void;
  /**
   * Files picked, pasted, or dropped. Setting it enables the attach button (a hidden
   * `<input type="file" multiple>`), file paste, and dropping files on the composer. Pasted files
   * become files when the clipboard has no plain text, and a pasted image also when the clipboard's
   * HTML is just that image ("Copy image"); rich copies such as Excel cells paste as text
   * (`shouldClaimFilePaste`).
   */
  onFiles?: (files: File[], source: ComposerFileSource) => void;
  /** File picker filter, e.g. "image/png,image/jpeg,application/pdf,text/*". */
  accept?: string;
  /**
   * More attach choices, e.g. "From project…". The attach button then opens a menu listing
   * "Upload from computer" (with `onFiles`) followed by these.
   */
  attachActions?: readonly ComposerAttachAction[];
  /** Menu label of the picker item (default "Upload from computer"). */
  uploadLabel?: string;
  /** Disables attaching (button, paste, drop) and explains why: the attach tooltip and drop overlay. */
  attachDisabledReason?: string;
  /** Allows sending a blank draft, e.g. when attachments are ready. */
  canSubmitEmpty?: boolean;
  /** Blocks sending and explains why: the send button's tooltip and a hint under the composer. */
  submitBlockedReason?: string;
  /** Overlay text while files are dragged over the composer (default "Drop files to attach"). */
  dropLabel?: string;
  /**
   * Plain-text pastes of `largePasteBytes` or more (characters or UTF-8 bytes) go here instead of
   * into the text, e.g. to attach them as a file. Ctrl/Cmd+Shift+V keeps a paste inline.
   */
  onLargePaste?: (text: string) => void;
  /** Threshold for `onLargePaste` (default 32 KiB). */
  largePasteBytes?: number;
  /** Accessible name for the message field. */
  label: string;
  placeholder?: string;
  attachLabel?: string;
  submitLabel?: string;
  disabled?: boolean;
  autoFocus?: boolean;
  textareaRef?: Ref<HTMLTextAreaElement>;
  /** The composer surface; pass it as a `CommandMenu` `anchorRef` so the menu sits on it and spans its width. */
  ref?: Ref<HTMLFormElement>;
  /**
   * Runs before the composer's own key handling; call `event.preventDefault()` to stop Enter from
   * sending. An open `CommandMenu` handles its keys before this runs.
   */
  onKeyDown?: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  /** Caret position (selection end) after typing, clicking, or moving it, e.g. to detect a `$` or `/` trigger. */
  onSelectionChange?: (caret: number) => void;
  /** Ties the field to a `CommandMenu` (`aria-controls`, `aria-activedescendant`) while focus stays in it. */
  suggestions?: CommandMenuFieldState;
  /** A popup for the field, typically a `CommandMenu`; it portals, so placing it here only groups the code. */
  menu?: ReactNode;
  /** Content above the text, e.g. an `AttachmentTray`. */
  attachments?: ReactNode;
  /** Controls after the attach button, e.g. a Chat/Agent switch. */
  toolbarStart?: ReactNode;
  /** Controls before the send button, e.g. a model picker. */
  toolbarEnd?: ReactNode;
  /** `compact` is a single-line composer docked under a page. */
  size?: "default" | "compact";
  className?: string;
};

/**
 * Multi-line prompt input with attach and send actions. Enter sends; Shift+Enter adds a line. With
 * `onFiles`, files can also be picked, pasted, or dropped on it (inside a `FileDropZone`, the zone
 * takes the drops instead, unless the composer is inside a `Dialog`).
 */
export function Composer({
  value,
  onValueChange,
  onSubmit,
  onAttach,
  onFiles,
  accept,
  attachActions,
  uploadLabel = "Upload from computer",
  attachDisabledReason,
  canSubmitEmpty = false,
  submitBlockedReason,
  dropLabel = "Drop files to attach",
  onLargePaste,
  largePasteBytes = LARGE_PASTE_BYTES,
  label,
  placeholder,
  attachLabel = "Attach files",
  submitLabel = "Send",
  disabled = false,
  autoFocus = false,
  textareaRef,
  ref,
  onKeyDown,
  onSelectionChange,
  suggestions,
  menu,
  attachments,
  toolbarStart,
  toolbarEnd,
  size = "default",
  className,
}: ComposerProps) {
  const hintId = useId();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const attachButtonRef = useRef<HTMLButtonElement>(null);
  const plainPasteUntil = useRef(0);
  const [attachMenuOpen, setAttachMenuOpen] = useState(false);
  const insideDropZone = useInsideFileDropZone();

  const attachBlocked = attachDisabledReason !== undefined && attachDisabledReason !== "";
  const filesEnabled = onFiles !== undefined && !disabled && !attachBlocked;
  const blocked = submitBlockedReason !== undefined && submitBlockedReason !== "";
  const hasDraft = value.trim().length > 0 || canSubmitEmpty;
  const canSubmit = onSubmit !== undefined && !disabled && !blocked && hasDraft;
  // A send blocked only by `submitBlockedReason` stays focusable, so its reason can be read.
  const sendFocusableWhileBlocked = blocked && onSubmit !== undefined && !disabled;

  const openFilePicker = () => {
    if (filesEnabled) fileInputRef.current?.click();
  };
  const menuActions: ComposerAttachAction[] = [
    ...(onFiles !== undefined
      ? [{ id: "upload", label: uploadLabel, icon: <UploadIcon />, disabled: !filesEnabled, onSelect: openFilePicker }]
      : []),
    ...(attachActions ?? []),
  ];
  const hasAttachMenu = !onAttach && (attachActions?.length ?? 0) > 0;
  const attachAvailable = !disabled && !attachBlocked && (onAttach !== undefined || hasAttachMenu || filesEnabled);

  // A menu closed because attaching became unavailable must not reopen by itself later.
  useEffect(() => {
    if (!attachAvailable) setAttachMenuOpen(false);
  }, [attachAvailable]);

  const drop = useFileDrop<HTMLFormElement>({
    enabled: filesEnabled && !insideDropZone,
    blockedReason: onFiles !== undefined && attachBlocked && !insideDropZone && !disabled ? attachDisabledReason : undefined,
    onFiles: (files) => onFiles?.(files, "drop"),
  });

  const submit = () => {
    if (canSubmit) onSubmit(value);
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    submit();
  };

  // Enter keeps its newline behavior unless this composer can actually send.
  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "v") {
      plainPasteUntil.current = Date.now() + PLAIN_PASTE_WINDOW_MS;
    }
    onKeyDown?.(event);
    if (event.defaultPrevented || !onSubmit || disabled) return;
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    submit();
  };

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const keepAsText = Date.now() <= plainPasteUntil.current;
    plainPasteUntil.current = 0;
    const data = event.clipboardData;
    const text = data.getData("text/plain");
    const files = Array.from(data.files);
    // HTML is read only when files are offered: it tells a copied image from a rich copy with a bitmap.
    if (filesEnabled && !keepAsText && files.length > 0 && shouldClaimFilePaste(files, text, data.getData("text/html"))) {
      event.preventDefault();
      onFiles?.(files, "paste");
      return;
    }
    if (onLargePaste && !keepAsText && isLargePaste(text, largePasteBytes)) {
      event.preventDefault();
      onLargePaste(text);
    }
  };

  const handleFileInput = (event: ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(event.currentTarget.files ?? []);
    // Reset, so picking the same file again still reports a change.
    event.currentTarget.value = "";
    // Files picked before the composer became busy still arrive; only a disabled-attach reason drops them.
    if (files.length > 0 && !attachBlocked) onFiles?.(files, "picker");
  };

  const handleAttach = () => {
    if (!attachAvailable) return;
    if (onAttach) onAttach(openFilePicker);
    else if (hasAttachMenu) setAttachMenuOpen((open) => !open);
    else openFilePicker();
  };

  const reportCaret = (event: SyntheticEvent<HTMLTextAreaElement>) => {
    onSelectionChange?.(event.currentTarget.selectionEnd);
  };

  // Clicks on the padded surface around the field should still place the caret.
  const focusField = (event: MouseEvent<HTMLElement>) => {
    if (event.target !== event.currentTarget) return;
    event.preventDefault();
    event.currentTarget.closest("form")?.querySelector("textarea")?.focus();
  };

  const buttonSize = size === "compact" ? "md" : "lg";

  return (
    <form
      ref={ref}
      className={cx(styles.composer, size === "compact" && styles.compact, disabled && styles.disabled, className)}
      onSubmit={handleSubmit}
      onMouseDown={focusField}
      {...drop.handlers}
    >
      {attachments ? <div className={styles.attachments}>{attachments}</div> : null}
      <textarea
        ref={textareaRef}
        className={styles.field}
        value={value}
        placeholder={placeholder}
        aria-label={label}
        rows={size === "compact" ? 1 : 2}
        disabled={disabled}
        autoFocus={autoFocus}
        onChange={(event) => {
          onValueChange(event.target.value);
          reportCaret(event);
        }}
        onSelect={reportCaret}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
        {...(suggestions ? commandMenuFieldProps(suggestions) : null)}
      />
      <div className={styles.toolbar} onMouseDown={focusField}>
        <div className={styles.group}>
          <IconButton
            ref={attachButtonRef}
            label={attachLabel}
            {...(attachBlocked && !disabled ? { title: attachDisabledReason } : null)}
            icon={<PaperclipIcon />}
            variant="soft"
            shape="circle"
            size={buttonSize}
            disabled={!attachAvailable && !(attachBlocked && !disabled)}
            aria-disabled={attachBlocked && !disabled ? true : undefined}
            aria-haspopup={hasAttachMenu ? "menu" : undefined}
            aria-expanded={hasAttachMenu ? attachMenuOpen : undefined}
            onClick={handleAttach}
          />
          {toolbarStart}
        </div>
        <div className={styles.group}>
          {toolbarEnd}
          <IconButton
            type="submit"
            label={submitLabel}
            {...(sendFocusableWhileBlocked ? { title: submitBlockedReason } : null)}
            icon={<ArrowUpIcon />}
            variant={canSubmit ? "primary" : "solid"}
            shape="circle"
            size={buttonSize}
            disabled={!canSubmit && !sendFocusableWhileBlocked}
            aria-disabled={sendFocusableWhileBlocked ? true : undefined}
            aria-describedby={blocked ? hintId : undefined}
          />
        </div>
      </div>
      {blocked ? (
        <p id={hintId} className={styles.hint}>
          <InfoIcon size={14} />
          <span>{submitBlockedReason}</span>
        </p>
      ) : null}
      {onFiles !== undefined ? (
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept={accept}
          hidden
          tabIndex={-1}
          aria-hidden="true"
          onChange={handleFileInput}
        />
      ) : null}
      {hasAttachMenu ? (
        <ComposerAttachMenu
          open={attachMenuOpen && attachAvailable}
          onOpenChange={setAttachMenuOpen}
          anchorRef={attachButtonRef}
          actions={menuActions}
          label={attachLabel}
        />
      ) : null}
      {drop.active ? (
        <DropOverlay placement="inset" label={dropLabel} blockedReason={drop.blocked ? attachDisabledReason : undefined} />
      ) : null}
      {menu}
    </form>
  );
}
