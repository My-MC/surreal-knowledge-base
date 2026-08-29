import "./testApiMock";

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { act, cleanup, render, screen, within } from "@testing-library/react";

import { fakeClient } from "./testApiMock";

// Dynamic import: a static one would link the real @skb/api-client graph
// before testApiMock's mock.module registration takes effect.
const { PostDetail } = await import("./PostDetail");

const DOC_ID = "document:blog1";
const OTHER_ID = "document:blog2";
const UNPUBLISHED_ID = "document:draft";
const TITLE = "SurrealDB 入門";
const CONTENT = "# SurrealDB\n\nSurrealDB はマルチモデルデータベースです。";

const docFixture = {
  id: DOC_ID,
  title: TITLE,
  content: CONTENT,
  created_at: "2026-08-28T10:00:00Z",
  sha256: "deadbeef",
  source: "inline",
  source_type: "text",
};

const postsFixture = [
  {
    document_id: DOC_ID,
    title: TITLE,
    created_at: "2026-08-28T10:00:00Z",
    author: "qa@example.com",
  },
  {
    document_id: OTHER_ID,
    title: "SurrealDB グラフ機能",
    created_at: "2026-08-27T09:00:00Z",
    author: "qa@example.com",
  },
];

const graphFixture = {
  nodes: [
    { id: DOC_ID, name: TITLE, kind: "document", depth: 0 },
    { id: "entity:⟨SurrealDB⟩", name: "SurrealDB", kind: "section", depth: 1 },
  ],
  edges: [{ from: DOC_ID, to: "entity:⟨SurrealDB⟩", relation: "mentions" }],
};

const backlinksFixture = {
  documents: [
    { id: OTHER_ID, title: "SurrealDB グラフ機能" },
    { id: UNPUBLISHED_ID, title: "未公開ドラフト" },
  ],
};

const hit = (document_id: string, title: string) => ({
  document_id,
  title,
  chunk_idx: 0,
  content: "chunk text",
  score: 0.5,
});

const ok = (data: unknown) => ({ data, error: undefined, response: { status: 200 } });

/**
 * Deterministic flush: TanStack Query v5 schedules notifications via
 * setTimeout(0) — a macrotask microtask flushes never see — so await a real
 * timer inside act's scope.
 */
const flush = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });

/**
 * Route every endpoint by path. Unexpected paths throw so a wiring mistake
 * surfaces as the query error state instead of a silently green assertion.
 */
function mockDetailApi(hits: ReturnType<typeof hit>[], doc = docFixture) {
  fakeClient.GET.mockImplementation(async (path: string) => {
    if (path === "/api/documents/{id}") {
      return ok(doc);
    }
    if (path === "/api/documents/{id}/backlinks") {
      return ok(backlinksFixture);
    }
    if (path === "/api/blog/posts") {
      return ok(postsFixture);
    }
    throw new Error(`unexpected GET ${path}`);
  });
  fakeClient.POST.mockImplementation(async (path: string) => {
    if (path === "/api/graph/query") {
      return ok(graphFixture);
    }
    if (path === "/api/search") {
      return ok({ hits, mode: "vector", elapsed_ms: 1 });
    }
    throw new Error(`unexpected POST ${path}`);
  });
}

async function renderDetail() {
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const postRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/post/$id",
    component: PostDetail,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([postRoute]),
    history: createMemoryHistory({
      initialEntries: [`/post/${encodeURIComponent(DOC_ID)}`],
    }),
  });
  await router.load();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  // Two flushes: the document query settles first, and RecommendedPosts only
  // mounts (and fetches) once the document render lands.
  await flush();
  await flush();
}

