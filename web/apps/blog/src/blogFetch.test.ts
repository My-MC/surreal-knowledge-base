import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { useAuthStore } from "./auth";
import { blogFetch } from "./blogFetch";

/**
 * The wrapper is exercised against a stubbed globalThis.fetch (resolved at
 * call time), so no HTTP client is involved. URLs are absolute because
 * happy-dom's location is about:blank and would reject relative ones.
 */
const realFetch = globalThis.fetch;

function stubFetch(
  respond: () => Response,
  calls: { input: RequestInfo | URL; init: RequestInit }[],
) {
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ input, init: init ?? {} });
    return respond();
  }) as typeof fetch;
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

describe("blogFetch", () => {
  beforeEach(() => {
    useAuthStore.setState({ email: "qa@example.com", role: "author" });
  });

  afterEach(() => {
    globalThis.fetch = realFetch;
    useAuthStore.setState({ email: null, role: null });
  });

  test("sends credentials: include on every request", async () => {
    const calls: { input: RequestInfo | URL; init: RequestInit }[] = [];
    stubFetch(() => json(200, []), calls);

    const response = await blogFetch("http://blog.test/api/blog/posts");

    expect(response.status).toBe(200);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.init.credentials).toBe("include");
  });

  test("clears the stored identity on a 401 response", async () => {
    stubFetch(() => json(401, { code: "E_VALIDATION", message: "author role required" }), []);

    const response = await blogFetch("http://blog.test/api/documents", { method: "POST" });

    expect(response.status).toBe(401);
    expect(useAuthStore.getState().email).toBeNull();
    expect(useAuthStore.getState().role).toBeNull();
  });

  test("leaves the stored identity untouched on non-401 responses", async () => {
    stubFetch(() => json(200, []), []);

    await blogFetch("http://blog.test/api/blog/posts");

    expect(useAuthStore.getState().email).toBe("qa@example.com");
    expect(useAuthStore.getState().role).toBe("author");
  });
});
