import type { ComponentPropsWithRef } from "react";
import { cx } from "../utils/cx";
import styles from "./Card.module.css";

export type CardProps = ComponentPropsWithRef<"div"> & {
  /** `md` (default) for settings groups and forms; `lg` for a lone feature card. */
  padding?: "md" | "lg";
};

/** Bordered surface that groups related content. */
export function Card({ padding = "md", className, ...props }: CardProps) {
  return <div className={cx(styles.card, styles[padding], className)} {...props} />;
}
