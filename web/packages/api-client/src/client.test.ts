import { describe, expect, it } from "bun:test";
import { createClient } from "./client";

describe("createClient", () => {
  it("returns an openapi-fetch client with the full method surface", () => {
    const client = createClient("http://127.0.0.1:18080");
    expect(typeof client.GET).toBe("function");
    expect(typeof client.POST).toBe("function");
    expect(typeof client.PUT).toBe("function");
    expect(typeof client.DELETE).toBe("function");
  });
});
