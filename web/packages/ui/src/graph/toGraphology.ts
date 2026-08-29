import Graph from "graphology";

/** Node of the `POST /api/graph/query` result graph (server DTO, dto/graph.rs). */
export interface GraphNodeDto {
  id: string;
  name: string;
  kind: string;
  depth: number;
}

/** Edge of the `POST /api/graph/query` result graph (server DTO, dto/graph.rs). */
export interface GraphEdgeDto {
  from: string;
  to: string;
  relation: string;
}

/** Body of the `POST /api/graph/query` response (server DTO, dto/graph.rs). */
export interface GraphQueryResultDto {
  nodes: GraphNodeDto[];
  edges: GraphEdgeDto[];
}

export interface GraphNodeAttrs {
  label: string;
  kind: string;
  depth: number;
  x: number;
  y: number;
  size: number;
  color?: string;
}

export interface GraphEdgeAttrs {
  relation: string;
}

/** Graphology graph ready for sigma rendering (nodes laid out on a circle). */
export type KnowledgeGraph = Graph<GraphNodeAttrs, GraphEdgeAttrs>;

const NODE_SIZE = 8;

function isValidNode(node: unknown): boolean {
  if (typeof node !== "object" || node === null) return false;
  const candidate = node as Record<string, unknown>;
  return (
    typeof candidate.id === "string" &&
    candidate.id !== "" &&
    typeof candidate.name === "string" &&
    candidate.name !== "" &&
    typeof candidate.kind === "string" &&
    candidate.kind !== "" &&
    typeof candidate.depth === "number"
  );
}

function isValidEdge(edge: unknown): boolean {
  if (typeof edge !== "object" || edge === null) return false;
  const candidate = edge as Record<string, unknown>;
  return (
    typeof candidate.from === "string" &&
    candidate.from !== "" &&
    typeof candidate.to === "string" &&
    candidate.to !== "" &&
    typeof candidate.relation === "string" &&
    candidate.relation !== ""
  );
}

/**
 * Convert a `POST /api/graph/query` result into a directed graphology graph.
 *
 * Tolerant by contract: duplicate nodes/edges are deduped silently (first
 * wins), empty input yields an empty graph, and rows with missing or empty
 * fields are skipped. Edges referencing nodes absent from `nodes` are also
 * skipped — the server guarantees endpoints, and auto-creating them would
 * render invisible nodes. Nodes are placed on a unit circle (deterministic;
 * no layout dependency).
 */
export function toGraphology(result: GraphQueryResultDto): KnowledgeGraph {
  const graph = new Graph<GraphNodeAttrs, GraphEdgeAttrs>({
    multi: false,
    type: "directed",
    allowSelfLoops: true,
  });

  const nodes = (Array.isArray(result.nodes) ? result.nodes : []).filter(isValidNode);
  const total = nodes.length;
  nodes.forEach((node, index) => {
    if (graph.hasNode(node.id)) return;
    const angle = total === 1 ? 0 : (2 * Math.PI * index) / total;
    graph.addNode(node.id, {
      label: node.name,
      kind: node.kind,
      depth: node.depth,
      x: Math.cos(angle),
      y: Math.sin(angle),
      size: NODE_SIZE,
    });
  });

  const edges = Array.isArray(result.edges) ? result.edges : [];
  for (const edge of edges) {
    if (!isValidEdge(edge)) continue;
    if (!graph.hasNode(edge.from) || !graph.hasNode(edge.to)) continue;
    if (graph.hasEdge(edge.from, edge.to)) continue;
    graph.addEdge(edge.from, edge.to, { relation: edge.relation });
  }

  return graph;
}
