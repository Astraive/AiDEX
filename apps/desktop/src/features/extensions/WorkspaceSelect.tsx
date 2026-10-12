import { Select } from "@aidex/ui/primitives";
import type { Workspace } from "@aidex/client/dto/workspace";

type Props = {
  label: string;
  /** Hide the label only where the options name the choice, e.g. in a filter bar. */
  labelVisibility?: "visible" | "hidden";
  workspaces: readonly Workspace[];
  /** The chosen workspace id; undefined shows the "none" option. */
  value: string | undefined;
  onChange: (id: string) => void;
  /** Label of the empty choice; omit when a workspace is required. */
  noneLabel?: string;
  /** Text before each workspace name, e.g. "Workspace: " when the label is hidden. */
  optionPrefix?: string;
  hint?: string;
  disabled?: boolean;
};

/** Picks one of the drive's available workspaces, optionally with a "no workspace" choice. */
export function WorkspaceSelect({
  label,
  labelVisibility,
  workspaces,
  value,
  onChange,
  noneLabel,
  optionPrefix = "",
  hint,
  disabled,
}: Props) {
  const options = [
    ...(noneLabel !== undefined ? [{ value: "", label: noneLabel }] : []),
    ...workspaces.map((workspace) => ({ value: workspace.id, label: `${optionPrefix}${workspace.name}` })),
  ];
  return (
    <Select
      label={label}
      labelVisibility={labelVisibility}
      options={options}
      value={value ?? ""}
      hint={hint}
      disabled={disabled || options.length === 0}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}
