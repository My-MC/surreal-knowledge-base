import { mock } from "bun:test";
import { createElement } from "react";

/**
 * Replaces openapi-fetch's createClient BEFORE src/api.ts evaluates (this
 * module must be statically imported first). api.ts builds its typed wrapper
 * around this spyable fake, so toApiError/query wrappers stay real and only
 * the transport is faked. The paths type import from @skb/api-client is
 * type-only and needs no mock.
 */
export type FakeResponse = {
  data?: unknown;
  error?: unknown;
  response: { status: number };
};

export type FakeMethod = (path: string, init?: unknown) => Promise<FakeResponse>;

export const fakeClient = {
  GET: mock<FakeMethod>(),
  PUT: mock<FakeMethod>(),
  POST: mock<FakeMethod>(),
  DELETE: mock<FakeMethod>(),
};

// bun test runs all suites in one process and mock.module is process-global:
// api-client's client.test.ts executes against this fake after the blog
// suites, so the fake must expose the real client's full surface (named
// createClient plus DELETE) for that surface assertion to stay meaningful.
mock.module("openapi-fetch", () => ({
  default: () => fakeClient,
  createClient: () => fakeClient,
}));

/**
 * MarkdownView is mocked rather than real: the shiki pipeline is built
 * asynchronously into a module-global singleton, and bun test runs every
 * suite in one process — a build triggered here would resolve the singleton
 * before packages/ui's own "fallback before the processor resolves" test
 * (alphabetically later) and break its declaration-order assumption. The
 * real pipeline is covered by packages/ui's tests and the QA screenshots.
 */
mock.module("@skb/ui", () => ({
  MarkdownView: (props: { content: string }) =>
    createElement("div", { "data-testid": "markdown-view" }, props.content),
}));
