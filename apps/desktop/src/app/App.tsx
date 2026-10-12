"use client";

import { useEffect, useState } from "react";
import { useFileDropGuard } from "@aidex/ui/components";
import { SelectDrivePage } from "../features/launch/SelectDrivePage";
import { UnlockPage } from "../features/launch/UnlockPage";
import { useAidexStatus } from "../hooks/useAidexStatus";
import { AidexProvider } from "../services/AidexProvider";
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
    return <UnlockPage onContinue={() => setStage("workspace")} />;
  return <Workspace />;
}
