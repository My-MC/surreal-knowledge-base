import "./testApiMock";

import { afterEach, beforeEach, describe, expect, jest, test } from "bun:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
  useParams,
} from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { SaveStatusIndicator } from "./SaveStatusIndicator";
import { fakeClient } from "./testApiMock";
import type { SaveInput } from "./useAutosave";

// Dynamic import: a static one would link the real @skb/api-client graph
// before testApiMock's mock.module registration takes effect.
const { useAutosave } = await import("./useAutosave");

const DOC_ID = "document:abc123";
const NEW_ID = "document:def456";
const TITLE = "テスト文書";

let nextEdit: SaveInput = { content: "", title: TITLE };

function AutosaveHarness() {
  const { id } = useParams({ from: "/doc/$id" });
  const { status, schedule, retry } = useAutosave(id);
  const [editCount, setEditCount] = useState(0);
  return (
    <div>
      <p data-testid="route-id">{id}</p>
      <SaveStatusIndicator status={status} onRetry={retry} />
      <button
        type="button"
        data-testid="edit"
        onClick={() => {
          nextEdit = { ...nextEdit, content: `content-${editCount}` };
          setEditCount((count) => count + 1);
          schedule(nextEdit);
        }}
      >
        edit
      </button>
    </div>
  );
}

function createHarness(initialId: string) {
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const docRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/doc/$id",
    component: AutosaveHarness,
  });
  const router = createRouter({
    routeTree: rootRoute.addChildren([docRoute]),
    history: createMemoryHistory({ initialEntries: [`/doc/${initialId}`] }),
  });
  const replaceCalls: string[] = [];
  const pushCalls: string[] = [];
  const originalReplace = router.history.replace.bind(router.history);
  const originalPush = router.history.push.bind(router.history);
  router.history.replace = (path, state) => {
    replaceCalls.push(path);
    originalReplace(path, state);
  };
  router.history.push = (path, state) => {
    pushCalls.push(path);
    originalPush(path, state);
  };
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return { router, queryClient, replaceCalls, pushCalls };
}

const okResponse = (documentId: string) => ({
  data: { document_id: documentId },
  error: undefined,
  response: { status: 200 },
});

const errorResponse = () => ({
  data: undefined,
  error: { code: "E_INTERNAL", message: "boom" },
  response: { status: 500 },
});

async function renderHarness(initialId: string) {
  const harness = createHarness(initialId);
  await harness.router.load();
  render(
    <QueryClientProvider client={harness.queryClient}>
      <RouterProvider router={harness.router} />
    </QueryClientProvider>,
  );
  return harness;
}

const clickEdit = async () => {
  await act(async () => {
    fireEvent.click(screen.getByTestId("edit"));
  });
};

const advance = async (ms: number) => {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
  await act(async () => {});
};

describe("useAutosave", () => {
  beforeEach(() => {
    fakeClient.GET.mockReset();
    fakeClient.PUT.mockReset();
    fakeClient.POST.mockReset();
    nextEdit = { content: "", title: TITLE };
  });

  afterEach(() => {
    // Drain the fake clock before switching back: a task queued on it (e.g. a
    // react scheduler delivery) is dropped by useRealTimers and wedges every
    // later react update in the shared bun test process.
    jest.runOnlyPendingTimers();
    jest.useRealTimers();
    cleanup();
  });

  test("coalesces 3 edits within the debounce window into exactly 1 PUT", async () => {
    fakeClient.PUT.mockResolvedValue(okResponse(DOC_ID));
    await renderHarness(DOC_ID);
    jest.useFakeTimers();

    await clickEdit();
    await clickEdit();
    await clickEdit();
    expect(fakeClient.PUT).not.toHaveBeenCalled();

    await advance(500);
    expect(fakeClient.PUT).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("status").textContent).toMatch(/^保存済み \d{2}:\d{2}$/);
  });

  test("sends the latest content and the unchanged title in the PUT body", async () => {
    fakeClient.PUT.mockResolvedValue(okResponse(DOC_ID));
    await renderHarness(DOC_ID);
    jest.useFakeTimers();

    await clickEdit();
    await clickEdit();
    await advance(500);

    expect(fakeClient.PUT).toHaveBeenCalledWith("/api/documents/{id}", {
      params: { path: { id: DOC_ID } },
      body: { content: "content-1", title: TITLE },
    });
  });

  test("replace-navigates to the new id when the PUT mints one", async () => {
    fakeClient.PUT.mockResolvedValue(okResponse(NEW_ID));
    const { replaceCalls, pushCalls } = await renderHarness(DOC_ID);
    jest.useFakeTimers();

    await clickEdit();
    await advance(500);

    // The router percent-encodes ":" in path params; decode to assert intent.
    expect(replaceCalls.map(decodeURIComponent)).toEqual([`/doc/${NEW_ID}`]);
    expect(pushCalls).toEqual([]);
    expect(screen.getByTestId("route-id").textContent).toBe(NEW_ID);
  });

  test("stays on the route when the PUT response keeps the same id", async () => {
    fakeClient.PUT.mockResolvedValue(okResponse(DOC_ID));
    const { replaceCalls, pushCalls } = await renderHarness(DOC_ID);
    jest.useFakeTimers();

    await clickEdit();
    await advance(500);

    expect(replaceCalls).toEqual([]);
    expect(pushCalls).toEqual([]);
    expect(screen.getByTestId("route-id").textContent).toBe(DOC_ID);
  });

  test("shows the error with a retry button and retry re-fires the PUT", async () => {
    fakeClient.PUT.mockResolvedValueOnce(errorResponse());
    fakeClient.PUT.mockResolvedValueOnce(okResponse(DOC_ID));
    await renderHarness(DOC_ID);
    jest.useFakeTimers();

    await clickEdit();
    await advance(500);

    expect(screen.getByRole("alert").textContent).toContain("boom");
    expect(screen.queryByRole("status")).toBeNull();
    const retryButton = screen.getByRole("button", { name: "再試行" });

    await act(async () => {
      fireEvent.click(retryButton);
    });
    await advance(0);

    expect(fakeClient.PUT).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("status").textContent).toMatch(/^保存済み \d{2}:\d{2}$/);
  });
});
