import { useState } from "react";
import { Dialog, Notice, SettingsSection } from "@aidex/ui/components";
import { BrandIcon, hasBrandIcon, PencilIcon, TrashIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, Text, TextField } from "@aidex/ui/primitives";

type Props = {
  brand: string;
  title: string;
  badges: readonly string[];
  /** Current custom name; renaming is offered only when `onRename` is set. */
  displayName?: string | null;
  onRename?: (name: string | null) => Promise<boolean>;
  onRemove?: () => Promise<boolean>;
  /** What else removing deletes, shown in the confirmation. */
  removeDetail?: string;
  busy: boolean;
  error: string | null;
};

const MAX_NAME = 64;

/** Editor header: brand, title, badges, and the Rename and Remove actions. */
export function EditorHeader({
  brand,
  title,
  badges,
  displayName,
  onRename,
  onRemove,
  removeDetail,
  busy,
  error,
}: Props) {
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState("");
  const [confirming, setConfirming] = useState(false);
  const trimmed = name.trim();
  const nameError =
    trimmed.length > MAX_NAME
      ? `Use at most ${MAX_NAME} characters.`
      : // Control characters are refused by the daemon.
        /[\u0000-\u001f\u007f]/.test(trimmed)
        ? "Remove control characters from the name."
        : undefined;

  const saveName = async () => {
    if (!onRename || nameError) return;
    if (await onRename(trimmed || null)) setRenaming(false);
  };

  return (
    <SettingsSection
      titleLevel={2}
      icon={<BrandIcon brand={brand} title={hasBrandIcon(brand) ? undefined : title} size={24} />}
      title={title}
      description={
        badges.length ? (
          <Stack direction="row" gap={2} wrap>
            {badges.map((badge) => (
              <Badge key={badge} tone="neutral" size="sm">
                {badge}
              </Badge>
            ))}
          </Stack>
        ) : undefined
      }
      actions={
        <Stack direction="row" gap={2}>
          {onRename && !renaming && (
            <Button
              variant="ghost"
              icon={<PencilIcon />}
              disabled={busy}
              onClick={() => {
                setName(displayName ?? title);
                setRenaming(true);
              }}
            >
              Rename
            </Button>
          )}
          {onRemove && (
            <Button variant="ghost" icon={<TrashIcon />} disabled={busy} onClick={() => setConfirming(true)}>
              Remove
            </Button>
          )}
        </Stack>
      }
    >
      {renaming && (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void saveName();
          }}
        >
          <Stack gap={3}>
            <TextField
              label="Account name"
              size="md"
              autoFocus
              value={name}
              maxLength={MAX_NAME + 8}
              error={nameError}
              hint="Leave empty to use the provider's name."
              onChange={(event) => setName(event.target.value)}
            />
            <Stack direction="row" gap={2}>
              <Button type="submit" variant="primary" disabled={busy || !!nameError}>
                Save name
              </Button>
              <Button variant="ghost" onClick={() => setRenaming(false)}>
                Cancel
              </Button>
            </Stack>
          </Stack>
        </form>
      )}
      {error && (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      )}
      <Dialog open={confirming} title={`Remove ${title}?`} onClose={() => setConfirming(false)}>
        <Text>Conversations that use it will stop working until you choose another model.</Text>
        {removeDetail ? <Text>{removeDetail}</Text> : null}
        {error && (
          <Notice tone="danger" role="alert">
            {error}
          </Notice>
        )}
        <Stack direction="row" gap={2} justify="end">
          <Button variant="ghost" onClick={() => setConfirming(false)}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={busy}
            onClick={() =>
              void onRemove?.().then((removed) => {
                if (removed) setConfirming(false);
              })
            }
          >
            Remove
          </Button>
        </Stack>
      </Dialog>
    </SettingsSection>
  );
}
