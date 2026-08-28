import { describe, expect, jest, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { getMarkdownProcessor, MarkdownView } from "./MarkdownView";
import { throttleTrailing } from "./throttle";

interface ElementNode {
  type: unknown;
  props: { children?: unknown } & Record<string, unknown>;
}

function isElementNode(value: unknown): value is ElementNode {
  if (typeof value !== "object" || value === null) return false;
  if (!("type" in value) || !("props" in value)) return false;
  return typeof value.props === "object" && value.props !== null;
}

function findAll(node: unknown, predicate: (element: ElementNode) => boolean): ElementNode[] {
  const found: ElementNode[] = [];
  const walk = (current: unknown): void => {
    if (Array.isArray(current)) {
      for (const child of current) walk(child);
      return;
    }
    if (!isElementNode(current)) return;
    if (predicate(current)) found.push(current);
    walk(current.props.children);
  };
  walk(node);
  return found;
}

const GFM_DOC = [
  "# Title",
  "",
  "| Name | Value |",
  "| ---- | ----- |",
  "| alpha | 1 |",
  "| beta | 2 |",
  "",
  "- first",
  "- second",
  "",
].join("\n");

describe("MarkdownView", () => {
  // Declaration order matters: this test runs before any test awaits
  // getMarkdownProcessor(), so the module-level pipeline is still unresolved.
  test("renders a lightweight fallback before the processor resolves", () => {
    const html = renderToStaticMarkup(createElement(MarkdownView, { content: "# raw" }));
    expect(html).toContain("md-fallback");
    expect(html).toContain("# raw");
  });

  test("renders GFM tables and lists", async () => {
    const processor = await getMarkdownProcessor();
    const file = processor.processSync(GFM_DOC);
    const tags = new Set(findAll(file.result, () => true).map((element) => element.type));
    expect(tags.has("table")).toBe(true);
    expect(tags.has("th")).toBe(true);
    expect(tags.has("td")).toBe(true);
    expect(tags.has("ul")).toBe(true);
    expect(tags.has("li")).toBe(true);
  });

  test("converts [[Foo]] into an anchor carrying data-wikilink", async () => {
    const processor = await getMarkdownProcessor();
    const file = processor.processSync("See [[Foo]] here.");
    const anchors = findAll(file.result, (element) => element.props["data-wikilink"] === "Foo");
    expect(anchors).toHaveLength(1);
    expect(anchors[0]?.props.children).toBe("Foo");
  });

  test("leaves [[...]] inside code fences untouched", async () => {
    const processor = await getMarkdownProcessor();
    const file = processor.processSync("```\n[[NotALink]]\n```");
    const anchors = findAll(file.result, (element) => "data-wikilink" in element.props);
    expect(anchors).toHaveLength(0);
  });

  test("highlights code blocks with shiki classes and themed spans", async () => {
    const processor = await getMarkdownProcessor();
    const file = processor.processSync("```ts\nconst value = 1;\n```");
    const pres = findAll(file.result, (element) => element.type === "pre");
    expect(pres).toHaveLength(1);
    const className = pres[0]?.props.className;
    expect(typeof className === "string" && className.includes("shiki")).toBe(true);
    const themedSpans = findAll(file.result, (element) => {
      if (element.type !== "span") return false;
      const style = element.props.style;
      return typeof style === "object" && style !== null;
    });
    expect(themedSpans.length).toBeGreaterThan(0);
  });

  test("drops raw <script> tags from the tree", async () => {
    const processor = await getMarkdownProcessor();
    const file = processor.processSync("<script>alert(1)</script>\n\nplain text");
    const serialized = JSON.stringify(file.result);
    expect(serialized).not.toContain("alert(1)");
    expect(serialized).not.toContain("script");
    const scripts = findAll(file.result, (element) => element.type === "script");
    expect(scripts).toHaveLength(0);
  });

  test("renders javascript: links inert end-to-end", async () => {
    await getMarkdownProcessor();
    const html = renderToStaticMarkup(
      createElement(MarkdownView, { content: "[click](javascript:alert(1))" }),
    );
    expect(html).not.toContain("javascript:");
    expect(html).not.toContain("href");
    expect(html).toContain("<a");
  });

  test("renders raw HTML inert end-to-end", async () => {
    await getMarkdownProcessor();
    const html = renderToStaticMarkup(
      createElement(MarkdownView, { content: "<script>alert(1)</script>\n\nok" }),
    );
    expect(html).not.toContain("<script");
    expect(html).not.toContain("alert(1)");
  });
});

describe("throttleTrailing", () => {
  // bun:test's jest compat provides fake timers; the throttle is tested
  // directly (component effects need a DOM renderer, which the element-tree
  // strategy deliberately avoids).
  test("coalesces bursts into one trailing flush with the latest value", () => {
    jest.useFakeTimers();
    try {
      const flushed: number[] = [];
      const throttled = throttleTrailing((value: number) => flushed.push(value), 100);
      throttled(1);
      jest.advanceTimersByTime(50);
      throttled(2);
      throttled(3);
      expect(flushed).toEqual([]);
      jest.advanceTimersByTime(50);
      expect(flushed).toEqual([3]);
      jest.advanceTimersByTime(500);
      expect(flushed).toEqual([3]);
      throttled(4);
      jest.advanceTimersByTime(100);
      expect(flushed).toEqual([3, 4]);
    } finally {
      jest.useRealTimers();
    }
  });

  test("cancel drops the pending flush", () => {
    jest.useFakeTimers();
    try {
      const flushed: string[] = [];
      const throttled = throttleTrailing((value: string) => flushed.push(value), 100);
      throttled("only");
      throttled.cancel();
      jest.advanceTimersByTime(1000);
      expect(flushed).toEqual([]);
    } finally {
      jest.useRealTimers();
    }
  });
});
