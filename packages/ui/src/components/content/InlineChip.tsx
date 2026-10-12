import { Fragment, type ComponentPropsWithRef, type ReactNode } from "react";
import { cx } from "../../utils/cx";
import { splitSkillTokens } from "./skillTokens";
import a11y from "./a11y.module.css";
import styles from "./InlineChip.module.css";

export type InlineChipProps = Omit<ComponentPropsWithRef<"span">, "children" | "title"> & {
  /** Visible text, e.g. a skill's display name. */
  label: string;
  /** Decorative leading glyph. */
  icon?: ReactNode;
  /** Native tooltip, e.g. the skill description or the raw `$name` token. */
  tooltip?: string;
  /** Spoken before the label (e.g. "Skill"), so the chip is not just styled text to a screen reader. */
  kindLabel?: string;
  tone?: "accent" | "neutral";
  /** Dashed outline for a token that names nothing known, e.g. a skill that is not installed. */
  unresolved?: boolean;
  /** The source text the chip stands for (e.g. `$code-review`), exposed as `data-raw-text`. */
  rawText?: string;
};

/** Non-interactive inline pill for a token in running text, such as a `$skill` mention. */
export function InlineChip({
  label,
  icon,
  tooltip,
  kindLabel,
  tone = "accent",
  unresolved = false,
  rawText,
  className,
  ...props
}: InlineChipProps) {
  return (
    <span
      title={tooltip}
      data-raw-text={rawText}
      className={cx(styles.chip, styles[tone], unresolved && styles.unresolved, className)}
      {...props}
    >
      {icon ? (
        <span className={styles.icon} aria-hidden="true">
          {icon}
        </span>
      ) : null}
      {kindLabel ? <span className={a11y.visuallyHidden}>{`${kindLabel}: `}</span> : null}
      <span className={styles.label}>{label}</span>
    </span>
  );
}

/**
 * Renders `$name` tokens in plain text through `renderToken` (typically an `InlineChip`); a token it
 * returns `null` for, such as an unknown name or `$HOME`, stays plain text. Amounts like `$20` or
 * `$1.5k` are never tokens. Works as `Markdown`'s `renderText`.
 */
export function renderSkillTokens(
  text: string,
  renderToken: (name: string, raw: string) => ReactNode | null,
): ReactNode {
  const segments = splitSkillTokens(text);
  if (segments.length === 1 && segments[0].type === "text") return text;
  return segments.map((segment, index) => {
    if (segment.type === "text") return <Fragment key={index}>{segment.text}</Fragment>;
    const chip = renderToken(segment.name, segment.raw);
    return <Fragment key={index}>{chip ?? segment.raw}</Fragment>;
  });
}
