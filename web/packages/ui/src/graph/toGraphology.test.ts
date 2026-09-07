import { describe, expect, test } from "bun:test";
import { type GraphQueryResultDto, toGraphology } from "./toGraphology";

function baseResult(): GraphQueryResultDto {
  return {
    nodes: [
      { id: "document:doc-1", name: "Doc 1", kind: "document", depth: 0 },
      { id: "entity:Foo", name: "Foo", kind: "reference", depth: 1 },
      { id: "entity:Bar", name: "Bar", kind: "section", depth: 1 },
    ],
    edges: [
      { from: "document:doc-1", to: "entity:Foo", relation: "mentions" },
      { from: "document:doc-1", to: "entity:Bar", relation: "mentions" },
    ],
  };
}

describe("toGraphology", () => {
  test("converts nodes and edges into a directed graphology graph", () => {
    const graph = toGraphology(baseResult());
    expect(graph.order).toBe(3);
    expect(graph.size).toBe(2);
    expect(graph.type).toBe("directed");
    expect(graph.getNodeAttribute("document:doc-1", "label")).toBe("Doc 1");
    expect(graph.getNodeAttribute("entity:Foo", "kind")).toBe("reference");
    expect(graph.hasEdge("document:doc-1", "entity:Foo")).toBe(true);
    expect(graph.getEdgeAttribute("document:doc-1", "entity:Foo", "relation")).toBe("mentions");
  });

  test("lays nodes out on a circle with finite coordinates", () => {
    const graph = toGraphology(baseResult());
    graph.forEachNode((_, attrs) => {
      expect(Number.isFinite(attrs.x)).toBe(true);
      expect(Number.isFinite(attrs.y)).toBe(true);
      expect(attrs.size).toBeGreaterThan(0);
    });
  });

  test("dedupes duplicate nodes silently (first wins)", () => {
    const graph = toGraphology({
      nodes: [
        { id: "entity:Foo", name: "Foo", kind: "reference", depth: 1 },
        { id: "entity:Foo", name: "Foo renamed", kind: "section", depth: 2 },
      ],
      edges: [],
    });
    expect(graph.order).toBe(1);
    expect(graph.getNodeAttribute("entity:Foo", "label")).toBe("Foo");
    expect(graph.getNodeAttribute("entity:Foo", "kind")).toBe("reference");
  });

  test("dedupes duplicate edges silently", () => {
    const graph = toGraphology({
      nodes: [
        { id: "a", name: "A", kind: "document", depth: 0 },
        { id: "b", name: "B", kind: "reference", depth: 1 },
      ],
      edges: [
        { from: "a", to: "b", relation: "mentions" },
        { from: "a", to: "b", relation: "mentions" },
      ],
    });
    expect(graph.size).toBe(1);
  });

  test("returns an empty graph for empty input", () => {
    const graph = toGraphology({ nodes: [], edges: [] });
    expect(graph.order).toBe(0);
    expect(graph.size).toBe(0);
  });

  test("skips rows with missing or empty fields", () => {
    const graph = toGraphology({
      nodes: [
        { id: "", name: "No id", kind: "document", depth: 0 },
        { id: "ok", name: "Ok", kind: "document", depth: 0 },
        { id: "no-name", name: "", kind: "document", depth: 0 },
        { id: "no-kind", name: "No kind", kind: "", depth: 0 },
        {
          id: "no-depth",
          name: "No depth",
          kind: "document",
          depth: undefined as unknown as number,
        },
      ],
      edges: [
        { from: "ok", to: "ghost", relation: "mentions" }, // unknown endpoint → skipped
        { from: "ok", to: "ok", relation: "" }, // empty relation → skipped
        { from: "ok", to: "ok", relation: "self" }, // self-loop kept
      ],
    });
    expect(graph.order).toBe(1);
    expect(graph.hasNode("ok")).toBe(true);
    expect(graph.size).toBe(1);
    expect(graph.hasEdge("ok", "ok")).toBe(true);
  });

  test("tolerates null/undefined node and edge arrays", () => {
    const graph = toGraphology({
      nodes: undefined as unknown as [],
      edges: undefined as unknown as [],
    });
    expect(graph.order).toBe(0);
    expect(graph.size).toBe(0);
  });
});
