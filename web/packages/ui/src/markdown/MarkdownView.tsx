import rehypeShikiFromHighlighter from "@shikijs/rehype/core";
import type { ComponentProps, ReactElement } from "react";
import { useEffect, useRef, useState } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import rehypeReact from "rehype-react";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { createHighlighter } from "shiki";
import type { Processor } from "unified";
import { unified } from "unified";
import { remarkWikiLink, wikiLinkHandler } from "./remarkWikiLink";
import type { Throttler } from "./throttle";
import { throttleTrailing } from "./throttle";

const STREAM_THROTTLE_MS = 100;

const SHIKI_THEMES = { light: "github-light", dark: "github-dark" } as const;

// Languages preloaded into the highlighter. Code fences asking for a language
// outside this list fall back to "text" (see the rehype options below);
// extend the list deliberately when real highlighting is wanted.
const SHIKI_LANGS = [
  "text",
  "markdown",
  "typescript",
  "javascript",
  "tsx",
  "jsx",
  "rust",
  "bash",
  "json",
  "yaml",
  "toml",
  "python",
  "html",
  "css",
  "sql",
  "diff",
] as const;

/** Anchors are only allowed to point at safe targets; anything else renders inert. */
const SAFE_URL_PATTERN = /^(?:https?:|mailto:|#|\/|\.\/|\.\.\/)/i;

type AnchorProps = ComponentProps<"a"> & { "data-wikilink"?: unknown };

/**
 * Wikilink-aware anchor. `[[Target]]` arrives as `<a data-wikilink="target">`
 * without an href (the app resolves document links); regular links pass
 * through unless their protocol is unsafe (`javascript:`, `data:`, ...), in
 * which case the anchor renders without an href.
 *
 * Wikilink source nodes have a document title rather than a URL, so they
 * receive an encoded fragment href to retain native link focus and Enter
 * activation. Space re-dispatches a real click so the app's delegated click
 * handler resolves the navigation exactly like a mouse click.
 */
function SafeAnchor(props: AnchorProps) {
  const { className, href, ...rest } = props;
  const wikiTarget = props["data-wikilink"];
  const classes = className ? `${className} md-wikilink` : "md-wikilink";
  if (typeof wikiTarget === "string") {
    return (
      <a
        {...rest}
        href={`#${encodeURIComponent(wikiTarget)}`}
        className={classes}
        onKeyDown={(event) => {
          if (event.key === " ") {
            event.preventDefault();
            event.currentTarget.click();
          }
        }}
      />
    );
  }
  if (href === undefined || SAFE_URL_PATTERN.test(href)) {
    return <a {...rest} href={href} className={className} />;
  }
  return <a {...rest} className={className} />;
}

function InlineCode(props: ComponentProps<"code">) {
  const { className, ...rest } = props;
  return <code {...rest} className={className ? `${className} md-code` : "md-code"} />;
}

function isReactElement(value: unknown): value is ReactElement {
  return typeof value === "object" && value !== null && "type" in value && "props" in value;
}

let resolvedProcessor: Processor | null = null;
let processorPromise: Promise<Processor> | null = null;

/**
 * Module-level pipeline, built exactly once (no per-render rebuild, no
 * useMemo). @shikijs/rehype's default export needs an async highlighter, so
 * the processor is constructed when `createHighlighter` resolves and cached
 * forever after; `resolvedProcessor` mirrors the promise for synchronous
 * reads (SSR / tests). Until then MarkdownView renders a lightweight
 * fallback. `rehypeShikiFromHighlighter` (the core export) keeps the built
 * processor fully synchronous, so `processSync` is safe during render.
 */
export function getMarkdownProcessor(): Promise<Processor> {
  processorPromise ??= createHighlighter({
    themes: [SHIKI_THEMES.light, SHIKI_THEMES.dark],
    langs: [...SHIKI_LANGS],
  }).then((highlighter) => {
    const processor = unified()
      .use(remarkParse)
      .use(remarkWikiLink)
      .use(remarkGfm)
      .use(remarkRehype, { handlers: { wikiLink: wikiLinkHandler } })
      // fallbackLanguage: an unsupported fence language (e.g. mermaid) must
      // not make processSync throw and take down the whole render.
      .use(rehypeShikiFromHighlighter, highlighter, {
        themes: SHIKI_THEMES,
        fallbackLanguage: "text",
      })
      .use(rehypeReact, {
        Fragment,
        jsx,
        jsxs,
        components: { a: SafeAnchor, code: InlineCode },
      });
    resolvedProcessor = processor;
    return processor;
  });
  return processorPromise;
}

export interface MarkdownViewProps {
  /** Markdown source to render. */
  content: string;
  /**
   * When true (SSE streaming), re-parses are throttled to one per 100ms with
   * a trailing edge. When false, the final content renders immediately and
   * any pending throttled flush is cancelled.
   */
  streaming?: boolean;
}

export function MarkdownView({ content, streaming = false }: MarkdownViewProps) {
  const [processor, setProcessor] = useState<Processor | null>(resolvedProcessor);
  const [visible, setVisible] = useState(content);
  const throttlerRef = useRef<Throttler<string> | null>(null);

  useEffect(() => {
    let active = true;
    void getMarkdownProcessor().then((ready) => {
      if (active) setProcessor(ready);
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!streaming) {
      throttlerRef.current?.cancel();
      setVisible(content);
      return;
    }
    if (throttlerRef.current === null) {
      throttlerRef.current = throttleTrailing(setVisible, STREAM_THROTTLE_MS);
    }
    throttlerRef.current(content);
  }, [content, streaming]);

  useEffect(
    () => () => {
      throttlerRef.current?.cancel();
    },
    [],
  );

  if (processor === null) {
    return <div className="md-root md-fallback">{content}</div>;
  }

  const file = processor.processSync(visible);
  if (!isReactElement(file.result)) {
    return <div className="md-root md-fallback">{content}</div>;
  }
  return <div className="md-root">{file.result}</div>;
}
