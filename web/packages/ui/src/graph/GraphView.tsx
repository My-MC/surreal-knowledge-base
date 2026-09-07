import { SigmaContainer, useRegisterEvents } from "@react-sigma/core";
import { useEffect, useMemo } from "react";
import { type GraphQueryResultDto, toGraphology } from "./toGraphology";

/** Node kind → tokens.css custom property name (resolved at render time). */
const KIND_COLOR_VARS: Record<string, string> = {
  document: "--color-primary",
  section: "--color-secondary",
  reference: "--color-accent",
};

const DEFAULT_KIND_VAR = "--color-text";
const EDGE_COLOR_VAR = "--color-border";
/**
 * Last-resort color used only when tokens.css is not loaded (component
 * tests); apps always import tokens.css, so the token value wins there.
 */
const FALLBACK_COLOR = "#808080";

function resolveTokenColor(style: CSSStyleDeclaration, name: string): string {
  const value = style.getPropertyValue(name).trim();
  return value === "" ? FALLBACK_COLOR : value;
}

interface GraphEventsProps {
  onNodeClick?: (id: string) => void;
}

function GraphEvents({ onNodeClick }: GraphEventsProps) {
  const registerEvents = useRegisterEvents();
  useEffect(() => {
    registerEvents({
      clickNode: (event) => onNodeClick?.(event.node),
    });
  }, [registerEvents, onNodeClick]);
  return null;
}

export interface GraphViewProps {
  /** Nodes of a `POST /api/graph/query` result. */
  nodes: GraphQueryResultDto["nodes"];
  /** Edges of a `POST /api/graph/query` result. */
  edges: GraphQueryResultDto["edges"];
  /** Called with the node id on click (navigation is the app's job). */
  onNodeClick?: (id: string) => void;
}

/**
 * WebGL knowledge-graph viewer (sigma.js v3 via @react-sigma/core v5).
 *
 * Apps must load tokens.css and `@skb/ui/src/graph/graph.css` themselves —
 * CSS is intentionally not imported from the TSX (bun test safety, T9/T11
 * convention). Node colors resolve tokens.css custom properties at render
 * time because WebGL cannot read CSS variables directly. The graph instance
 * is memoized on `nodes`/`edges`; parents should pass stable arrays.
 */
export function GraphView({ nodes, edges, onNodeClick }: GraphViewProps) {
  const rootStyle = useMemo(() => getComputedStyle(document.documentElement), []);
  const edgeColor = useMemo(() => resolveTokenColor(rootStyle, EDGE_COLOR_VAR), [rootStyle]);

  const graph = useMemo(() => {
    const built = toGraphology({ nodes, edges });
    built.forEachNode((id) => {
      const kind = built.getNodeAttribute(id, "kind");
      const varName = KIND_COLOR_VARS[kind] ?? DEFAULT_KIND_VAR;
      built.setNodeAttribute(id, "color", resolveTokenColor(rootStyle, varName));
    });
    return built;
  }, [nodes, edges, rootStyle]);

  if (nodes.length === 0) {
    return (
      <div className="skb-graph skb-graph-empty">
        <p className="skb-graph-empty-message">ノードがありません</p>
      </div>
    );
  }

  return (
    <div className="skb-graph">
      <SigmaContainer
        graph={graph}
        settings={{ allowInvalidContainer: true, defaultEdgeColor: edgeColor }}
      >
        <GraphEvents onNodeClick={onNodeClick} />
      </SigmaContainer>
    </div>
  );
}
