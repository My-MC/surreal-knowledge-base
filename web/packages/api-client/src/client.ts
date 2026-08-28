import createOpenApiFetchClient from "openapi-fetch";
import type { components, paths } from "./schema.gen";

export type { components, paths };

export function createClient(baseUrl: string) {
  return createOpenApiFetchClient<paths>({ baseUrl });
}

export type ApiClient = ReturnType<typeof createClient>;
