import { DataTable, InlineCode, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { PlusIcon } from "@aidex/ui/icons";
import { Button, Stack, StatusIndicator } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import { sampleFilesystemRules, sampleNetworkRules, type FilesystemRule, type NetworkRule } from "./sample";

const accessTone = { "Read and write": "success", Read: "neutral", Deny: "warning" } as const;
const policyTone = { Allow: "success", Ask: "neutral", Deny: "warning" } as const;

const filesystemColumns: readonly DataTableColumn<FilesystemRule>[] = [
  { key: "path", header: "Path", rowHeader: true, render: (row) => <InlineCode>{row.path}</InlineCode> },
  { key: "access", header: "Access", render: (row) => <StatusIndicator tone={accessTone[row.access]} label={row.access} /> },
  { key: "scope", header: "Scope" },
];

const networkColumns: readonly DataTableColumn<NetworkRule>[] = [
  { key: "domain", header: "Domain", rowHeader: true },
  { key: "policy", header: "Policy", render: (row) => <StatusIndicator tone={policyTone[row.policy]} label={row.policy} /> },
  { key: "scope", header: "Scope" },
];

/** Sample filesystem mounts and network allowlist; the policy engine (crates/policy) has no daemon methods yet. */
export function PolicyRules() {
  const notify = useToast();

  const addRuleButton = (kind: "filesystem" | "network") => (
    <Button
      variant="outline"
      size="md"
      icon={<PlusIcon />}
      aria-label={`Add ${kind} rule`}
      onClick={() => notify(`Preview: adding ${kind} rules will be available when AiDEX Core is connected.`)}
    >
      Add rule
    </Button>
  );

  return (
    <>
      <section>
        <Stack gap={4}>
          <SectionHeader title="Filesystem rules" meta="Sample rules · agents see only these mounts" actions={addRuleButton("filesystem")} />
          <DataTable
            caption="Filesystem rules"
            hideCaption
            columns={filesystemColumns}
            rows={sampleFilesystemRules}
            rowKey={(row) => row.id}
          />
        </Stack>
      </section>
      <section>
        <Stack gap={4}>
          <SectionHeader title="Network rules" meta="Sample rules · allowlist mode" actions={addRuleButton("network")} />
          <DataTable caption="Network rules" hideCaption columns={networkColumns} rows={sampleNetworkRules} rowKey={(row) => row.id} />
        </Stack>
      </section>
    </>
  );
}
