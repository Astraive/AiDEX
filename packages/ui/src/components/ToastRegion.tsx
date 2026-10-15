"use client";

import { Toast as BaseToast } from "@base-ui/react/toast";
import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { CheckCircleIcon, InfoIcon, XCircleIcon, XIcon } from "../icons";
import { IconButton } from "../primitives/IconButton";
import { LAYER_ATTRIBUTE } from "../primitives/layers";
import { cx } from "../utils/cx";
import styles from "./ToastRegion.module.css";

export type ToastTone = "info" | "success" | "danger";

export type ToastMessage = { id: string; message: string; tone?: ToastTone };

export type ToastRegionProps = {
  toasts: readonly ToastMessage[];
  onDismiss: (id: string) => void;
  dismissLabel?: string;
};

export type ToastProviderProps = {
  children?: ReactNode;
  /** Milliseconds a toast stays (default 5000); hovering, focusing or leaving the window pauses it. */
  timeout?: number;
  /** Toasts shown at once (default 3); the oldest gives way. */
  limit?: number;
  dismissLabel?: string;
};

export type ToastManager = {
  /** Shows a toast and returns its id. `danger` toasts are announced assertively. */
  notify: (message: string, tone?: ToastTone) => string;
  dismiss: (id: string) => void;
};

const DEFAULT_TIMEOUT_MS = 5000;
const DEFAULT_LIMIT = 3;
const DEFAULT_DISMISS_LABEL = "Dismiss notification";
const ICON_BY_TONE = { info: InfoIcon, success: CheckCircleIcon, danger: XCircleIcon } as const;

function toneOf(type: string | undefined): ToastTone {
  return type === "success" || type === "danger" ? type : "info";
}

function priorityOf(tone: ToastTone): "low" | "high" {
  return tone === "danger" ? "high" : "low";
}

/** The rendered stack, newest nearest the corner. F6 moves focus to it. */
function ToastList({ dismissLabel }: { dismissLabel: string }) {
  const { toasts } = BaseToast.useToastManager();
  return (
    // `aria-live` keeps the portal out of a modal dialog's aria-hidden pass (it skips live regions),
    // so the hidden announcer Base UI renders beside the viewport still speaks danger toasts raised
    // while a dialog is open. "off" adds no announcing: the viewport and the alerts set their own.
    <BaseToast.Portal aria-live="off">
      {/* A layer of its own: pressing a toast never counts as a press outside an open popover. */}
      <BaseToast.Viewport className={styles.region} {...{ [LAYER_ATTRIBUTE]: "" }}>
        {toasts.map((toast) => {
          const tone = toneOf(toast.type);
          const Icon = ICON_BY_TONE[tone];
          return (
            <BaseToast.Root
              key={toast.id}
              toast={toast}
              swipeDirection={["right", "down"]}
              className={cx(styles.toast, styles[tone])}
            >
              <Icon className={styles.icon} aria-hidden="true" />
              <BaseToast.Title render={<p />} className={styles.message} />
              <BaseToast.Close render={<IconButton size="sm" label={dismissLabel} icon={<XIcon />} />} />
            </BaseToast.Root>
          );
        })}
      </BaseToast.Viewport>
    </BaseToast.Portal>
  );
}

const ToastManagerContext = createContext<ToastManager | null>(null);

/**
 * App-wide notifications on Base UI Toast: each stays `timeout` ms, pausing while hovered, focused
 * or while the window is in the background; `danger` ones are announced assertively; F6 jumps to
 * them; a swipe right or down dismisses one. Toasts sit above dialogs. Read `useToastManager()`.
 */
export function ToastProvider({
  children,
  timeout = DEFAULT_TIMEOUT_MS,
  limit = DEFAULT_LIMIT,
  dismissLabel = DEFAULT_DISMISS_LABEL,
}: ToastProviderProps) {
  const [manager] = useState(() => BaseToast.createToastManager());
  const api = useMemo<ToastManager>(
    () => ({
      notify: (message, tone = "info") => manager.add({ title: message, type: tone, priority: priorityOf(tone) }),
      dismiss: (id) => manager.close(id),
    }),
    [manager],
  );
  return (
    <BaseToast.Provider toastManager={manager} timeout={timeout} limit={limit}>
      <ToastManagerContext.Provider value={api}>
        {children}
        <ToastList dismissLabel={dismissLabel} />
      </ToastManagerContext.Provider>
    </BaseToast.Provider>
  );
}

/** Shows and dismisses toasts; it must be called inside `ToastProvider`. */
export function useToastManager(): ToastManager {
  const api = useContext(ToastManagerContext);
  if (!api) throw new Error("useToastManager must be used inside ToastProvider");
  return api;
}

/**
 * Renders a list of toasts the owner keeps (adding, timing and removing them), e.g. the installer's.
 * It looks and behaves like `ToastProvider`'s toasts, but leaves the timing to the owner: closing
 * one (its button or a swipe) calls `onDismiss`.
 */
export function ToastRegion({ toasts, onDismiss, dismissLabel = DEFAULT_DISMISS_LABEL }: ToastRegionProps) {
  const [manager] = useState(() => BaseToast.createToastManager());
  const shown = useRef(new Set<string>());
  const onDismissRef = useRef(onDismiss);
  useLayoutEffect(() => {
    onDismissRef.current = onDismiss;
  });

  useEffect(() => {
    const ids = new Set(toasts.map((toast) => toast.id));
    for (const id of Array.from(shown.current)) {
      if (ids.has(id)) continue;
      shown.current.delete(id);
      manager.close(id);
    }
    for (const { id, message, tone = "info" } of toasts) {
      if (shown.current.has(id)) continue;
      shown.current.add(id);
      manager.add({
        id,
        title: message,
        type: tone,
        priority: priorityOf(tone),
        timeout: 0,
        onClose: () => {
          // Closed here (button or swipe) rather than by the owner dropping it: tell the owner.
          if (shown.current.delete(id)) onDismissRef.current(id);
        },
      });
    }
  }, [toasts, manager]);

  return (
    <BaseToast.Provider toastManager={manager} timeout={0} limit={Math.max(DEFAULT_LIMIT, toasts.length)}>
      <ToastList dismissLabel={dismissLabel} />
    </BaseToast.Provider>
  );
}
