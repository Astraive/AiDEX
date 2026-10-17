import type { ComponentPropsWithRef } from "react";
import { LogoMark } from "../icons/LogoMark";
import { cx } from "../utils/cx";
import styles from "./Brand.module.css";

export type BrandProps = Omit<ComponentPropsWithRef<"span">, "children"> & {
  name?: string;
  /** `md` (24px mark, 16px name) for sidebar headers, `lg` (28 / 18px), `xl` for launch screens. */
  size?: "md" | "lg" | "xl";
  /** `stacked` places the wordmark under the mark, for splash and launch screens. */
  layout?: "inline" | "stacked";
};

/** Logo mark with the product wordmark; a name too long for its row ends in an ellipsis. */
export function Brand({ name = "AiDEX", size = "lg", layout = "inline", className, ...props }: BrandProps) {
  return (
    <span className={cx(styles.brand, styles[size], styles[layout], className)} {...props}>
      <LogoMark className={styles.mark} />
      <span className={styles.wordmark}>{name}</span>
    </span>
  );
}
