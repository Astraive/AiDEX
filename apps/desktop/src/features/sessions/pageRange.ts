/** "21–40 of 75" for a page of `count` items at `offset`; "0 of 0" when the page is empty. */
export function pageRangeLabel(offset: number, count: number, total: number): string {
  if (count <= 0) return `0 of ${total}`;
  return `${offset + 1}–${offset + count} of ${total}`;
}
