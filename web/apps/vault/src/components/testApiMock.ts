import { mock } from "bun:test";

/**
 * Replaces @skb/api-client's createClient BEFORE src/api.ts evaluates (this
 * module must be statically imported first). api.ts then builds its typed
 * wrapper around this spyable fake, so toApiError/query wrappers stay real
 * and only the transport is faked.
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
};

mock.module("@skb/api-client", () => ({
  createClient: () => fakeClient,
}));
