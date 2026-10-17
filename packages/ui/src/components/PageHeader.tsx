import type { ReactNode, Ref } from "react";
import { ArrowLeftIcon } from "../icons/interface";
import { cx } from "../utils/cx";
import styles from "./PageHeader.module.css";

export type PageHeaderProps = {
  title: ReactNode;
  description?: ReactNode;
  /** Small label above the title, e.g. the parent section. */
  eyebrow?: ReactNode;
  /** Page-level actions, aligned to the end of the title row. */
  actions?: ReactNode;
  /** Status line under the description, e.g. run state, workspace, and branch. */
  meta?: ReactNode;
  /** Back link for drill-in pages. */
  back?: { label: string; onClick: () => void };
  /** Lets a page move focus to its title on arrival; the title is focusable programmatically. */
  titleRef?: Ref<HTMLHeadingElement>;
  className?: string;
};

/** Page title row: heading, description, and actions, with optional back link and status line. */
export function PageHeader({ title, description, eyebrow, actions, meta, back, titleRef, className }: PageHeaderProps) {
  return (
    <header className={cx(styles.header, className)}>
      {back ? (
        <button type="button" className={styles.back} onClick={back.onClick}>
          <ArrowLeftIcon />
          {back.label}
        </button>
      ) : null}
      <div className={styles.row}>
        <div className={styles.text}>
          {eyebrow ? <p className={styles.eyebrow}>{eyebrow}</p> : null}
          <h1 ref={titleRef} tabIndex={-1} className={styles.title}>
            {title}
          </h1>
          {description ? <p className={styles.description}>{description}</p> : null}
        </div>
        {actions ? <div className={styles.actions}>{actions}</div> : null}
      </div>
      {meta ? <div className={styles.meta}>{meta}</div> : null}
    </header>
  );
}

export type SectionHeaderProps = {
  title: ReactNode;
  level?: 2 | 3;
  /** Muted detail beside the title, e.g. "3 / 4 complete". */
  meta?: ReactNode;
  actions?: ReactNode;
  /** Lets a page move focus to the section's title, e.g. once its list is empty; the title is then focusable programmatically. */
  titleRef?: Ref<HTMLHeadingElement>;
  className?: string;
};

/** Heading row for a section within a page. */
export function SectionHeader({ title, level = 2, meta, actions, titleRef, className }: SectionHeaderProps) {
  const Heading = level === 2 ? "h2" : "h3";
  return (
    <div className={cx(styles.section, className)}>
      <Heading
        ref={titleRef}
        tabIndex={titleRef ? -1 : undefined}
        className={cx(styles.sectionTitle, level === 3 && styles.small)}
      >
        {title}
      </Heading>
      {meta || actions ? (
        <div className={styles.sectionEnd}>
          {meta ? <span className={styles.sectionMeta}>{meta}</span> : null}
          {actions}
        </div>
      ) : null}
    </div>
  );
}
