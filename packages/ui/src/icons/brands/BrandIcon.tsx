import type { CSSProperties, SVGProps } from "react";
import { cx } from "../../utils/cx";
import { brandGlyphs, type BrandGlyph, type BrandId } from "./brandGlyphs";
import styles from "./BrandIcon.module.css";

export type { BrandId };

export type BrandIconProps = Omit<SVGProps<SVGSVGElement>, "children"> & {
  /** Brand glyph key such as `openai` or `claude`; unknown keys render a monogram. */
  brand: string;
  size?: number | string;
  /** `brand` (default) uses the brand color when the glyph has one; `mono` inherits `currentColor`. */
  tone?: "brand" | "mono";
  /**
   * Accessible name and tooltip. Omit for decorative icons next to a visible name. Ignored as a name
   * when the icon is `aria-hidden`, so it never covers the enclosing control's tooltip.
   */
  title?: string;
  /** Text the monogram initials come from when there is no glyph (defaults to `title`, then the brand). */
  monogramLabel?: string;
};

function glyphFor(brand: string): BrandGlyph | undefined {
  return Object.prototype.hasOwnProperty.call(brandGlyphs, brand)
    ? (brandGlyphs as Record<string, BrandGlyph>)[brand]
    : undefined;
}

/** True when `brand` has a bundled glyph; other keys render a monogram. */
export function hasBrandIcon(brand: string): brand is BrandId {
  return glyphFor(brand) !== undefined;
}

/** Display name of a brand glyph, or a title-cased form of the key (`my-provider` → "My Provider"). */
export function brandTitle(brand: string): string {
  const glyph = glyphFor(brand);
  if (glyph) return glyph.title;
  return brand
    .split(/[\s_-]+/u)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

/** Up to two initials: the first two characters of a single word, else the first character of the first two words. */
export function monogram(label: string): string {
  const words = label.trim().split(/[\s_-]+/u).filter(Boolean);
  if (words.length === 0) return "?";
  const initials =
    words.length === 1 ? Array.from(words[0]).slice(0, 2).join("") : words.slice(0, 2).map((word) => Array.from(word)[0]).join("");
  return initials.toUpperCase();
}

/** Provider or runtime logo from the bundled brand glyphs, with a monogram fallback. */
export function BrandIcon({
  brand,
  size = 20,
  tone = "brand",
  title,
  monogramLabel,
  className,
  style,
  ...props
}: BrandIconProps) {
  const glyph = glyphFor(brand);
  const hidden = props["aria-hidden"] === true || props["aria-hidden"] === "true";
  // A decorative icon gets no <title>: browsers show the innermost title, which would replace the
  // tooltip of the button or row around it while the pointer is over the glyph.
  const label = hidden ? undefined : title;
  const a11y = label ? ({ role: "img", "aria-label": label } as const) : ({ "aria-hidden": true } as const);

  if (!glyph) {
    const letters = monogram(monogramLabel ?? title ?? brandTitle(brand));
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        focusable="false"
        className={cx(styles.icon, styles.monogram, className)}
        style={style}
        {...a11y}
        {...props}
      >
        {label ? <title>{label}</title> : null}
        <rect className={styles.monogramTile} x="1" y="1" width="22" height="22" rx="6" />
        <text
          className={styles.monogramText}
          x="12"
          y="12.5"
          textAnchor="middle"
          dominantBaseline="central"
          fontSize={letters.length > 1 ? 10 : 12}
        >
          {letters}
        </text>
      </svg>
    );
  }

  const colored = tone === "brand" && glyph.color !== null;
  const iconStyle = colored ? ({ ...style, "--aidex-brand-color": glyph.color } as CSSProperties) : style;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      focusable="false"
      className={cx(styles.icon, colored && styles.brand, className)}
      style={iconStyle}
      {...a11y}
      {...props}
    >
      {label ? <title>{label}</title> : null}
      {glyph.paths.map((path, index) => (
        <path
          // Paths are fixed generated data; their order is their identity.
          key={index}
          d={path.d}
          fillRule={path.evenOdd ? "evenodd" : undefined}
          clipRule={path.evenOdd ? "evenodd" : undefined}
          fillOpacity={path.opacity}
        />
      ))}
    </svg>
  );
}
