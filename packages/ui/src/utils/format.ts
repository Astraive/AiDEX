// Display formatting shared by components. Pure functions; `Intl` exists during prerendering too.

const BYTE_UNITS = ["KB", "MB", "GB", "TB"] as const;

/** "0 B", "512 B", "1.5 KB", "12 MB" (binary multiples, one decimal under 10). Invalid input gives "". */
export function formatByteSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  return `${rounded} ${BYTE_UNITS[unit]}`;
}

/** Parses a Date, ISO string, or epoch milliseconds; null when it is not a valid time. */
export function toDate(value: Date | string | number | null | undefined): Date | null {
  if (value === null || value === undefined || value === "") return null;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Medium date and short time in the viewer's locale, e.g. "Oct 11, 2026, 7:18 PM". */
export function formatDateTime(date: Date): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

/**
 * Attachment size label: "3.2 MB" from 1 MiB up, otherwise whole kilobytes rounded up ("48 KB"),
 * so a small file never reads as "0 KB". Invalid input gives "".
 */
export function formatAttachmentSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${Math.max(1, Math.ceil(bytes / 1024))} KB`;
}
