import type { ComponentPropsWithRef } from "react";
import { ExternalLinkIcon } from "../../icons/interface";
import { cx } from "../../utils/cx";
import { safeExternalUrl, urlHost } from "../../utils/url";
import a11y from "./a11y.module.css";
import styles from "./WebResultList.module.css";

export type WebResult = {
  title?: string;
  url: string;
  snippet?: string;
  /** Display-ready date, e.g. "2 days ago" or "May 3, 2026". */
  published?: string;
};

export type WebResultListProps = Omit<ComponentPropsWithRef<"ol">, "children" | "results"> & {
  results: readonly WebResult[];
  /**
   * Called with the result's normalized http(s) URL; the list never navigates itself. Results whose
   * URL is not a safe absolute http(s) URL show as plain text.
   */
  onOpenLink?: (url: string) => void;
  /** Spoken after each title on its open button (default "opens in your browser"). */
  openHint?: string;
  /** Shown instead of the list when there are no results; omit to render nothing. */
  emptyText?: string;
};

/** Search results from a web search tool call: title (opens via `onOpenLink`), host, date, and snippet. */
export function WebResultList({
  results,
  onOpenLink,
  openHint = "opens in your browser",
  emptyText,
  className,
  ...props
}: WebResultListProps) {
  if (results.length === 0) {
    return emptyText ? <p className={styles.empty}>{emptyText}</p> : null;
  }
  return (
    <ol className={cx(styles.list, className)} {...props}>
      {results.map((result, index) => {
        const url = safeExternalUrl(result.url, { allowMailto: false });
        const host = urlHost(result.url);
        const title = result.title?.trim() || host || result.url || "Search result";
        return (
          <li key={`${index}:${result.url}`} className={styles.item}>
            {url && onOpenLink ? (
              <button type="button" className={cx(styles.title, styles.open)} title={url} onClick={() => onOpenLink(url)}>
                <span className={styles.titleText}>{title}</span>
                <ExternalLinkIcon size={14} className={styles.openIcon} />
                <span className={a11y.visuallyHidden}>{`, ${openHint}`}</span>
              </button>
            ) : (
              <span className={styles.title} title={url ?? undefined}>
                <span className={styles.titleText}>{title}</span>
              </span>
            )}
            {host || result.published ? (
              <span className={styles.meta}>
                {host ? <span className={styles.host}>{host}</span> : null}
                {host && result.published ? <span aria-hidden="true">·</span> : null}
                {result.published ? <span>{result.published}</span> : null}
              </span>
            ) : null}
            {result.snippet ? <p className={styles.snippet}>{result.snippet}</p> : null}
          </li>
        );
      })}
    </ol>
  );
}
