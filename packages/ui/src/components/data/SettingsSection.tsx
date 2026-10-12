import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { cx } from "../../utils/cx";
import styles from "./SettingsSection.module.css";

export type SettingsSectionProps = Omit<ComponentPropsWithRef<"section">, "title" | "children"> & {
  title: ReactNode;
  /** Leading icon in the header, e.g. a stroke icon or `BrandIcon`. */
  icon?: ReactNode;
  description?: ReactNode;
  /** Header controls such as `Button`s, aligned to the end of the title row. */
  actions?: ReactNode;
  /** Heading level of the title (default 3). */
  titleLevel?: 2 | 3 | 4;
  children?: ReactNode;
};

/** Bordered, titled settings group: a header row (icon, title, actions), optional description, and a body column. */
export function SettingsSection({
  title,
  icon,
  description,
  actions,
  titleLevel = 3,
  className,
  children,
  ...props
}: SettingsSectionProps) {
  const titleId = useId();
  const Title = `h${titleLevel}` as const;
  return (
    <section aria-labelledby={titleId} className={cx(styles.section, className)} {...props}>
      <div className={styles.header}>
        <div className={styles.heading}>
          {icon ? (
            <span className={styles.icon} aria-hidden="true">
              {icon}
            </span>
          ) : null}
          <Title id={titleId} className={styles.title}>
            {title}
          </Title>
        </div>
        {actions ? <div className={styles.actions}>{actions}</div> : null}
      </div>
      {description ? <div className={styles.description}>{description}</div> : null}
      {children != null && children !== false ? <div className={styles.body}>{children}</div> : null}
    </section>
  );
}
