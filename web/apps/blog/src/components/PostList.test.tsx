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
import { act, cleanup, render, screen } from "@testing-library/react";

import { fakeClient } from "./testApiMock";

// Dynamic import: a static one would link the real @skb/api-client graph
// before testApiMock's mock.module registration takes effect.
const { PostList } = await import("./PostList");

const POSTS = [
  {
    document_id: "document:aaa",
    title: "最初の記事",
    created_at: "2026-08-28T10:00:00Z",
    author: "qa@example.com",
  },
  {
    document_id: "document:bbb",
    title: "二番目の記事",
    created_at: "2026-08-27T09:00:00Z",
    author: "other@example.com",
  },
];

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

function renderList() {
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: PostList,
  });
  // <Link to="/post/$id"> resolves against the router's route tree, so the
  // target route must exist even though this suite never visits it.
  const postRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/post/$id",
    component: () => null,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, postRoute]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("PostList", () => {
  beforeEach(() => {
    fakeClient.GET.mockReset();
    fakeClient.PUT.mockReset();
    fakeClient.POST.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  test("renders published posts with title, date, and author", async () => {
    fakeClient.GET.mockImplementation(async () => ok(POSTS));
    renderList();
    await flush();

    expect(fakeClient.GET).toHaveBeenCalledWith("/api/blog/posts");
    expect(screen.getByRole("link", { name: "最初の記事" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "二番目の記事" })).toBeTruthy();
    expect(screen.getByText("qa@example.com")).toBeTruthy();
    expect(screen.getByText("other@example.com")).toBeTruthy();
    expect(screen.getByText("2026-08-28")).toBeTruthy();
    expect(screen.getByText("2026-08-27")).toBeTruthy();
  });

  test("shows the empty state when nothing is published", async () => {
    fakeClient.GET.mockImplementation(async () => ok([]));
    renderList();
    await flush();

    expect(fakeClient.GET).toHaveBeenCalledWith("/api/blog/posts");
    expect(screen.getByTestId("posts-empty").textContent).toBe("公開中の記事はありません。");
    expect(screen.queryByTestId("post-list")).toBeNull();
  });
});
