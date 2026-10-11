import { useState } from "react";
import { toAidexError } from "@aidex/client/errors";
export function useExtensionAction(refetch: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();
  const act = async (operation: () => Promise<unknown>, success?: string) => {
    setBusy(true);
    setError(undefined);
    setMessage(undefined);
    try {
      await operation();
      refetch();
      if (success) setMessage(success);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, message, act };
}
