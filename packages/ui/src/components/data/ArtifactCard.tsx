import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { FileCodeIcon, FileIcon } from "../../icons/development";
import { ClipboardCheckIcon, DatabaseIcon, GlobeIcon } from "../../icons/system";
import { FileTextIcon, LayersIcon } from "../../icons/objects";
import { Badge } from "../../primitives/Badge";
import { cx } from "../../utils/cx";
import { formatByteSize, formatDateTime, toDate } from "../../utils/format";
import styles from "./ArtifactCard.module.css";

export type ArtifactKind = "document" | "code" | "data" | "diagram" | "plan" | "web_page" | "other";

const KIND_LABEL: Record<ArtifactKind, string> = {
  document: "Document",
  code: "Code",
  data: "Data",
  diagram: "Diagram",
  plan: "Plan",
  web_page: "Web page",
  other: "File",
};

const KIND_ICON: Record<ArtifactKind, ReactNode> = {
  document: <FileTextIcon />,
  code: <FileCodeIcon />,
  data: <DatabaseIcon />,
  diagram: <LayersIcon />,
  plan: <ClipboardCheckIcon />,
  web_page: <GlobeIcon />,
  other: <FileIcon />,
};

/** Own keys only, so names such as `constructor` or `__proto__` count as unknown kinds. */
function isArtifactKind(kind: string): kind is ArtifactKind {
  return Object.prototype.hasOwnProperty.call(KIND_LABEL, kind);
}

/** Display name of an artifact kind; unknown kinds read as "File". */
export function artifactKindLabel(kind: string): string {
  return KIND_LABEL[isArtifactKind(kind) ? kind : "other"];
}

export type ArtifactCardProps = Omit<ComponentPropsWithRef<"article">, "children"> & {
  name: string;
  /** Picks the default icon and badge; unknown kinds show as "File". */
  kind: ArtifactKind | (string & {});
  /** Badge text override, e.g. "Markdown". */
  kindLabel?: string;
  /** Replaces the kind's default icon. */
  icon?: ReactNode;
  /** Size in bytes, shown as "1.2 KB"; or pass `sizeLabel`. */
  size?: number;
  sizeLabel?: string;
  /** Creation time, shown in the viewer's locale; or pass `createdLabel` (e.g. "2 min ago"). */
  createdAt?: Date | string | number;
  createdLabel?: string;
  /** Extra fact for the meta line, e.g. the language or MIME type. */
  detail?: ReactNode;
  /** Makes the name a button, e.g. to open the artifact viewer. */
  onOpen?: () => void;
  /** Buttons such as Open and Download. */
  actions?: ReactNode;
  titleLevel?: 2 | 3 | 4;
};

/** A produced file in a conversation: kind icon, name, kind badge, size, created time, and actions. */
export function ArtifactCard({
  name,
  kind,
  kindLabel,
  icon,
  size,
  sizeLabel,
  createdAt,
  createdLabel,
  detail,
  onOpen,
  actions,
  titleLevel = 3,
  className,
  ...props
}: ArtifactCardProps) {
  const titleId = useId();
  const Title = `h${titleLevel}` as const;
  const known: ArtifactKind = isArtifactKind(kind) ? kind : "other";
  const created = toDate(createdAt);
  const sizeText = sizeLabel ?? (size !== undefined ? formatByteSize(size) : "");
  const facts: ReactNode[] = [];
  if (detail) facts.push(detail);
  if (sizeText) facts.push(sizeText);
  if (created || createdLabel) {
    facts.push(
      created ? (
        // Locale formatting may differ between a prerender and the client; the client value wins.
        <time dateTime={created.toISOString()} suppressHydrationWarning>
          {createdLabel ?? formatDateTime(created)}
        </time>
      ) : (
        createdLabel
      ),
    );
  }

  return (
    <article aria-labelledby={titleId} className={cx(styles.card, className)} {...props}>
      <span className={styles.media} aria-hidden="true">
        {icon ?? KIND_ICON[known]}
      </span>
      <div className={styles.main}>
        <Title id={titleId} className={styles.name}>
          {onOpen ? (
            <button type="button" className={styles.nameButton} title={name} onClick={onOpen}>
              {name}
            </button>
          ) : (
            <span className={styles.nameText} title={name}>
              {name}
            </span>
          )}
        </Title>
        <div className={styles.meta}>
          <Badge tone="neutral" size="sm">
            {kindLabel ?? KIND_LABEL[known]}
          </Badge>
          {facts.map((fact, index) =>
            index > 0 ? (
              // A separator wraps with the fact it introduces, so no line ends in an orphan "·".
              <span key={index} className={styles.group}>
                <span className={styles.separator} aria-hidden="true">
                  ·
                </span>
                <span className={styles.fact}>{fact}</span>
              </span>
            ) : (
              <span key={index} className={styles.fact}>
                {fact}
              </span>
            ),
          )}
        </div>
      </div>
      {actions ? <div className={styles.actions}>{actions}</div> : null}
    </article>
  );
}
