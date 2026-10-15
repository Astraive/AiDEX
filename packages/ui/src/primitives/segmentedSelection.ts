export type SegmentedSelection = {
  /** Index of the option whose value matches, or -1 when none does (then no option is checked). */
  checkedIndex: number;
  /** Index of the option that takes Tab focus: the checked one, or the first when none is checked. */
  tabStopIndex: number;
};

/**
 * Selection state of a single-choice group. A value outside the options checks nothing, rather than
 * claiming the first option is selected, but the first option still takes Tab focus so the group
 * stays reachable from the keyboard.
 */
export function segmentedSelection<Value extends string>(
  options: readonly { value: Value }[],
  value: Value | undefined,
): SegmentedSelection {
  const checkedIndex = options.findIndex((option) => option.value === value);
  return { checkedIndex, tabStopIndex: checkedIndex === -1 ? 0 : checkedIndex };
}
