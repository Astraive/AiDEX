import { useEffect, useReducer, useRef, useState } from "react";
import { Dialog, KeyValueList, Notice, SectionHeader, SettingRow } from "@aidex/ui/components";
import { RefreshIcon, ShieldCheckIcon } from "@aidex/ui/icons";
import { Button, Card, Stack, StatusIndicator, Switch, Text, TextField } from "@aidex/ui/primitives";
import { callUpdate } from "@aidex/client/desktop";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type {
  ApplyReport,
  CheckReport,
  JournalOperation,
  RollbackReport,
  SignatureState,
  UpdateStatus,
} from "@aidex/client/dto/updates";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { usePoll } from "../../hooks/usePoll";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { mayStillRun } from "./updateCalls";
import {
  absoluteUpdateSource,
  applySelectionReason,
  countUnchangedReads,
  followedOutcome,
  journalProgressKey,
  progressPollInterval,
  reviewReducer,
  sameCandidate,
} from "./updateSelection";

const DOWNGRADE_ID = "setting-update-downgrade";

/** An apply or rollback whose call stopped waiting while AiDEX Core may still be working on it. */
class StoppedWaiting extends Error {
  constructor(
    readonly operation: JournalOperation,
    readonly failure: unknown,
  ) {
    super(toAidexError(failure).message);
  }
}

/** Sends an apply or rollback; a lost or timed-out call becomes `StoppedWaiting` (it may still finish). */
async function runOperation<T>(operation: JournalOperation, send: () => Promise<T>): Promise<T> {
  try {
    return await send();
  } catch (caught) {
    if (mayStillRun(toAidexError(caught))) throw new StoppedWaiting(operation, caught);
    throw caught;
  }
}

const signatureLabel = (signature: SignatureState | null): string =>
  !signature
    ? "Not verified"
    : signature.status === "verified"
      ? `${signature.key_name} (${signature.key_id}, ${signature.key_source} key)`
      : "Unsigned";

function integrityRows(status: UpdateStatus) {
  const { integrity } = status;
  return [
    {
      label: "Result",
      value: integrity.verified
        ? `Verified (${integrity.scope === "all" ? "every file" : "critical files"})`
        : `Not verified${integrity.reason ? `: ${integrity.reason.replace(/_/g, " ")}` : ""}`,
    },
    { label: "Signer", value: signatureLabel(integrity.signature) },
    { label: "Files checked", value: integrity.verified_files ?? "Unknown" },
    {
      label: "SBOM",
      value: integrity.sbom.present ? (integrity.sbom.valid ? "Present and valid" : "Present, invalid") : "Not included",
    },
  ];
}

/**
 * Signed updates of the AiDEX bundle on this drive: the selected and previous versions, the installed
 * bundle's integrity, checking and applying a manually supplied signed package, and rolling back to the
 * recorded previous version. Every source must be an absolute local path, unsigned packages are never
 * accepted, and apply and rollback re-check what was reviewed before they change anything. A switch
 * takes effect on the next launch. Check, apply, and rollback run on a bridge connection of their own
 * (`callUpdate`); when one stops waiting while AiDEX Core is still working, the page follows the update
 * journal until it ends instead of reporting a failure.
 */
