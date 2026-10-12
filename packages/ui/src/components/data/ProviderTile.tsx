import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { cx } from "../../utils/cx";
import styles from "./ProviderTile.module.css";

export type ProviderTileProps = Omit<ComponentPropsWithRef<"button">, "onSelect" | "children" | "type"> & {
  /** Brand glyph key; unknown keys show a monogram of `name`. */
  brand: string;
  name: string;
  /** One line of supporting text; longer text is truncated and kept in the tooltip. */
  description?: string;
  /** Small `Badge`s such as "API key" or "Local". */
  badges?: ReactNode;
  /**
   * Marks the tile as the current choice (`aria-pressed`). Leave undefined when the tile is a plain
   * action that opens the next step.
   */
  selected?: boolean;
  onSelect: () => void;
  disabled?: boolean;
};

/** Compact catalog card for choosing a provider or runtime to add. */
export function ProviderTile({
  brand,
  name,
  description,
  badges,
  selected,
  onSelect,
  disabled = false,
  className,
  title,
  ...props
}: ProviderTileProps) {
  const descriptionId = useId();
  return (
    <button
      type="button"
      className={cx(styles.tile, selected && styles.selected, className)}
      aria-pressed={selected}
      aria-describedby={description ? descriptionId : undefined}
      title={title ?? description}
      disabled={disabled}
      onClick={onSelect}
      {...props}
    >
      <span className={styles.header}>
        <span className={styles.logo}>
          <BrandIcon brand={brand} monogramLabel={name} size={28} aria-hidden />
        </span>
        <span className={styles.name}>{name}</span>
      </span>
      {description ? (
        <span id={descriptionId} className={styles.description}>
          {description}
        </span>
      ) : null}
      {badges ? <span className={styles.badges}>{badges}</span> : null}
    </button>
  );
}
