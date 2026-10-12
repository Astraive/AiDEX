import { useCallback } from "react";
import { safeExternalUrl } from "@aidex/ui/components";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useToast } from "../../../app/providers/toast";

/**
 * Opens a link from model output or a web result in the system browser. Only absolute http(s) URLs
 * without credentials are passed on, and `web.open` validates them again (it opens https only).
 */
export function useOpenLink(): (url: string) => void {
  const notify = useToast();
  return useCallback(
    (raw: string) => {
      const url = safeExternalUrl(raw, { allowMailto: false });
      if (!url) {
        notify("Only http and https links can be opened.", "danger");
        return;
      }
      call("web.open", { url }).catch((caught) => notify(toAidexError(caught).message, "danger"));
    },
    [notify],
  );
}