export function UpdatesPanel() {
  const status = useRpc("update.status");
  const { driveRoot, connectionKey } = useAidexStatus();
  // Results that arrive after the drive or daemon changed describe another installation; drop them.
  const connection = useRef(connectionKey);
  connection.current = connectionKey;
  const [source, setSource] = useState("");
  const [allowDowngrade, setAllowDowngrade] = useState(false);
  const [checked, setChecked] = useState<CheckReport>();
  const [result, setResult] = useState<ApplyReport | RollbackReport>();
  const [{ review, error }, dispatch] = useReducer(reviewReducer, {});
  const setError = (message: string | undefined) => dispatch({ type: "error", error: message });
  const closeReview = () => dispatch({ type: "close" });
  const [busy, setBusy] = useState<string>();
  // An apply or rollback the page stopped waiting for, followed through the journal until it ends.
  // `afterRead`: only status reads that arrive after it describe the followed operation.
  const [following, setFollowing] = useState<{ operation: JournalOperation; since: number; afterRead: number }>();
  const [outcome, setOutcome] = useState<ReturnType<typeof followedOutcome>>();

  useEffect(() => {
    setChecked(undefined);
    setResult(undefined);
    dispatch({ type: "reset" });
    setFollowing(undefined);
    setOutcome(undefined);
  }, [connectionKey]);

  const pending = status.data?.pending ?? false;
  // Every status read hashes the bundle's critical files: poll only while the journal can move, more
  // slowly once it stopped moving, and only while the window is visible.
  const [unchangedReads, setUnchangedReads] = useState<{ key: string; count: number }>();
  const statusData = status.data;
  const statusReads = useRef(0);
  useEffect(() => {
    if (!statusData) return;
    statusReads.current += 1;
    setUnchangedReads((previous) => countUnchangedReads(previous, journalProgressKey(statusData.journal)));
  }, [statusData]);
  const pollMs = progressPollInterval({
    busy: busy !== undefined || following !== undefined,
    pending,
    recoveryError: !!statusData?.recovery_error,
    unchangedReads: unchangedReads?.count ?? 0,
  });
  usePoll(status.refetch, pollMs ?? 0, { enabled: pollMs !== undefined, whileAway: true });

  // The followed operation ended: report what the journal says.
  useEffect(() => {
    if (!following || !statusData || statusData.pending || statusReads.current <= following.afterRead) return;
    setOutcome(followedOutcome(following.operation, following.since, statusData.journal));
    setFollowing(undefined);
  }, [following, statusData]);

  /** Runs one operation; `perform` gets a guard that is false once the connection changed. */
  const act = async (label: string, perform: (current: () => boolean) => Promise<void>) => {
    const key = connectionKey;
    const current = () => connection.current === key;
    // A little before the call, so a journal the daemon starts right away still counts as this one.
    const since = Date.now() - 5000;
    setBusy(label);
    setError(undefined);
    setOutcome(undefined);
    try {
      await perform(current);
    } catch (caught) {
      if (!current()) return;
      if (caught instanceof StoppedWaiting) {
        // AiDEX Core keeps working when its caller goes away: follow the journal, unless it shows
        // nothing running (then the call failed, and its error says why).
        const fresh = await call("update.status").catch(() => undefined);
        if (!current()) return;
        if (fresh?.pending) {
          closeReview();
          setFollowing({ operation: caught.operation, since, afterRead: statusReads.current });
          return;
        }
      }
      const failure = toAidexError(caught instanceof StoppedWaiting ? caught.failure : caught);
      const reason = (failure.data as { reason?: string } | undefined)?.reason;
      setError(reason ? `${failure.message} (${reason.replace(/_/g, " ")})` : failure.message);
    } finally {
      if (current()) status.refetch();
      setBusy(undefined);
    }
  };

  const check = () =>
    void act("Checking the package signature and contents…", async (current) => {
      const path = source.trim();
      if (!status.data || !absoluteUpdateSource(path, status.data.target)) {
        throw new Error(
          "Enter the absolute path of a package or release folder on this computer. URLs, relative paths, and network shares are not supported; copy a shared package to this computer first.",
        );
      }
      const report = await callUpdate("update.check", {
        source: path,
        allow_downgrade: allowDowngrade,
        allow_unsigned: false,
      });
      if (!current()) return;
      setChecked(report);
      setResult(undefined);
    });

  const apply = () =>
    void act("Re-checking, staging, and self-testing the package…", async (current) => {
      const refusal = applySelectionReason(checked, status.data?.target, pending);
      if (refusal || !checked) throw new Error(refusal ?? "Check a signed package first.");
      const params = { source: checked.package, allow_downgrade: allowDowngrade, allow_unsigned: false };
      const refreshed = await callUpdate("update.check", params);
      if (!current()) return;
      if (!sameCandidate(checked, refreshed)) {
        setChecked(refreshed);
        closeReview();
        throw new Error("The package changed since you reviewed it. Review the new check result before applying it.");
      }
      const report = await runOperation("apply", () => callUpdate("update.apply", params));
      if (!current()) return;
      setResult(report);
      setChecked(undefined);
      closeReview();
    });

  const rollback = () =>
    void act("Re-verifying the previous version…", async (current) => {
      const reviewed = status.data?.previous;
      if (!reviewed) throw new Error("No previous version is recorded.");
      const fresh = await call("update.status");
      if (!current()) return;
      const previous = fresh.previous;
      if (
        fresh.pending ||
        !previous ||
        previous.version !== reviewed.version ||
        previous.dir !== reviewed.dir ||
        previous.manifest_sha256 !== reviewed.manifest_sha256
      ) {
        closeReview();
        throw new Error("The previous version or the update state changed. Review the rollback again.");
      }
      const report = await runOperation("rollback", () => callUpdate("update.rollback", { allow_unsigned: false }));
      if (!current()) return;
      setResult(report);
      setChecked(undefined);
      closeReview();
    });

  const data = status.data;
  const applyRefusal = applySelectionReason(checked, data?.target, pending);
  const idle = busy === undefined;

  return (
    <Stack gap={10}>
      <CoreStatusNotice requires="daemon" />
      {error && review === undefined ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}
      {status.error ? (
        <Notice tone="danger">
          <Stack gap={3} align="start">
            <Text as="span">Cannot read the update status: {status.error.message}</Text>
            <Button variant="outline" size="md" icon={<RefreshIcon />} disabled={!idle} onClick={status.refetch}>
              Retry
            </Button>
          </Stack>
        </Notice>
      ) : null}
      {busy && review === undefined ? <Notice role="status">{busy}</Notice> : null}
      {following ? (
        <Notice role="status">
          This page stopped waiting, but AiDEX Core is still{" "}
          {following.operation === "apply" ? "applying the package" : "rolling back"}. The update status is re-read
          until it finishes.
        </Notice>
      ) : null}
      {outcome ? (
        <Notice tone={outcome.tone} role={outcome.tone === "danger" ? "alert" : "status"}>
          {outcome.text}
        </Notice>
      ) : null}

      <section>
        <SectionHeader title="Version" />
        {data ? (
          <Stack gap={4}>
            <KeyValueList
              items={[
                { label: "Running", value: data.daemon_version },
                { label: "Selected for next launch", value: data.current?.version ?? "None installed" },
                { label: "Previous", value: data.previous?.version ?? "None recorded" },
                { label: "This computer", value: data.target },
                { label: "Drive", value: driveRoot ?? "Unknown" },
              ]}
            />
            {data.recovery_error ? (
              <Notice tone="danger">Startup recovery failed: {data.recovery_error}</Notice>
            ) : data.recovery && data.recovery.outcome !== "clean" ? (
              <Notice tone="warning">
                Startup recovery: {data.recovery.outcome.replace(/_/g, " ")}. {data.recovery.warnings.join(" ")}
              </Notice>
            ) : null}
            {data.pending ? (
              busy !== undefined || following ? (
                <Text tone="secondary">Current step: {data.journal?.state.replace(/_/g, " ") ?? "starting"}.</Text>
              ) : (
                <Notice tone="warning">
                  An update is unfinished ({data.journal?.state.replace(/_/g, " ") ?? "unknown step"}).{" "}
                  {data.recovery_error
                    ? "AiDEX Core must recover it when it starts again"
                    : "It may still be running, for example from the command line; otherwise AiDEX Core must recover it"}{" "}
                  before another package is applied.
                </Notice>
              )
            ) : null}
            {data.restart_required || result?.restart_required ? (
              <Notice tone="warning">
                The selected version starts the next time AiDEX launches. Restart AiDEX when you are ready; nothing replaces
                the running version.
              </Notice>
            ) : null}
          </Stack>
        ) : status.loading ? (
          <Text tone="secondary">Reading the update status…</Text>
        ) : null}
      </section>

      {data ? (
        <section>
          <SectionHeader
            title="Integrity"
            meta={
              <StatusIndicator
                tone={data.integrity.verified ? "success" : "warning"}
                label={data.integrity.verified ? "Verified" : "Not verified"}
              />
            }
            actions={
              <Button
                variant="outline"
                size="md"
                icon={<ShieldCheckIcon />}
                disabled={!idle || status.loading}
                focusableWhenDisabled
                onClick={status.refetch}
              >
                {status.loading ? "Verifying…" : "Verify again"}
              </Button>
            }
          />
          <Stack gap={4}>
            <KeyValueList items={integrityRows(data)} />
            {data.integrity.error ? <Notice tone="warning">{data.integrity.error}</Notice> : null}
            <Text tone="muted">
              The selected bundle is checked the way the launcher checks it: its publisher signature and the hashes of its
              critical files.
            </Text>
          </Stack>
        </section>
      ) : null}

      <section>
        <SectionHeader title="Install a signed package" />
        <Card padding="md">
          <Stack gap={5}>
            <Text>
              Choose a package (.zip or .tar.gz), an unpacked bundle, or a release folder on this computer. A folder resolves
              to its newest package for this computer. This build has no online release catalog.
            </Text>
            <TextField
              label="Package or folder path"
              placeholder={data?.target.startsWith("windows-") ? "D:\\releases\\aidex.zip" : "/media/releases/aidex.tar.gz"}
              autoComplete="off"
              spellCheck={false}
              value={source}
              disabled={!idle || pending}
              hint="An absolute path. Packages must carry a trusted publisher signature; a matching checksum alone is not enough."
              onChange={(event) => {
                setSource(event.target.value);
                setChecked(undefined);
              }}
            />
            <SettingRow
              title="Allow a downgrade"
              description="Only to the exact previous version recorded on this drive."
              controlId={DOWNGRADE_ID}
              control={
                <Switch
                  id={DOWNGRADE_ID}
                  checked={allowDowngrade}
                  disabled={!idle || pending}
                  onCheckedChange={(enabled) => {
                    setAllowDowngrade(enabled);
                    setChecked(undefined);
                  }}
                />
              }
            />
            <Stack direction="row" justify="end">
              <Button
                variant="outline"
                size="md"
                disabled={!idle || !source.trim() || !data || pending}
                focusableWhenDisabled={!idle}
                onClick={check}
              >
                Check package
              </Button>
            </Stack>
            {checked ? (
              <Stack gap={4}>
                <KeyValueList
                  items={[
                    { label: "Package", value: checked.package },
                    { label: "Version", value: `${checked.candidate.version} (${checked.candidate.target})` },
                    { label: "Action", value: checked.action.replace(/_/g, " ") },
                    { label: "Signature", value: signatureLabel(checked.candidate.signature) },
                    { label: "Checksum", value: checked.checksum.replace(/_/g, " ") },
                    { label: "Manifest digest", value: checked.candidate.manifest_sha256 },
                    { label: "SBOM", value: checked.candidate.sbom ? "Included" : "Not included" },
                  ]}
                />
                <Text tone="muted">
                  Checking changed nothing. Applying checks the package again, stages it, and runs its self-tests before it is
                  selected for the next launch.
                </Text>
                {applyRefusal ? <Notice>{applyRefusal}</Notice> : null}
                <Stack direction="row" justify="end">
                  <Button
                    variant="primary"
                    size="md"
                    disabled={!idle || !!applyRefusal}
                    onClick={() => dispatch({ type: "open", review: "apply" })}
                  >
                    Apply…
                  </Button>
                </Stack>
              </Stack>
            ) : null}
          </Stack>
        </Card>
      </section>

      <section>
        <SectionHeader
          title="Roll back"
          actions={
            <Button
              variant="outline"
              size="md"
              disabled={!idle || !data?.previous || pending}
              onClick={() => dispatch({ type: "open", review: "rollback" })}
            >
              Roll back…
            </Button>
          }
        />
        <Text tone="secondary">
          {data?.previous
            ? `Switches back to ${data.previous.version}, the previous version recorded on this drive, after verifying its signature again.`
            : "No previous version is recorded on this drive."}
        </Text>
      </section>

      {result ? (
        <Card padding="md">
          <Stack gap={3}>
            <Text>
              {result.to.version} is selected for the next launch ({result.to.dir}).
            </Text>
            {"self_test" in result && result.self_test ? (
              <Text tone="secondary">Self-test commands passed: {result.self_test.commands.length}.</Text>
            ) : null}
            {result.warnings.map((warning, index) => (
              <Notice key={index} tone="warning">
                {warning}
              </Notice>
            ))}
          </Stack>
        </Card>
      ) : null}

      <Dialog
        open={review !== undefined}
        title={review === "apply" ? "Apply this signed package?" : "Roll back to the previous version?"}
        description={`Drive: ${driveRoot ?? "unknown"}`}
        onClose={() => {
          if (idle) closeReview();
        }}
      >
        <Stack gap={4}>
          {error ? (
            <Notice tone="danger" role="alert">
              {error}
            </Notice>
          ) : null}
          {busy ? <Notice role="status">{busy}</Notice> : null}
          {review === "apply" && checked ? (
            <>
              <KeyValueList
                items={[
                  { label: "Version", value: `${checked.candidate.version} (${checked.action.replace(/_/g, " ")})` },
                  { label: "Package", value: checked.package },
                ]}
              />
              <Text tone="secondary">
                AiDEX Core checks the package again, then stages, verifies, and self-tests it before selecting it for the next
                launch.
              </Text>
            </>
          ) : (
            <>
              <KeyValueList
                items={[
                  { label: "Selected now", value: data?.current?.version ?? "Unknown" },
                  { label: "Rolls back to", value: data?.previous?.version ?? "Unavailable" },
                ]}
              />
              <Text tone="secondary">
                AiDEX Core verifies the previous version and its signature again before selecting it for the next launch.
              </Text>
            </>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={!idle} onClick={closeReview}>
              Cancel
            </Button>
            {review === "apply" ? (
              <Button variant="primary" disabled={!idle || !!applyRefusal} focusableWhenDisabled={!idle} onClick={apply}>
                {idle ? "Apply package" : "Applying…"}
              </Button>
            ) : (
              <Button
                variant="primary"
                disabled={!idle || !data?.previous || pending}
                focusableWhenDisabled={!idle}
                onClick={rollback}
              >
                {idle ? "Roll back" : "Rolling back…"}
              </Button>
            )}
          </Stack>
        </Stack>
      </Dialog>
    </Stack>
  );
}
