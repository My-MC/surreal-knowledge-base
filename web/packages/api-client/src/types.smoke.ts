// Type-level smoke for the generated OpenAPI schema (plan todo 10). No runtime
// code: `bun run typecheck` fails if a route, method, or schema is renamed.
import type { components, paths } from "./schema.gen";

export type HealthResponse =
  paths["/api/health"]["get"]["responses"][200]["content"]["application/json"];
export type SearchRequestBody =
  paths["/api/search"]["post"]["requestBody"]["content"]["application/json"];
export type ChatStreamRequestBody =
  paths["/api/chat/stream"]["post"]["requestBody"]["content"]["application/json"];
export type SearchHit = components["schemas"]["SearchHit"];
export type ChatStreamRequest = components["schemas"]["ChatStreamRequest"];
