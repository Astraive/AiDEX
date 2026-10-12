import { Fragment, type ComponentPropsWithRef, type ReactNode } from "react";
import { cx } from "../../utils/cx";
import styles from "./TranscriptDivider.module.css";

export type TranscriptDividerProps = Omit<ComponentPropsWithRef<"div">, "children"> & {
  /** What changed, e.g. "Continued as Agent", "Switched to Chat", or "Continued from Trip ideas". */
  label: string;
  /** Facts after the label, separated by middle dots, e.g. the project and the permission. */
  details?: ReactNode[];
  /** Leading icon, e.g. `ArrowRightLeftIcon`. */
  icon?: ReactNode;
};

/**
 * A centered line of text between transcript turns that marks a change in the conversation, such as
 * a switch from Chat to Agent or the point a forked conversation continues from. It is read as
 * ordinary text; the rules on both sides are decorative.
 */
export function TranscriptDivider({ label, details, icon, className, ...props }: TranscriptDividerProps) {
  const facts = (details ?? []).filter((detail) => detail !== null && detail !== undefined && detail !== "");
  return (
    <div className={cx(styles.divider, className)} {...props}>
      <span className={styles.rule} aria-hidden="true" />
      <p className={styles.text}>
        {icon ? (
          <span className={styles.icon} aria-hidden="true">
            {icon}
          </span>
        ) : null}
        <span className={styles.label}>{label}</span>
        {facts.map((detail, index) => (
          <Fragment key={index}>
            <span className={styles.separator} aria-hidden="true">
              ·
            </span>
            <span className={styles.detail}>{detail}</span>
          </Fragment>
        ))}
      </p>
      <span className={styles.rule} aria-hidden="true" />
    </div>
  );
}
