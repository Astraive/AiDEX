"use client";

import {
  Fragment,
  useMemo,
  type ComponentPropsWithRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { CheckIcon, ImageIcon } from "../../icons/interface";
import { cx } from "../../utils/cx";
import { CodeBlock } from "./CodeBlock";
import { InlineCode } from "./InlineCode";
import { parseMarkdown, type MarkdownBlock, type MarkdownInline, type MarkdownTableAlign } from "./parseMarkdown";
import a11y from "./a11y.module.css";
import styles from "./Markdown.module.css";

export type MarkdownCodeBlock = { code: string; language: string | null; closed: boolean };

export type MarkdownProps = Omit<ComponentPropsWithRef<"div">, "children"> & {
  /** Markdown source. Partial input is fine: an unclosed code fence renders as code while it streams. */
  source: string;
  /**
   * Called with a safe absolute http(s) or mailto URL when a link or image chip is activated. Links
   * never navigate the page themselves; without a handler they render as plain text.
   */
  onOpenLink?: (url: string) => void;
  /** Added to heading levels so `#` nests under the surrounding page outline (e.g. 2 makes `#` an `<h3>`). */
  headingLevelOffset?: number;
  /** Render single newlines inside a paragraph as line breaks instead of spaces. */
  breaks?: boolean;
  /** Header controls for each fenced code block, e.g. a copy button. */
  codeBlockActions?: (block: MarkdownCodeBlock) => ReactNode;
  /** Replaces plain text runs (not code or link text), e.g. to turn `$skill` tokens into chips. */
  renderText?: (text: string) => ReactNode;
};

type RenderContext = {
  onOpenLink?: (url: string) => void;
  headingLevelOffset: number;
  breaks: boolean;
  codeBlockActions?: (block: MarkdownCodeBlock) => ReactNode;
  renderText?: (text: string) => ReactNode;
};

const ALIGN_CLASS: Record<Exclude<MarkdownTableAlign, null>, string | undefined> = {
  left: styles.alignLeft,
  center: styles.alignCenter,
  right: styles.alignRight,
};

/**
 * Safe markdown for model output, skill bodies, plans, and markdown artifacts. Raw HTML is shown as
 * text, never interpreted; links call `onOpenLink` instead of navigating; images are shown as link
 * chips and never loaded.
 */
export function Markdown({
  source,
  onOpenLink,
  headingLevelOffset = 0,
  breaks = false,
  codeBlockActions,
  renderText,
  className,
  ...props
}: MarkdownProps) {
  const blocks = useMemo(() => parseMarkdown(source), [source]);
  const context: RenderContext = { onOpenLink, headingLevelOffset, breaks, codeBlockActions, renderText };
  return (
    <div className={cx(styles.markdown, className)} {...props}>
      {renderBlocks(blocks, context)}
    </div>
  );
}

function renderBlocks(blocks: readonly MarkdownBlock[], context: RenderContext, tight = false): ReactNode[] {
  return blocks.map((block, index) => renderBlock(block, context, index, tight));
}

function renderBlock(block: MarkdownBlock, context: RenderContext, key: number, tight: boolean): ReactNode {
  switch (block.type) {
    case "heading": {
      const level = Math.min(6, Math.max(1, block.level + Math.round(context.headingLevelOffset)));
      const Tag = `h${level}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
      // The visual size follows the source level, so offsetting the outline keeps the look.
      return (
        <Tag key={key} className={cx(styles.block, styles.heading, styles[`h${block.level}`])}>
          {renderInlines(block.children, context, false)}
        </Tag>
      );
    }
    case "paragraph":
      // Tight list items show their text without paragraph spacing.
      return tight ? (
        <span key={key} className={cx(styles.block, styles.tightText)}>
          {renderInlines(block.children, context, false)}
        </span>
      ) : (
        <p key={key} className={styles.block}>
          {renderInlines(block.children, context, false)}
        </p>
      );
    case "code":
      return (
        <CodeBlock
          key={key}
          className={styles.block}
          code={block.code}
          language={block.language ?? undefined}
          actions={context.codeBlockActions?.({ code: block.code, language: block.language, closed: block.closed })}
          data-streaming={block.closed ? undefined : "true"}
        />
      );
    case "list": {
      const items = block.items.map((item, index) => (
        <li key={index} className={cx(styles.item, item.checked !== null && styles.task)}>
          {item.checked !== null ? (
            <span
              role="img"
              aria-label={item.checked ? "Completed" : "Not completed"}
              className={cx(styles.checkbox, item.checked && styles.checked)}
            >
              {item.checked ? <CheckIcon size={12} strokeWidth={2.5} /> : null}
            </span>
          ) : null}
          {renderBlocks(item.children, context, block.tight)}
        </li>
      ));
      return block.ordered ? (
        <ol key={key} className={cx(styles.block, styles.list)} start={block.start !== 1 ? block.start : undefined}>
          {items}
        </ol>
      ) : (
        <ul key={key} className={cx(styles.block, styles.list)}>
          {items}
        </ul>
      );
    }
    case "blockquote":
      return (
        <blockquote key={key} className={cx(styles.block, styles.quote)}>
          {renderBlocks(block.children, context)}
        </blockquote>
      );
    case "table":
      return (
        // Focusable so keyboard users can scroll wide tables, like `CodeBlock`.
        <div key={key} className={cx(styles.block, styles.tableScroll)} tabIndex={0}>
          <table className={styles.table}>
            <thead>
              <tr>
                {block.header.map((cell, index) => (
                  <th key={index} scope="col" className={alignClass(block.align[index])}>
                    {renderInlines(cell, context, false)}
                  </th>
                ))}
              </tr>
            </thead>
            {block.rows.length > 0 ? (
              <tbody>
                {block.rows.map((row, rowIndex) => (
                  <tr key={rowIndex}>
                    {row.map((cell, index) => (
                      <td key={index} className={alignClass(block.align[index])}>
                        {renderInlines(cell, context, false)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            ) : null}
          </table>
        </div>
      );
    case "rule":
      return <hr key={key} className={cx(styles.block, styles.rule)} />;
  }
}

function alignClass(align: MarkdownTableAlign | undefined): string | undefined {
  return align ? ALIGN_CLASS[align] : undefined;
}

function renderInlines(nodes: readonly MarkdownInline[], context: RenderContext, inLink: boolean): ReactNode[] {
  return nodes.map((node, index) => renderInline(node, context, index, inLink));
}

function linkHandlers(url: string, onOpenLink: (url: string) => void) {
  return {
    onClick: (event: MouseEvent<HTMLElement>) => {
      event.preventDefault();
      onOpenLink(url);
    },
    onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
      if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
      event.preventDefault();
      onOpenLink(url);
    },
  };
}

function linkTitle(url: string, title: string | undefined): string {
  return title ? `${title}\n${url}` : url;
}

function renderInline(node: MarkdownInline, context: RenderContext, key: number, inLink: boolean): ReactNode {
  switch (node.type) {
    case "text":
      return context.renderText && !inLink ? <Fragment key={key}>{context.renderText(node.text)}</Fragment> : node.text;
    case "softbreak":
      return context.breaks ? <br key={key} /> : "\n";
    case "break":
      return <br key={key} />;
    case "code":
      return <InlineCode key={key}>{node.code}</InlineCode>;
    case "emphasis":
      return <em key={key}>{renderInlines(node.children, context, inLink)}</em>;
    case "strong":
      return <strong key={key}>{renderInlines(node.children, context, inLink)}</strong>;
    case "strikethrough":
      return <del key={key}>{renderInlines(node.children, context, inLink)}</del>;
    case "link": {
      const children = renderInlines(node.children, context, true);
      if (!node.url || !context.onOpenLink || inLink) {
        return (
          <span key={key} className={styles.inertLink} title={node.url ?? undefined}>
            {children}
          </span>
        );
      }
      // No href: the webview can never navigate, open a new window, or offer "Open link" for it. The
      // destination is the tooltip, so the target is visible before it is opened.
      return (
        <a
          key={key}
          role="link"
          tabIndex={0}
          className={styles.link}
          title={linkTitle(node.url, node.title)}
          {...linkHandlers(node.url, context.onOpenLink)}
        >
          {children}
        </a>
      );
    }
    case "image": {
      const label = node.alt.trim() || "Image";
      const content = (
        <>
          <ImageIcon size={14} className={styles.chipIcon} />
          <span className={a11y.visuallyHidden}>Image: </span>
          <span className={styles.chipLabel}>{label}</span>
        </>
      );
      if (!node.url || !context.onOpenLink || inLink) {
        return (
          <span key={key} className={styles.imageChip} title={node.url ?? undefined}>
            {content}
          </span>
        );
      }
      return (
        <a
          key={key}
          role="link"
          tabIndex={0}
          className={cx(styles.imageChip, styles.interactiveChip)}
          title={linkTitle(node.url, node.title)}
          {...linkHandlers(node.url, context.onOpenLink)}
        >
          {content}
        </a>
      );
    }
  }
}
