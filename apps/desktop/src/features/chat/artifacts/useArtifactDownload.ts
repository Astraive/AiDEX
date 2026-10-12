import { useCallback } from "react";
import { toAidexError } from "@aidex/client/errors";
import { useToast } from "../../../app/providers/toast";
import { downloadArtifact } from "./artifactContent";

/** Saves an artifact as a file; failures (a locked vault, a deleted artifact) show as a toast. */
export function useArtifactDownload(): (id: string) => void {
  const notify = useToast();
  return useCallback(
    (id: string) => {
      downloadArtifact(id).catch((caught) => notify(toAidexError(caught).message, "danger"));
    },
    [notify],
  );
}
