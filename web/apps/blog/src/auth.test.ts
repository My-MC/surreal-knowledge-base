import { beforeEach, describe, expect, test } from "bun:test";
import { createAuthStore } from "./auth";

describe("auth store", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  test("starts logged out on fresh storage", () => {
    const store = createAuthStore();
    expect(store.getState().email).toBeNull();
    expect(store.getState().role).toBeNull();
  });

  test("persists setAuth across store instances", () => {
    const first = createAuthStore();
    first.getState().setAuth("qa@example.com", "author");

    const second = createAuthStore();
    expect(second.getState().email).toBe("qa@example.com");
    expect(second.getState().role).toBe("author");
  });

  test("clearAuth resets the identity and persists the reset", () => {
    const first = createAuthStore();
    first.getState().setAuth("qa@example.com", "author");
    first.getState().clearAuth();

    const second = createAuthStore();
    expect(second.getState().email).toBeNull();
    expect(second.getState().role).toBeNull();
  });
});
