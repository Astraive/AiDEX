import { useId, type ComponentPropsWithRef, type Key, type ReactNode } from "react";
import { cx } from "../../utils/cx";
import styles from "./DataTable.module.css";

export type DataTableColumn<Row> = {
  /** Unique column id; also the row field shown when `render` is omitted. */
  key: string;
  header: ReactNode;
  /** `end` right-aligns the column and uses tabular figures, for numbers. */
  align?: "start" | "end";
  /** CSS width, e.g. "120px" or "30%". */
  width?: string;
  /** Renders the cell as a row header (`<th scope="row">`), typically the name column. */
  rowHeader?: boolean;
  render?: (row: Row, index: number) => ReactNode;
};

export type DataTableProps<Row> = Omit<ComponentPropsWithRef<"table">, "children"> & {
  columns: readonly DataTableColumn<Row>[];
  rows: readonly Row[];
  rowKey: (row: Row, index: number) => Key;
  /** Names the table for assistive technology. */
  caption?: ReactNode;
  /** Visually hides the caption when a nearby heading already names the table. */
  hideCaption?: boolean;
  /** Shown in place of rows when `rows` is empty. */
  emptyText?: ReactNode;
};

function defaultCell<Row>(row: Row, key: string): ReactNode {
  const value = (row as unknown as Record<string, unknown>)[key];
  return value == null ? "" : String(value);
}

/**
 * Semantic data table with muted headers, ruled rows, and an empty state. A table wider than its
 * container scrolls sideways inside its own wrapper instead of widening the page. With a caption,
 * the wrapper is a region named by it, so a keyboard-focusable scroller is announced by name.
 */
export function DataTable<Row>({
  columns,
  rows,
  rowKey,
  caption,
  hideCaption = false,
  emptyText = "No data",
  className,
  ...props
}: DataTableProps<Row>) {
  const captionId = useId();
  return (
    <div
      className={styles.wrapper}
      role={caption ? "region" : undefined}
      aria-labelledby={caption ? captionId : undefined}
    >
      <table className={cx(styles.table, className)} {...props}>
        {caption ? (
          <caption id={captionId} className={hideCaption ? styles.visuallyHidden : styles.caption}>
            {caption}
          </caption>
        ) : null}
        <colgroup>
          {columns.map((column) => (
            <col key={column.key} style={column.width ? { width: column.width } : undefined} />
          ))}
        </colgroup>
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column.key} scope="col" className={cx(styles.header, column.align === "end" && styles.end)}>
                {column.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={columns.length} className={styles.empty}>
                {emptyText}
              </td>
            </tr>
          ) : (
            rows.map((row, index) => (
              <tr key={rowKey(row, index)} className={styles.row}>
                {columns.map((column) => {
                  const Cell = column.rowHeader ? "th" : "td";
                  return (
                    <Cell
                      key={column.key}
                      scope={column.rowHeader ? "row" : undefined}
                      className={cx(styles.cell, column.rowHeader && styles.rowHeader, column.align === "end" && styles.end)}
                    >
                      {column.render ? column.render(row, index) : defaultCell(row, column.key)}
                    </Cell>
                  );
                })}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}
