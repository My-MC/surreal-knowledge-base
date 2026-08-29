import { createClient } from "@skb/api-client";
import { queryOptions } from "@tanstack/react-query";

/**
 * Typed OpenAPI client. The empty baseUrl keeps request URLs relative so the
 * Vite dev proxy (vite.config.ts, SKB_SERVER_PORT-driven) owns routing to
 * skb-server. Every network call in this app goes through this client.
 */
export const api = createClient("");

/** Error carrying the server's machine-readable code (ErrorResponse). */
export class ApiError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

/**
 * Normalize an openapi-fetch failure into an ApiError. Typed server errors
 * arrive as an ErrorResponse object, but a dead upstream (the dev proxy
 * answers 502 with an empty text body) arrives as an empty string — map every
 * shape to something displayable instead of "undefined".
 */
export function toApiError(error: unknown, status: number): ApiError {
  if (typeof error === "string") {
    return new ApiError(`E_HTTP_${status}`, error.length > 0 ? error : `HTTP ${status}`);
  }
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    const code =
      "code" in error && typeof error.code === "string" ? error.code : `E_HTTP_${status}`;
    return new ApiError(code, error.message);
  }
  return new ApiError(`E_HTTP_${status}`, `HTTP ${status}`);
}

/**
 * Document summaries for the tree and the "/" latest-document redirect.
 *
 * The generated schema types the list query params under `parameters.path`
 * (utoipa emits them without an explicit `in`, openapi-typescript maps that to
 * path params), so the type-correct call passes them explicitly as nulls. At
 * runtime openapi-fetch only substitutes path params into `{placeholder}`
 * segments — "/api/documents" has none — so the request is a plain
 * GET /api/documents and the server defaults apply.
 */
export const documentsQuery = () =>
  queryOptions({
    queryKey: ["documents"],
    queryFn: async () => {
      const { data, error, response } = await api.GET("/api/documents", {
        params: {
          path: { limit: null, offset: null, order: null, after: null },
        },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return data;
    },
  });

export const documentQuery = (id: string) =>
  queryOptions({
    queryKey: ["documents", id],
    queryFn: async () => {
      const { data, error, response } = await api.GET("/api/documents/{id}", {
        params: { path: { id, include_chunks: null } },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return data;
    },
  });
