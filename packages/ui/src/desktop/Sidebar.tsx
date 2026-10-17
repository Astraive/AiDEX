"use client";

import { useRef, type ReactNode } from "react";
import { useOverflowEdges } from "../utils/useOverflowEdges";
import styles from "./Sidebar.module.css";

export type SidebarProps = {
  /** Accessible name for the complementary landmark. */
  label: string;
  /** Fixed content above the scrolling body, such as the brand row and primary action. */
  header?: ReactNode;
  children: ReactNode;
};

/**
 * Contextual panel with a fixed header and a scrolling list of `SidebarSection`s. It is
 * `--aidex-sidebar-width` wide (208-480px); the body fades at an edge that hides more items.
 */
export function Sidebar({ label, header, children }: SidebarProps) {
  const bodyRef = useRef<HTMLDivElement>(null);
  useOverflowEdges(bodyRef, "y");
  return (
    <aside aria-label={label} className={styles.sidebar}>
      {header ? <div className={styles.header}>{header}</div> : null}
      <div ref={bodyRef} className={styles.body}>
        {children}
      </div>
    </aside>
  );
}
