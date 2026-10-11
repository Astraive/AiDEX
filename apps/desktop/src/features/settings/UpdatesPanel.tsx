import { useEffect, useRef, useState } from "react";
import {
  Dialog,
  KeyValueList,
  Notice,
  SectionHeader,
} from "@aidex/ui/components";
import {
  Button,
  Card,
  Stack,
  Switch,
  Text,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type {
  ApplyReport,
  CheckReport,
  RollbackReport,
  UpdateStatus,
} from "@aidex/client/dto/updates";
import { useRpc } from "../../hooks/useRpc";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import {
  absoluteUpdateSource,
  applySelectionReason,
  sameCandidate,
} from "./updateSelection";

export function UpdatesPanel() {
  const status = useRpc("update.status");
  const { driveRoot, connectionKey } = useAidexStatus();
  const currentConnection = useRef(connectionKey);
  currentConnection.current = connectionKey;
  const [source, setSource] = useState("");
  const [allowDowngrade, setAllowDowngrade] = useState(false);
  const [checked, setChecked] = useState<CheckReport>();
  const [result, setResult] = useState<ApplyReport | RollbackReport>();
  const [verification, setVerification] = useState<UpdateStatus>();
  const [busy, setBusy] = useState(false);
  const [operation, setOperation] = useState("");
  const [error, setError] = useState<string>();
  const [review, setReview] = useState<"apply" | "rollback">();
  useEffect(() => {
    setChecked(undefined);
    setResult(undefined);
    setVerification(undefined);
    setReview(undefined);
    setError(undefined);
  }, [connectionKey]);
  useEffect(() => {
    if (!busy && !status.data?.pending) return;
    const timer = setInterval(status.refetch, 1500);
    return () => clearInterval(timer);
  }, [busy, status.data?.pending, status.refetch]);
  const act = async (label: string, perform: () => Promise<void>) => {
    const key = connectionKey;
    setBusy(true);
    setOperation(label);
    setError(undefined);
    try {
      await perform();
    } catch (caught) {
      if (currentConnection.current === key) {
        const failure = toAidexError(caught);
        const reason = (
          failure.data as { reason?: string; integrity?: boolean } | undefined
        )?.reason;
        setError(`${failure.message}${reason ? ` · ${reason}` : ""}`);
      }
    } finally {
      if (currentConnection.current === key) status.refetch();
      setBusy(false);
      setOperation("");
    }
  };
  const check = () =>
    void act(
      "Checking signature, package integrity, and version rules…",
      async () => {
        if (
          !status.data ||
          !absoluteUpdateSource(source.trim(), status.data.target)
        )
          throw new Error(
            "Enter an absolute local package or directory path for this host. URLs and relative paths are unsupported.",
          );
        const key = connectionKey;
        const report = await call("update.check", {
          source: source.trim(),
          allow_downgrade: allowDowngrade,
          allow_unsigned: false,
        });
        if (currentConnection.current === key) {
          setChecked(report);
          setResult(undefined);
        }
      },
    );
  const apply = () =>
    void act(
      "Staging, re-verifying, and self-testing the signed package…",
      async () => {
        const key = connectionKey;
        const refusal = applySelectionReason(
          checked,
          status.data?.target,
          status.data?.pending ?? true,
        );
        if (refusal) throw new Error(refusal);
        const selected = checked!;
        const refreshed = await call("update.check", {
          source: selected.package,
          allow_downgrade: allowDowngrade,
          allow_unsigned: false,
        });
        if (currentConnection.current !== key) return;
        if (!sameCandidate(selected, refreshed)) {
          setChecked(refreshed);
          setReview(undefined);
          throw new Error(
            "The signed source changed after review. Review the refreshed candidate before applying it.",
          );
        }
        const report = await call("update.apply", {
          source: selected.package,
          allow_downgrade: allowDowngrade,
          allow_unsigned: false,
        });
        if (currentConnection.current === key) {
          setResult(report);
          setChecked(undefined);
          setReview(undefined);
        }
      },
    );
  const rollback = () =>
    void act(
      "Verifying and selecting the recorded previous bundle…",
      async () => {
        const key = connectionKey,
          previous = status.data?.previous;
        if (!previous) throw new Error("No previous bundle is recorded.");
        const fresh = await call("update.status");
        if (currentConnection.current !== key) return;
        if (
          fresh.pending ||
          fresh.previous?.manifest_sha256 !== previous.manifest_sha256 ||
          fresh.previous?.version !== previous.version ||
          fresh.previous?.dir !== previous.dir
        ) {
          setReview(undefined);
          throw new Error(
            "The previous bundle or update state changed. Refresh and review it again.",
          );
        }
        const report = await call("update.rollback", { allow_unsigned: false });
        if (currentConnection.current === key) {
          setResult(report);
          setChecked(undefined);
          setReview(undefined);
        }
      },
    );
  const reason = applySelectionReason(
    checked,
    status.data?.target,
    status.data?.pending ?? true,
  );
  return (
    <Stack gap={5}>
      <CoreStatusNotice requires="daemon" />
      <SectionHeader title="Signed updates on this drive" />
      {(error || status.error) && (
        <Notice tone="danger">{error ?? status.error?.message}</Notice>
      )}
      {busy && (
        <Notice role="status">
          {operation} Core enforces package/extraction limits and self-test
          deadlines.
        </Notice>
      )}
      {status.data && (
        <KeyValueList
          items={[
            { label: "Drive", value: driveRoot ?? "Unavailable" },
            { label: "Host target", value: status.data.target },
            { label: "Running daemon", value: status.data.daemon_version },
            {
              label: "Selected bundle",
              value: status.data.current?.version ?? "Not installed",
            },
            {
              label: "Recorded previous",
              value: status.data.previous?.version ?? "None",
            },
            {
              label: "Installed integrity",
              value: status.data.integrity.verified
                ? `Verified · ${status.data.integrity.scope} file scope`
                : (status.data.integrity.reason ?? "Unverified"),
            },
            {
              label: "Signer",
              value: status.data.integrity.signer_key_id ?? "Not verified",
            },
            {
              label: "Verified files",
              value: status.data.integrity.verified_files ?? "Unknown",
            },
            {
              label: "SBOM",
              value: status.data.integrity.sbom.present
                ? status.data.integrity.sbom.valid
                  ? "Present and valid"
                  : "Present; invalid"
                : "Not present",
            },
          ]}
        />
      )}
      {status.data?.integrity.error && (
        <Notice tone="warning">{status.data.integrity.error}</Notice>
      )}
      {status.data?.recovery_error && (
        <Notice tone="danger">
          Startup recovery failed: {status.data.recovery_error}
        </Notice>
      )}
      {status.data?.recovery && status.data.recovery.outcome !== "clean" && (
        <Notice>
          Startup recovery: {status.data.recovery.outcome}.{" "}
          {status.data.recovery.warnings.join(" ")}
        </Notice>
      )}
      {status.data?.pending && (
        <Notice tone="warning">
          An update is unfinished:{" "}
          {status.data.journal?.state ?? "unknown stage"}. Core recovery must
          complete before another package can be applied.
        </Notice>
      )}
      {(status.data?.restart_required || result?.restart_required) && (
        <Notice tone="warning">
          The selected version takes effect on the next launch. Restart AiDEX
          when ready; this action does not replace the running daemon's
          binaries.
        </Notice>
      )}
      <Stack direction="row" gap={2} wrap>
        <Button variant="outline" disabled={busy} onClick={status.refetch}>
          Refresh update status
        </Button>
        <Button
          variant="outline"
          disabled={busy || !status.data}
          onClick={() =>
            void act(
              "Verifying the installed bundle's critical files and signature…",
              async () => {
                const key = connectionKey;
                const report = await call("update.status");
                if (currentConnection.current === key) setVerification(report);
              },
            )
          }
        >
          Verify installed critical files
        </Button>
        <Button
          variant="outline"
          disabled={busy || !status.data?.previous || status.data.pending}
          onClick={() => setReview("rollback")}
        >
          Review rollback
        </Button>
      </Stack>
      {verification && (
        <Notice tone={verification.integrity.verified ? "success" : "warning"}>
          Installed verification:{" "}
          {verification.integrity.verified
            ? "passed"
            : (verification.integrity.reason ?? "failed")}{" "}
          · {verification.integrity.scope} scope ·{" "}
          {verification.integrity.verified_files ?? 0} files.{" "}
          {verification.integrity.error}
        </Notice>
      )}
      <Card padding="md">
        <Stack gap={4}>
          <SectionHeader title="Check a manually supplied signed package" />
          <Text>
            Choose a local .zip/.tar.gz package, unpacked bundle, or release
            directory. Directory checks resolve the newest matching host
            package. This build has no online release catalog or automatic
            update channel.
          </Text>
          <TextField
            label="Absolute package or directory path"
            value={source}
            disabled={busy || status.data?.pending}
            onChange={(event) => {
              setSource(event.target.value);
              setChecked(undefined);
            }}
            hint="Publisher signatures are required. Transfer checksums do not replace trusted signature verification."
          />
          <Stack direction="row" gap={2} align="center">
            <Switch
              label="Permit only the recorded previous package as a downgrade"
              checked={allowDowngrade}
              disabled={busy || status.data?.pending}
              onCheckedChange={(enabled) => {
                setAllowDowngrade(enabled);
                setChecked(undefined);
              }}
            />
            <Text>
              Permit a downgrade only to the exact recorded previous bundle
            </Text>
          </Stack>
          <Button
            disabled={
              busy || !source.trim() || !status.data || status.data.pending
            }
            onClick={check}
          >
            Check signed source
          </Button>
          {checked && (
            <Stack gap={3}>
              <KeyValueList
                items={[
                  { label: "Resolved package", value: checked.package },
                  {
                    label: "Candidate",
                    value: `${checked.candidate.version} · ${checked.candidate.target}`,
                  },
                  { label: "Action", value: checked.action },
                  {
                    label: "Signature",
                    value:
                      checked.candidate.signature.status === "verified"
                        ? `${checked.candidate.signature.key_name} · ${checked.candidate.signature.key_id} · ${checked.candidate.signature.key_source}`
                        : "Unsigned source refused by desktop",
                  },
                  { label: "Transfer checksum", value: checked.checksum },
                  {
                    label: "Manifest digest",
                    value: checked.candidate.manifest_sha256,
                  },
                  {
                    label: "SBOM listed",
                    value: checked.candidate.sbom ? "Yes" : "No",
                  },
                ]}
              />
              <Text tone="muted">
                Checking has not switched the active version. Applying repeats
                verification and runs the staged binaries' self-tests.
              </Text>
              {reason && <Notice>{reason}</Notice>}
              <Button
                disabled={busy || !!reason}
                onClick={() => setReview("apply")}
              >
                Review applying this source
              </Button>
            </Stack>
          )}
        </Stack>
      </Card>
      {result && (
        <Card padding="md">
          <Stack gap={3}>
            <Text>
              Core selected {result.to.version} in {result.to.dir}.
            </Text>
            {"self_test" in result && result.self_test && (
              <Text>
                Self-test commands completed: {result.self_test.commands.length}
                .
              </Text>
            )}
            {result.warnings.map((warning, index) => (
              <Notice key={index} tone="warning">
                {warning}
              </Notice>
            ))}
          </Stack>
        </Card>
      )}
      <Dialog
        open={!!review}
        title={
          review === "apply"
            ? "Apply verified signed source"
            : "Roll back to the recorded bundle"
        }
        onClose={() => {
          if (!busy) setReview(undefined);
        }}
      >
        <Text>Selected drive: {driveRoot ?? "Unavailable"}</Text>
        {review === "apply" && checked ? (
          <>
            <Text>
              Check result: {checked.candidate.version} · {checked.action}
            </Text>
            <Text>{checked.package}</Text>
            <Text>
              The source is checked again before applying. Core stages, verifies
              and self-tests it before selecting the next-launch version.
            </Text>
            <Button disabled={busy || !!reason} onClick={apply}>
              Apply signed source
            </Button>
          </>
        ) : (
          <>
            <Text>
              Current {status.data?.current?.version ?? "unknown"} → recorded
              previous {status.data?.previous?.version ?? "unavailable"}
            </Text>
            <Text>
              Core re-verifies the recorded bundle and its pinned signature
              before changing the next-launch selection.
            </Text>
            <Button
              disabled={busy || !status.data?.previous || status.data.pending}
              onClick={rollback}
            >
              Roll back to recorded previous
            </Button>
          </>
        )}
        <Button
          variant="ghost"
          disabled={busy}
          onClick={() => setReview(undefined)}
        >
          Cancel
        </Button>
      </Dialog>
    </Stack>
  );
}
