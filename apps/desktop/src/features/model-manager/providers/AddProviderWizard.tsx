import { useMemo, useState } from "react";
import { Dialog } from "@aidex/ui/components";
import { ArrowLeftIcon } from "@aidex/ui/icons";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import type { ProviderPreset } from "@aidex/client/dto/models";
import type { RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { accountTitles, isOAuthAccount } from "../catalog/providerAccounts";
import { runtimeDriverName, runtimeName } from "../catalog/runtimes";
import { providerSelectionId } from "./selection";
import { AccountConnectStep, type SavedAccount } from "./wizard/AccountConnectStep";
import { PresetStep } from "./wizard/PresetStep";
import { ResultStep, type WizardResultTarget } from "./wizard/ResultStep";
import { RuntimeConnectStep } from "./wizard/RuntimeConnectStep";

type Props = {
  open: boolean;
  onClose: () => void;
  /** Called with the new row's selection id when the user finishes. */
  onDone: (selectionId: string) => void;
  runtimesNotice: string | null;
};

/** "Add provider": choose a preset or runtime, connect it, then see which models it offers. */
export function AddProviderWizard({ open, onClose, onDone, runtimesNotice }: Props) {
  // A fresh wizard each time it opens.
  const [session, setSession] = useState(0);
  const close = () => {
    onClose();
    setSession((value) => value + 1);
  };
  return (
    <Dialog open={open} title="Add provider" size="lg" onClose={close}>
      {open && (
        <WizardSteps
          key={session}
          runtimesNotice={runtimesNotice}
          onDone={(selectionId) => {
            close();
            onDone(selectionId);
          }}
        />
      )}
    </Dialog>
  );
}

type Choice = { kind: "preset"; preset: ProviderPreset } | { kind: "runtime"; driver: RuntimeDriverInfo };

const STEP_LABELS = ["Provider", "Connect", "Models"] as const;

function WizardSteps({
  runtimesNotice,
  onDone,
}: {
  runtimesNotice: string | null;
  onDone: (selectionId: string) => void;
}) {
  const catalog = useProviderCatalog();
  const [step, setStep] = useState<1 | 2 | 3>(1);
  const [choice, setChoice] = useState<Choice | null>(null);
  const [savedAccount, setSavedAccount] = useState<SavedAccount | null>(null);
  const [savedRuntime, setSavedRuntime] = useState<RuntimeConfig | null>(null);
  const [result, setResult] = useState<WizardResultTarget | null>(null);

  const takenAccountIds = useMemo(
    () => new Set(catalog.accounts.map((account) => account.descriptor.id)),
    [catalog.accounts],
  );
  const takenRuntimeIds = useMemo(
    () => new Set(catalog.runtimes.map((runtime) => runtime.id)),
    [catalog.runtimes],
  );
  const chatGptAccounts = useMemo(() => {
    const titles = accountTitles(catalog.accounts, catalog.presets);
    return catalog.accounts
      .filter(isOAuthAccount)
      .map((account) => ({ id: account.descriptor.id, title: titles.get(account.descriptor.id) ?? account.descriptor.id }));
  }, [catalog.accounts, catalog.presets]);

  const choose = (next: Choice) => {
    setChoice(next);
    setSavedAccount(null);
    setSavedRuntime(null);
    setStep(2);
  };

  const finish = () => {
    if (!result) return;
    onDone(
      result.kind === "provider"
        ? providerSelectionId("provider", result.id)
        : providerSelectionId("runtime", result.runtime.id),
    );
  };

  return (
    <Stack gap={4}>
      <Stack direction="row" gap={2} align="center" justify="between">
        <Text as="span" tone="muted" aria-live="polite">
          Step {step} of 3 · {STEP_LABELS[step - 1]}
          {choice && step > 1
            ? ` · ${choice.kind === "preset" ? choice.preset.display_name : runtimeDriverName(choice.driver.driver)}`
            : ""}
        </Text>
        {step === 2 && (
          <Button variant="ghost" icon={<ArrowLeftIcon />} onClick={() => setStep(1)}>
            All providers
          </Button>
        )}
      </Stack>
      {step === 1 && (
        <PresetStep
          presets={catalog.presets}
          drivers={catalog.runtimeDrivers}
          runtimesNotice={runtimesNotice}
          onPreset={(preset) => choose({ kind: "preset", preset })}
          onRuntime={(driver) => choose({ kind: "runtime", driver })}
        />
      )}
      {step === 2 && choice?.kind === "preset" && (
        <AccountConnectStep
          preset={choice.preset}
          takenIds={takenAccountIds}
          existing={savedAccount}
          onSaved={(saved) => {
            setSavedAccount(saved);
            catalog.reload();
            setResult({
              kind: "provider",
              id: saved.id,
              title: choice.preset.display_name,
              providerKind: choice.preset.kind,
            });
            setStep(3);
          }}
        />
      )}
      {step === 2 && choice?.kind === "runtime" && (
        <RuntimeConnectStep
          driver={choice.driver}
          takenIds={takenRuntimeIds}
          chatGptAccounts={chatGptAccounts}
          existing={savedRuntime}
          onSaved={(runtime) => {
            setSavedRuntime(runtime);
            setResult({ kind: "runtime", runtime, title: runtimeName(runtime, catalog.runtimeDrivers) });
            setStep(3);
          }}
        />
      )}
      {step === 3 && result && <ResultStep target={result} onBack={() => setStep(2)} onDone={finish} />}
    </Stack>
  );
}
