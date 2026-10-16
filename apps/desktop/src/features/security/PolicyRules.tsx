import { useState } from "react";
import { Notice, SectionHeader, SettingRow } from "@aidex/ui/components";
import { Select, Stack, Text } from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import type { ExecutionStatus } from "@aidex/client/dto/execution";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import { usePreferences } from "../../app/providers/preferences";
import { shellNeedsFullAccessFrom, useShellNeedsFullAccess } from "../chat/continuation/engineCapabilities";
import { PERMISSION_LABELS, permissionChoices, permissionDescription } from "../chat/continuation/permissions";

type Props = { execution: ExecutionStatus | undefined };

const PERMISSION_ID = "security-default-permission";

/**
 * The policy controls that exist: the default permission for new conversations (saved in the encrypted
 * preferences; existing conversations keep their own) and what the engine enforces on this computer.
 * There is no daemon method to edit filesystem mounts or network allowlists, so none are shown.
 */
export function PolicyRules({ execution }: Props) {
  const { settings, updateSettings, loaded } = usePreferences();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const capabilities = execution?.capabilities;
  // Until the Security page's execution status arrives, use the window's cached answer (no extra read).
  const assumed = useShellNeedsFullAccess(false);
  const context = { shellNeedsFullAccess: capabilities ? shellNeedsFullAccessFrom(capabilities) : assumed };
  const current: PermissionMode = settings.permission_mode ?? "supervised";
  const choices = permissionChoices(context, current);

  const change = async (mode: PermissionMode) => {
    setSaving(true);
    setError(undefined);
    try {
      await updateSettings({ permission_mode: mode });
    } catch (caught) {
      setError(`The default permission was not saved: ${toAidexError(caught).message}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <section>
      <Stack gap={4}>
        <SectionHeader title="Policy" />
        {error ? (
          <Notice tone="danger" role="alert">
            {error}
          </Notice>
        ) : null}
        <SettingRow
          title="Default permission for new conversations"
          description={permissionDescription(current, context)}
          controlId={PERMISSION_ID}
          control={
            <Select
              id={PERMISSION_ID}
              label="Default permission for new conversations"
              labelVisibility="hidden"
              options={choices.map((choice) => ({ value: choice.value, label: choice.title }))}
              value={current}
              disabled={!loaded || saving}
              onChange={(event) => void change(event.target.value as PermissionMode)}
            />
          }
        />
        {current === "full_access" ? (
          <Notice tone="warning">
            {PERMISSION_LABELS.full_access} runs commands as your user account without filesystem or network confinement.
          </Notice>
        ) : null}
        {capabilities ? (
          <Text tone="secondary">
            {capabilities.automatic_reviewer
              ? "An automatic reviewer answers approvals in Auto."
              : `There is no automatic reviewer, so Auto asks like ${PERMISSION_LABELS[capabilities.auto_falls_back_to]}.`}{" "}
            {context.shellNeedsFullAccess
              ? "Commands need Full access on this computer because it cannot confine them to the project."
              : "Commands outside Full access are confined to the project's files."}
          </Text>
        ) : null}
        <Text tone="secondary">
          AiDEX Core checks each tool request against its conversation's project and permission. This build has no setting
          for filesystem mounts or network allowlists.
        </Text>
      </Stack>
    </section>
  );
}
