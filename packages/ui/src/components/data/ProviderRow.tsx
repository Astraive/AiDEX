import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { StatusDot } from "../../primitives/StatusDot";
import { cx } from "../../utils/cx";
import styles from "./ProviderRow.module.css";

export type ProviderRowTone = "success" | "warning" | "danger" | "neutral";

export type ProviderRowProps = Omit<ComponentPropsWithRef<"li">, "title" | "onSelect" | "children"> & {
  /** Brand glyph key for the leading tile; unknown keys show a monogram of `title`. */
  brand: string;
  title: string;
  /** Short version or kind tag shown in monospace after the title. */
  version?: string;
  /** Status headline such as "Available" or "Not authenticated". */
  headline: string;
  /** Extra detail appended to the headline after " · ". */
  detail?: string;
  /** Status tone; a dot is shown for `warning` and `danger`, next to the headline text that names the status. */
  tone: ProviderRowTone;
  selected?: boolean;
  /** Dims the row (for example a disabled account); it stays selectable and returns to full strength on hover. */
  disabled?: boolean;
  /** Small `Badge`s after the title. */
  badges?: ReactNode;
  /** Independent controls such as an enable `Switch`, kept outside the select button. */
  trailing?: ReactNode;
  onSelect: () => void;
};

/** Provider or runtime entry in a providers list: brand tile, name, version, status line, and trailing controls. */
export function ProviderRow({
  brand,
  title,
  version,
  headline,
  detail,
  tone,
  selected = false,
  disabled = false,
  badges,
  trailing,
  onSelect,
  className,
  ...props
}: ProviderRowProps) {
  const id = useId();
  const badgesId = `${id}-badges`;
  const summaryId = `${id}-summary`;
  const showDot = tone === "warning" || tone === "danger";
  return (
    <li className={cx(styles.row, selected && styles.selected, disabled && styles.dimmed, className)} {...props}>
      <button
        type="button"
        className={styles.select}
        aria-pressed={selected}
        aria-label={`Select ${title}`}
        aria-describedby={badges ? `${badgesId} ${summaryId}` : summaryId}
        onClick={onSelect}
      >
        <span className={styles.tile}>
          {/* Unknown brands show a monogram of the row title; the button's own label names the row. */}
          <BrandIcon brand={brand} monogramLabel={title} size={20} aria-hidden />
        </span>
        <span className={styles.body}>
          <span className={styles.titleLine}>
            <span className={styles.title}>{title}</span>
            {version ? <code className={styles.version}>{version}</code> : null}
            {badges ? (
              <span id={badgesId} className={styles.badges}>
                {badges}
              </span>
            ) : null}
          </span>
          <span id={summaryId} className={cx(styles.summary, styles[tone])}>
            {showDot ? <StatusDot tone={tone} size="sm" /> : null}
            <span className={styles.summaryText}>
              <span className={styles.headline}>{headline}</span>
              {detail ? <span> · {detail}</span> : null}
            </span>
          </span>
        </span>
      </button>
      {trailing ? <span className={styles.trailing}>{trailing}</span> : null}
    </li>
  );
}
