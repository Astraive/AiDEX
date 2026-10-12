import { useRef, useState, type FormEvent } from "react";
import { Dialog, Notice } from "@aidex/ui/components";
import { FolderIcon, ShieldCheckIcon } from "@aidex/ui/icons";
import { Button, Stack, Text, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { PluginDetail, PluginInstallOutcome, PluginSummary } from "@aidex/client/dto/plugins";
import { useToast } from "../../../app/providers/toast";
import { describeError, reasonOf, useAction, useExtensionsQuery } from "../data";
import { PluginContents } from "./PluginContents";

type TrustProps = {
  detail: PluginDetail;
  onClose: () => void;
  onTrusted: (plugin: PluginSummary) => void;
  /** Label of the button that leaves the plugin as it is. */
  dismissLabel: string;
};

/** The review contents plus "Trust and enable", which trusts exactly the tree hash shown. */
function ReviewAndTrust({ detail, onClose, onTrusted, dismissLabel }: TrustProps) {
  const action = useAction();
  const notify = useToast();
  const hash = detail.tree_sha256;

  const trust = async () => {
    if (!hash) return;
    const trusted = await action.run("trust", () => call("plugin.trust", { name: detail.name, tree_sha256: hash, enable: true }));
    if (!trusted) return;
    notify(
      detail.mcp_server_count > 0
        ? `${detail.name} is on. Its MCP servers were added turned off.`
        : `${detail.name} is on.`,
      "success",
    );
    onTrusted(trusted);
  };

  return (
    <Stack gap={5}>
      <PluginContents detail={detail} />
      {!hash ? (
        <Notice tone="danger">The plugin's files can't be read, so there is nothing to trust. Reinstall it.</Notice>
      ) : null}
      {action.error ? (
        <Notice tone="danger" role="alert">
          {action.failure && reasonOf(action.failure) === "hash_mismatch"
            ? "The plugin's files changed while you were reviewing them. Close this and review it again."
            : action.error}
        </Notice>
      ) : null}
      <Stack direction="row" gap={2} justify="end">
        <Button variant="ghost" onClick={onClose}>
          {dismissLabel}
        </Button>
        <Button
          variant="primary"
          icon={<ShieldCheckIcon />}
          disabled={!hash || action.busy !== null}
          onClick={() => void trust()}
        >
          {action.busy ? "Enabling…" : "Trust and enable"}
        </Button>
      </Stack>
    </Stack>
  );
}

const REVIEW_INTRO =
  "Plugins bundle skills and MCP servers. Trusting one approves exactly these files: if any file changes later, AiDEX turns the plugin off until you review it again.";

type ReviewProps = {
  open: boolean;
  name: string;
  onClose: () => void;
  onTrusted: (plugin: PluginSummary) => void;
};

/** Reviews an installed plugin (`plugin.show`) before trusting and enabling it. */
export function PluginReviewDialog({ open, name, onClose, onTrusted }: ReviewProps) {
  const detail = useExtensionsQuery("plugin.show", { name }, { enabled: open });
  return (
    <Dialog open={open} size="lg" title={`Review ${name}`} description={REVIEW_INTRO} onClose={onClose}>
      {detail.data && detail.data.name === name ? (
        <ReviewAndTrust detail={detail.data} onClose={onClose} onTrusted={onTrusted} dismissLabel="Not now" />
      ) : detail.error ? (
        <Notice tone="danger" role="alert">
          {describeError(detail.error)}
        </Notice>
      ) : (
        <Text tone="muted">Reading the plugin…</Text>
      )}
    </Dialog>
  );
}

type InstallProps = {
  open: boolean;
  onClose: () => void;
  /** Called once the plugin is installed (turned off), so the list can select it. */
  onInstalled: (name: string) => void;
};

/**
 * "Install from folder": copies a local folder or `.zip` into the drive's plugins (turned off), then
 * shows the review so the user can trust and enable it. Give it a new `key` per opening.
 */
export function PluginInstallDialog({ open, onClose, onInstalled }: InstallProps) {
  const [path, setPath] = useState("");
  const [pathError, setPathError] = useState<string | undefined>();
  const [installed, setInstalled] = useState<PluginInstallOutcome | null>(null);
  const action = useAction();
  const pathRef = useRef<HTMLInputElement>(null);
  const exists = reasonOf(action.failure) === "plugin_exists";

  const install = async (replace: boolean) => {
    const sourcePath = path.trim();
    if (!sourcePath) {
      setPathError("Enter the full path of a plugin folder or .zip file.");
      pathRef.current?.focus();
      return;
    }
    const outcome = await action.run("install", () =>
      call("plugin.install", { source_path: sourcePath, ...(replace ? { replace: true } : {}) }),
    );
    if (!outcome) return;
    setInstalled(outcome);
    onInstalled(outcome.name);
  };

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void install(false);
  };

  return (
    <Dialog
      open={open}
      size="lg"
      title={installed ? `Review ${installed.name}` : "Install a plugin"}
      description={
        installed
          ? `${installed.replaced ? "Replaced and turned off." : "Installed and turned off."} ${REVIEW_INTRO}`
          : "AiDEX copies the plugin onto the drive and leaves it off until you review and trust it. Plugins carry skills and MCP servers, never other programs."
      }
      onClose={onClose}
    >
      {installed ? (
        <ReviewAndTrust detail={installed} onClose={onClose} onTrusted={onClose} dismissLabel="Keep it off" />
      ) : (
        <form onSubmit={submit} noValidate aria-label="Install a plugin">
          <Stack gap={4}>
            <TextField
              ref={pathRef}
              label="Plugin folder or .zip"
              size="md"
              leadingIcon={<FolderIcon />}
              placeholder="D:\plugins\code-review"
              autoComplete="off"
              spellCheck={false}
              value={path}
              error={pathError}
              hint="The folder holds aidex-plugin.json or .claude-plugin/plugin.json, plus skills/ and an optional .mcp.json."
              onChange={(event) => {
                setPath(event.target.value);
                setPathError(undefined);
                action.clearError();
              }}
            />
            {action.error ? (
              <Notice tone={exists ? "warning" : "danger"} role="alert">
                <Stack gap={3} align="start">
                  <Text as="span">
                    {exists ? "A plugin with this name is already installed. Replacing it turns it off until you trust it again." : action.error}
                  </Text>
                  {exists ? (
                    <Button variant="outline" disabled={action.busy !== null} onClick={() => void install(true)}>
                      Replace the installed plugin
                    </Button>
                  ) : null}
                </Stack>
              </Notice>
            ) : null}
            <Stack direction="row" gap={2} justify="end">
              <Button variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" disabled={action.busy !== null}>
                {action.busy ? "Installing…" : "Install"}
              </Button>
            </Stack>
          </Stack>
        </form>
      )}
    </Dialog>
  );
}
