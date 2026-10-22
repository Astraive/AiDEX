import { useCallback, useEffect, useState } from "react";
import type { DriveInspection } from "@aidex/client/dto/installer";
import { toAidexError, type AidexError } from "@aidex/client/errors";
import { inspectNormalTarget } from "@aidex/client/installer";

export type NormalTargetQuery = {
  inspection: DriveInspection | null;
  error: AidexError | null;
  loading: boolean;
  refresh: () => void;
};

/** Read-only inspection of the current user's standard AiDEX installation target. */
export function useNormalTarget(): NormalTargetQuery {
  const [inspection, setInspection] = useState<DriveInspection | null>(null);
  const [error, setError] = useState<AidexError | null>(null);
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    let current = true;
    setLoading(true);
    inspectNormalTarget().then(
      (result) => {
        if (!current) return;
        setInspection(result);
        setError(null);
        setLoading(false);
      },
      (caught: unknown) => {
        if (!current) return;
        setError(toAidexError(caught));
        setLoading(false);
      },
    );
    return () => {
      current = false;
    };
  }, [revision]);

  const refresh = useCallback(() => setRevision((value) => value + 1), []);
  return { inspection, error, loading, refresh };
}
