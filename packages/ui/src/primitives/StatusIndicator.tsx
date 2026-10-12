import { cx } from "../utils/cx";
import { StatusDot } from "./StatusDot";
import styles from "./StatusIndicator.module.css";

export type StatusIndicatorProps = {
  label: string;
  tone?: "success" | "warning" | "danger" | "neutral";
  className?: string;
};

/** Colored dot with a text label; the label carries the meaning, so color is never the only signal. */
export function StatusIndicator({ label, tone = "neutral", className }: StatusIndicatorProps) {
  return (
    <span className={cx(styles.status, className)}>
      <StatusDot tone={tone} size="md" />
      {label}
    </span>
  );
}
