import "./components/testApiMock";

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { useAuthStore } from "./auth";

// Dynamic import: a static one would link the real api module before
// testApiMock's openapi-fetch mock takes effect.
const { AppLayout } = await import("./App");
const { fakeClient } = await import("./components/testApiMock");

async function renderShell() {
  const rootRoute = createRootRoute({ component: AppLayout });
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <p data-testid="home">home</p>,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  await router.load();
  render(<RouterProvider router={router} />);
}

const ok = (data: unknown) => ({ data, error: undefined, response: { status: 200 } });

describe("AppLayout", () => {
  beforeEach(() => {
    useAuthStore.setState({ email: null, role: null });
  });

  afterEach(() => {
    cleanup();
    useAuthStore.setState({ email: null, role: null });
  });

  test("hides 新規投稿 from readers and shows it to authors", async () => {
    useAuthStore.setState({ email: "reader@example.com", role: "reader" });
    await renderShell();
    expect(screen.queryByRole("link", { name: "新規投稿" })).toBeNull();
    expect(screen.getByTestId("header-email").textContent).toBe("reader@example.com");

    cleanup();
    useAuthStore.setState({ email: "author@example.com", role: "author" });
    await renderShell();
    expect(screen.getByRole("link", { name: "新規投稿" })).toBeTruthy();
  });

  test("logout revokes the session server-side and then clears the local store", async () => {
    useAuthStore.setState({ email: "author@example.com", role: "author" });
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/auth/logout") {
        return ok(undefined);
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderShell();

    fireEvent.click(screen.getByTestId("logout"));

    await waitFor(() => {
      expect(fakeClient.POST.mock.calls.some(([path]) => path === "/api/auth/logout")).toBe(true);
    });
    await waitFor(() => {
      expect(useAuthStore.getState().email).toBeNull();
      expect(useAuthStore.getState().role).toBeNull();
    });
  });

  test("logout still clears the local store when the server call fails", async () => {
    useAuthStore.setState({ email: "author@example.com", role: "author" });
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/auth/logout") {
        return {
          data: undefined,
          error: { code: "E_HTTP_503", message: "down" },
          response: { status: 503 },
        };
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderShell();

    fireEvent.click(screen.getByTestId("logout"));

    await waitFor(() => {
      expect(useAuthStore.getState().email).toBeNull();
      expect(useAuthStore.getState().role).toBeNull();
    });
  });
});