describe("PostDetail", () => {
  beforeEach(() => {
    fakeClient.GET.mockReset();
    fakeClient.PUT.mockReset();
    fakeClient.POST.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  test("renders the post content and the related/recommended sections", async () => {
    mockDetailApi([hit(DOC_ID, TITLE), hit(OTHER_ID, "SurrealDB グラフ機能")]);
    await renderDetail();

    expect(screen.getByRole("heading", { name: TITLE })).toBeTruthy();
    // MarkdownView renders async (shiki processor); the text is present in
    // both the fallback and the parsed state, so assert on the article text.
    const article = screen.getByRole("article");
    expect(article.textContent).toContain("SurrealDB はマルチモデルデータベースです。");

    const related = within(screen.getByTestId("related"));
    expect(related.getByTestId("related-entities").textContent).toContain("SurrealDB");
    // The unpublished backlink is filtered out; only the published post stays.
    expect(related.getByRole("link", { name: "SurrealDB グラフ機能" })).toBeTruthy();
    expect(related.queryByRole("link", { name: "未公開ドラフト" })).toBeNull();

    const recommended = within(screen.getByTestId("recommended"));
    expect(recommended.getByRole("link", { name: "SurrealDB グラフ機能" })).toBeTruthy();
    expect(recommended.queryByRole("link", { name: TITLE })).toBeNull();
  });

  test("fetches graph/query depth 1 from the document id and vector search top_k 6", async () => {
    mockDetailApi([hit(OTHER_ID, "SurrealDB グラフ機能")]);
    await renderDetail();

    expect(fakeClient.POST).toHaveBeenCalledWith("/api/graph/query", {
      body: { from: DOC_ID, depth: 1 },
    });
    expect(fakeClient.POST).toHaveBeenCalledWith("/api/search", {
      body: { query: TITLE, mode: "vector", top_k: 6 },
    });
    expect(fakeClient.GET).toHaveBeenCalledWith("/api/documents/{id}", {
      params: { path: { id: DOC_ID, include_chunks: null } },
    });
    expect(fakeClient.GET).toHaveBeenCalledWith("/api/documents/{id}/backlinks", {
      params: { path: { id: DOC_ID } },
    });
    expect(fakeClient.GET).toHaveBeenCalledWith("/api/blog/posts");
  });

  test("seeds the vector search from the content head when the title is empty", async () => {
    mockDetailApi([], { ...docFixture, title: "" });
    await renderDetail();

    expect(fakeClient.POST).toHaveBeenCalledWith("/api/search", {
      body: { query: CONTENT.slice(0, 200), mode: "vector", top_k: 6 },
    });
  });

  test("caps recommended at 5 hits", async () => {
    mockDetailApi([
      hit("document:o1", "記事1"),
      hit("document:o2", "記事2"),
      hit("document:o3", "記事3"),
      hit("document:o4", "記事4"),
      hit("document:o5", "記事5"),
      hit("document:o6", "記事6"),
    ]);
    await renderDetail();

    const recommended = within(screen.getByTestId("recommended"));
    expect(recommended.getAllByRole("link")).toHaveLength(5);
    expect(recommended.getByRole("link", { name: "記事5" })).toBeTruthy();
    expect(recommended.queryByRole("link", { name: "記事6" })).toBeNull();
  });

  test("shows the empty states when no entities and no other hits exist", async () => {
    fakeClient.GET.mockImplementation(async (path: string) => {
      if (path === "/api/documents/{id}") {
        return ok(docFixture);
      }
      if (path === "/api/documents/{id}/backlinks") {
        return ok({ documents: [] });
      }
      if (path === "/api/blog/posts") {
        return ok([postsFixture[0]]);
      }
      throw new Error(`unexpected GET ${path}`);
    });
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/graph/query") {
        return ok({
          nodes: [{ id: DOC_ID, name: TITLE, kind: "document", depth: 0 }],
          edges: [],
        });
      }
      if (path === "/api/search") {
        return ok({ hits: [hit(DOC_ID, TITLE)], mode: "vector", elapsed_ms: 1 });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderDetail();

    expect(screen.getByTestId("related-empty").textContent).toBe("関連記事はありません。");
    expect(screen.queryByTestId("related-entities")).toBeNull();
    expect(screen.getByTestId("recommended-empty").textContent).toBe(
      "おすすめの記事はありません。",
    );
  });
});
