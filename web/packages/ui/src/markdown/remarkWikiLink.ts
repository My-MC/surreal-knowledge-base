import type { Element, ElementContent } from "hast";
import type { Data, Node, Nodes, Root, Text } from "mdast";
import { visit } from "unist-util-visit";

/**
 * Wikilink syntax: `[[Target]]`. The target is the literal text between the
 * double brackets — no nested markdown is parsed inside it.
 */
const WIKI_LINK = /\[\[([^\][\n]+)\]\]/g;

export interface WikiLinkData extends Data {
  target: string;
}

export interface WikiLinkNode extends Node {
  type: "wikiLink";
  data: WikiLinkData;
}

// Register the custom node type so mdast unions (and therefore parent
// `children` arrays and remark-rehype's `Handlers` key set) accept it.
declare module "mdast" {
  interface RootContentMap {
    wikiLink: WikiLinkNode;
  }
  interface PhrasingContentMap {
    wikiLink: WikiLinkNode;
  }
}

/**
 * remark plugin: splits `[[Target]]` occurrences inside text nodes into
 * `wikiLink` nodes carrying `data.target`. Code blocks and inline code are
 * untouched (they are not `text` nodes).
 */
export function remarkWikiLink() {
  return (tree: Root) => {
    visit(tree, "text", (node, index, parent) => {
      if (typeof index !== "number" || !parent) return;
      const value = node.value;
      WIKI_LINK.lastIndex = 0;
      const matches = [...value.matchAll(WIKI_LINK)];
      if (matches.length === 0) return;

      const replacement: Array<Text | WikiLinkNode> = [];
      let cursor = 0;
      for (const match of matches) {
        const start = match.index;
        const target = match[1];
        if (start === undefined || target === undefined) continue;
        if (start > cursor) {
          replacement.push({ type: "text", value: value.slice(cursor, start) });
        }
        replacement.push({ type: "wikiLink", data: { target } });
        cursor = start + match[0].length;
      }
      if (cursor < value.length) {
        replacement.push({ type: "text", value: value.slice(cursor) });
      }

      parent.children.splice(index, 1, ...replacement);
      return index + replacement.length;
    });
  };
}

/**
 * remark-rehype handler: converts a `wikiLink` node into a placeholder
 * `<a data-wikilink="target">target</a>`. No href is emitted — the consuming
 * app resolves document links (via a components override or its router).
 */
export function wikiLinkHandler(_state: unknown, node: Nodes): Element | undefined {
  if (node.type !== "wikiLink") return undefined;
  const target = node.data.target;
  const child: ElementContent = { type: "text", value: target };
  return {
    type: "element",
    tagName: "a",
    properties: { dataWikilink: target },
    children: [child],
  };
}
