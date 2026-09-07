import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import { GraphView } from "./GraphView";
import type { GraphEdgeDto, GraphNodeDto } from "./toGraphology";

// sigma.js asserts a non-null WebGL context (createWebGLContext calls
// gl.blendFunc right after getContext), and happy-dom returns null for every
// getContext call. The smoke tests therefore install a Proxy-based fake GL /
// 2D context: UPPERCASE property reads return GL constants (numbers), known
// status queries return plausible values, everything else is a no-op fn.
function fakeWebGLContext(canvas: HTMLCanvasElement): WebGLRenderingContext {
  return new Proxy(
    { canvas },
    {
      get(target, prop: string | symbol) {
        if (prop === "canvas") return target.canvas;
        if (typeof prop === "string" && /^[A-Z][A-Z0-9_]*$/.test(prop)) return 1;
        if (prop === "getShaderParameter" || prop === "getProgramParameter") return () => true;
        if (prop === "getShaderInfoLog" || prop === "getProgramInfoLog") return () => "";
        if (prop === "getUniformLocation") return () => ({});
        if (prop === "getExtension") return () => null;
        if (prop === "getParameter") return () => 4096;
        if (prop === "getSupportedExtensions") return () => [];
        if (prop === "checkFramebufferStatus" || prop === "getError") return () => 1;
        if (typeof prop === "string" && prop.startsWith("create")) return () => ({});
        return () => undefined;
      },
    },
  ) as unknown as WebGLRenderingContext;
}

function fake2DContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  return new Proxy(
    { canvas },
    {
      get(target, prop: string | symbol) {
        if (prop === "canvas") return target.canvas;
        if (prop === "measureText") return () => ({ width: 10 });
        if (typeof prop === "string" && /^[A-Z][A-Z0-9_]*$/.test(prop)) return 1;
        return () => undefined;
      },
    },
  ) as unknown as CanvasRenderingContext2D;
}

describe("GraphView", () => {
  const originalGetContext = HTMLCanvasElement.prototype.getContext;

  beforeEach(() => {
    HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, contextId: string) {
      if (contextId === "2d") return fake2DContext(this);
      if (contextId === "webgl2" || contextId === "webgl" || contextId === "experimental-webgl") {
        return fakeWebGLContext(this);
      }
      return null;
    } as unknown as typeof HTMLCanvasElement.prototype.getContext;
  });

  afterEach(() => {
    cleanup();
    HTMLCanvasElement.prototype.getContext = originalGetContext;
  });

  const nodes: GraphNodeDto[] = [
    { id: "document:doc-1", name: "Doc 1", kind: "document", depth: 0 },
    { id: "entity:Foo", name: "Foo", kind: "reference", depth: 1 },
  ];
  const edges: GraphEdgeDto[] = [
    { from: "document:doc-1", to: "entity:Foo", relation: "mentions" },
  ];

  test("mounts a populated graph without crashing (WebGL smoke, no pixel asserts)", () => {
    render(<GraphView nodes={nodes} edges={edges} />);
    expect(document.querySelector(".skb-graph")).not.toBeNull();
    expect(document.querySelector(".skb-graph .sigma-container")).not.toBeNull();
  });

  test("renders the empty state for empty nodes/edges without crashing", () => {
    render(<GraphView nodes={[]} edges={[]} />);
    expect(document.querySelector(".skb-graph-empty")).not.toBeNull();
    expect(screen.getByText("ノードがありません")).toBeDefined();
    expect(document.querySelector(".sigma-container")).toBeNull();
  });
});
