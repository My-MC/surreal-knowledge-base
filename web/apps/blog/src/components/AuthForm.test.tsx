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
const { AuthForm } = await import("./AuthForm");

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

/**
 * Deterministic flush: TanStack Query v5 schedules notifications via
 * setTimeout(0) — a macrotask microtask flushes never see — so await a real
 * timer inside act's scope.
 */
const flush = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });

async function renderAuth(mode: "login" | "register") {
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <p data-testid="home">home</p>,
  });
  const loginRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/login",
    component: () => <AuthForm mode="login" />,
  });
  const registerRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/register",
    component: () => <AuthForm mode="register" />,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, loginRoute, registerRoute]),
    history: createMemoryHistory({ initialEntries: [mode === "login" ? "/login" : "/register"] }),
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

function fillCredentials(email: string, password: string) {
  fireEvent.change(screen.getByTestId("auth-email"), { target: { value: email } });
  fireEvent.change(screen.getByTestId("auth-password"), { target: { value: password } });
}

describe("AuthForm", () => {
  beforeEach(() => {
    fakeClient.GET.mockReset();
    fakeClient.PUT.mockReset();
    fakeClient.POST.mockReset();
    useAuthStore.setState({ email: null, role: null });
  });

  afterEach(() => {
    cleanup();
  });

  test("login stores the echoed identity and navigates to /", async () => {
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/auth/login") {
        return ok({ email: "qa@example.com", role: "author" });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderAuth("login");
    fillCredentials("qa@example.com", "secret");
    fireEvent.click(screen.getByTestId("auth-submit"));
    await flush();

    expect(fakeClient.POST).toHaveBeenCalledWith("/api/auth/login", {
      body: { email: "qa@example.com", password: "secret" },
    });
    expect(useAuthStore.getState().email).toBe("qa@example.com");
    expect(useAuthStore.getState().role).toBe("author");
    expect(screen.getByTestId("home")).toBeTruthy();
  });

  test("login shows the server message inline on 401", async () => {
    fakeClient.POST.mockImplementation(async (path: string) => {
      if (path === "/api/auth/login") {
        return fail(401, { code: "E_VALIDATION", message: "invalid email or password" });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderAuth("login");
    fillCredentials("qa@example.com", "wrong");
    fireEvent.click(screen.getByTestId("auth-submit"));
    await flush();

    expect(screen.getByTestId("auth-error").textContent).toBe("invalid email or password");
    expect(useAuthStore.getState().email).toBeNull();
    expect(screen.queryByTestId("home")).toBeNull();
  });

  test("register auto-logins: register then login, then lands on /", async () => {
    const calls: string[] = [];
    fakeClient.POST.mockImplementation(async (path: string) => {
      calls.push(path);
      if (path === "/api/auth/register") {
        return ok({ email: "new@example.com", role: "author" }, 201);
      }
      if (path === "/api/auth/login") {
        return ok({ email: "new@example.com", role: "author" });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderAuth("register");
    fillCredentials("new@example.com", "secret");
    fireEvent.click(screen.getByTestId("auth-submit"));
    await flush();

    expect(calls).toEqual(["/api/auth/register", "/api/auth/login"]);
    expect(fakeClient.POST).toHaveBeenCalledWith("/api/auth/register", {
      body: { email: "new@example.com", password: "secret" },
    });
    expect(useAuthStore.getState().role).toBe("author");
    expect(screen.getByTestId("home")).toBeTruthy();
  });

  test("register shows a 409 duplicate inline and skips auto-login", async () => {
    const calls: string[] = [];
    fakeClient.POST.mockImplementation(async (path: string) => {
      calls.push(path);
      if (path === "/api/auth/register") {
        return fail(409, { code: "E_CONFLICT", message: "email already registered" });
      }
      throw new Error(`unexpected POST ${path}`);
    });
    await renderAuth("register");
    fillCredentials("taken@example.com", "secret");
    fireEvent.click(screen.getByTestId("auth-submit"));
    await flush();

    expect(calls).toEqual(["/api/auth/register"]);
    expect(screen.getByTestId("auth-error").textContent).toBe("email already registered");
    expect(useAuthStore.getState().email).toBeNull();
  });
});
