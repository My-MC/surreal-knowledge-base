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
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import { useAuthStore } from "../auth";
import { fakeClient } from "./testApiMock";

// Dynamic import: a static one would link the real openapi-fetch graph
// before testApiMock's mock.module registration takes effect.
const { NewPost } = await import("./NewPost");

const ok = (data: unknown, status = 200) => ({
  data,
  error: undefined,
  response: { status },
});
const fail = (status: number, body: unknown) => ({
  data: undefined,
  error: body,
  response: { status },
});

const flush = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });

async function renderNewPost() {
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <p data-testid="home">home</p>,
  });
  const newRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/new",
    component: NewPost,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, newRoute]),
    history: createMemoryHistory({ initialEntries: ["/new"] }),
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
}

describe("NewPost", () => {
  beforeEach(() => {
    fakeClient.GET.mockReset();
    fakeClient.PUT.mockReset();
    fakeClient.POST.mockReset();
    useAuthStore.setState({ email: "qa@example.com", role: "author" });
  });

  afterEach(() => {
    cleanup();
  });

  test("submit ingests with app=blog metadata and shows the 投稿完了 state", async () => {
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/documents") {
        return ok(
          {
            document_id: "document:abc",
            status: "created",
            title: "タイトル",
            sha256: "beef",
            chunks: 1,
            tokens: 10,
            entities: [],
          },
          201,
        );
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderNewPost();
    fireEvent.change(screen.getByTestId("new-title"), { target: { value: "タイトル" } });
    fireEvent.change(screen.getByTestId("new-content"), {
      target: { value: "# 本文\n\n[[リンク]]" },
    });
    fireEvent.click(screen.getByTestId("new-submit"));
    await flush();

    expect(fakeClient.POST).toHaveBeenCalledWith("/api/documents", {
      body: { title: "タイトル", content: "# 本文\n\n[[リンク]]", metadata: { app: "blog" } },
    });
    expect(screen.getByTestId("new-success")).toBeTruthy();
    expect(screen.getByTestId("new-publish")).toBeTruthy();
  });

  test("公開する publishes and navigates to /", async () => {
    fakeClient.POST.mockImplementation(async (path: string, init?: unknown) => {
      if (path === "/api/documents") {
        return ok({ document_id: "document:abc", status: "created" }, 201);
      }
      if (path === "/api/blog/posts/{document_id}/publish") {
        expect(init).toEqual({ params: { path: { document_id: "document:abc" } } });
        return ok({ document_id: "document:abc", published: true });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderNewPost();
    fireEvent.change(screen.getByTestId("new-title"), { target: { value: "タイトル" } });
    fireEvent.change(screen.getByTestId("new-content"), { target: { value: "本文" } });
    fireEvent.click(screen.getByTestId("new-submit"));
    await flush();
    fireEvent.click(screen.getByTestId("new-publish"));
    await flush();

    expect(screen.getByTestId("home")).toBeTruthy();
  });

  test("a sha256 skip (null document_id) surfaces as an inline error", async () => {
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/documents") {
        return ok({ document_id: null, status: "skipped" }, 201);
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderNewPost();
    fireEvent.change(screen.getByTestId("new-title"), { target: { value: "タイトル" } });
    fireEvent.change(screen.getByTestId("new-content"), { target: { value: "本文" } });
    fireEvent.click(screen.getByTestId("new-submit"));
    await flush();

    expect(screen.getByTestId("new-error").textContent).toContain("既に存在");
    expect(screen.queryByTestId("new-success")).toBeNull();
  });

  test("an upload 401 shows the server message inline", async () => {
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/documents") {
        return fail(401, { code: "E_VALIDATION", message: "author role required" });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderNewPost();
    fireEvent.change(screen.getByTestId("new-title"), { target: { value: "タイトル" } });
    fireEvent.change(screen.getByTestId("new-content"), { target: { value: "本文" } });
    fireEvent.click(screen.getByTestId("new-submit"));
    await flush();

    expect(screen.getByTestId("new-error").textContent).toBe("author role required");
  });
});
