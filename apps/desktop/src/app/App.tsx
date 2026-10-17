"use client";

import { useEffect, useState } from "react";
import { setUiScale } from "@aidex/client/desktop";
import { call } from "@aidex/client/rpc";
import { useFileDropGuard } from "@aidex/ui/components";
import { SelectDrivePage } from "../features/launch/SelectDrivePage";
import { UnlockPage } from "../features/launch/UnlockPage";
import { useAidexStatus } from "../hooks/useAidexStatus";
import { AidexProvider } from "../services/AidexProvider";
import { normalizeAppearance } from "./providers/appearanceState";
import { Workspace } from "./Workspace";

type Stage = "drive" | "unlock" | "workspace";

/** Launch flow: find the drive, start Core, unlock the vault, then enter chat. */
export function App() {
  // HTML5 drops reach the page (`dragDropEnabled: false`), so a file dropped anywhere outside a drop
  // target must never navigate the window to it.
  useFileDropGuard();
  return (
    <AidexProvider>
      <Launch />
    </AidexProvider>
  );
}

function Launch() {
  const [stage, setStage] = useState<Stage>("drive");
  const { status } = useAidexStatus();
  const driveResolved = status?.drive.state === "resolved";

  // A drive found automatically (or just chosen) skips the selection screen.
  useEffect(() => {
    if (stage === "drive" && driveResolved) setStage("unlock");
  }, [stage, driveResolved]);

  if (stage === "drive")
    return <SelectDrivePage onSkip={() => setStage("unlock")} />;
  if (stage === "unlock")
    return (
      <UnlockPage
        onContinue={() => void applySavedScale().then(() => setStage("workspace"))}
      />
    );
  return <Workspace />;
}

/** How long entering the workspace waits for the saved interface scale. */
const SCALE_WAIT_MS = 400;

/**
 * Applies the saved interface scale before the workspace mounts, so the window does not jump once
 * the appearance loads. Best effort: it gives up after `SCALE_WAIT_MS` and never fails.
 */
async function applySavedScale(): Promise<void> {
  const apply = call("appearance.get")
    .then((saved) => setUiScale(normalizeAppearance(saved).ui_scale))
    .catch(() => undefined);
  await Promise.race([apply, new Promise((resolve) => setTimeout(resolve, SCALE_WAIT_MS))]);
}
