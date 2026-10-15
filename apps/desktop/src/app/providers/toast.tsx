import type { ReactNode } from "react";
import { ToastProvider as UiToastProvider, useToastManager, type ToastMessage } from "@aidex/ui/components";

type Notify = (message: string, tone?: ToastMessage["tone"]) => void;

/**
 * App-wide notifications; preview actions use it to say what would happen once Core is connected.
 * Toasts stay five seconds, pause while hovered, focused or in the background, and F6 reaches them.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  return <UiToastProvider>{children}</UiToastProvider>;
}

export function useToast(): Notify {
  return useToastManager().notify;
}
