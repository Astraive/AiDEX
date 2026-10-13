import { useRef, useState, type FormEvent } from "react";
import { Notice } from "@aidex/ui/components";
import { Button, Grid, PasswordField, Stack } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { MIN_PASSPHRASE_CHARS } from "@aidex/client/dto/vault";

type Props = {
  /** After a recovery-key unlock the daemon accepts a new passphrase without the old one. */
  unlockedByRecoveryKey: boolean;
  onChanged: () => void;
};

type Errors = { current?: string; next?: string; confirm?: string; form?: string };

/**
 * Changes the vault passphrase (`vault.passphrase.change`). Passphrases go straight to the daemon and
 * every field is cleared after each attempt; nothing is kept, logged, or shown.
 */
export function PassphraseForm({ unlockedByRecoveryKey, onChanged }: Props) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const [changed, setChanged] = useState(false);
  const currentRef = useRef<HTMLInputElement>(null);
  const nextRef = useRef<HTMLInputElement>(null);

  const clear = () => {
    setCurrent("");
    setNext("");
    setConfirm("");
  };

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setChanged(false);
    const found: Errors = {};
    if (!current && !unlockedByRecoveryKey) found.current = "Enter the current passphrase.";
    if ([...next].length < MIN_PASSPHRASE_CHARS) found.next = `Use at least ${MIN_PASSPHRASE_CHARS} characters.`;
    else if (next === current) found.next = "Choose a passphrase different from the current one.";
    if (confirm !== next) found.confirm = "The passphrases do not match.";
    setErrors(found);
    if (found.current) return currentRef.current?.focus();
    if (found.next || found.confirm) return nextRef.current?.focus();

    setBusy(true);
    try {
      await call("vault.passphrase.change", { ...(current ? { old_passphrase: current } : {}), new_passphrase: next });
      setChanged(true);
      onChanged();
    } catch (caught) {
      const failure = toAidexError(caught);
      const reason = (failure.data as { reason?: string } | undefined)?.reason;
      setErrors(
        reason === "wrong_passphrase"
          ? { current: "That is not the current passphrase." }
          : reason === "passphrase_required"
            ? { current: "Enter the current passphrase." }
            : { form: failure.message },
      );
    } finally {
      clear();
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} noValidate aria-label="Change passphrase">
      <Stack gap={5}>
        {changed ? (
          <Notice tone="success" role="status">
            Passphrase changed. Use the new one next time you unlock this drive.
          </Notice>
        ) : null}
        {errors.form ? (
          <Notice tone="danger" role="alert">
            {errors.form}
          </Notice>
        ) : null}
        <Grid columns={3} gap={4}>
          <PasswordField
            ref={currentRef}
            label="Current passphrase"
            autoComplete="current-password"
            revealLabel="Show current passphrase"
            value={current}
            disabled={busy}
            error={errors.current}
            hint={unlockedByRecoveryKey ? "Optional: the vault was unlocked with the recovery key." : undefined}
            onChange={(event) => {
              setCurrent(event.target.value);
              setErrors((value) => ({ ...value, current: undefined }));
            }}
          />
          <PasswordField
            ref={nextRef}
            label="New passphrase"
            autoComplete="new-password"
            revealLabel="Show new passphrase"
            value={next}
            disabled={busy}
            error={errors.next}
            onChange={(event) => {
              setNext(event.target.value);
              setErrors((value) => ({ ...value, next: undefined }));
            }}
          />
          <PasswordField
            label="Repeat new passphrase"
            autoComplete="new-password"
            revealLabel="Show repeated passphrase"
            value={confirm}
            disabled={busy}
            error={errors.confirm}
            onChange={(event) => {
              setConfirm(event.target.value);
              setErrors((value) => ({ ...value, confirm: undefined }));
            }}
          />
        </Grid>
        <Stack direction="row" justify="end">
          <Button type="submit" variant="outline" size="md" disabled={busy}>
            {busy ? "Changing…" : "Change passphrase"}
          </Button>
        </Stack>
      </Stack>
    </form>
  );
}
