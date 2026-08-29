import type { paths } from "@skb/api-client";
import { queryOptions } from "@tanstack/react-query";
import createOpenApiFetchClient from "openapi-fetch";
import { blogFetch } from "./blogFetch";

/**
 * Typed OpenAPI client. The empty baseUrl keeps request URLs relative so the
 * Vite dev proxy (vite.config.ts, SKB_SERVER_PORT-driven) owns routing to
 * skb-server. Every network call in this app goes through this client; the
 * credentials/401 behavior lives in the shared blogFetch (see blogFetch.ts).
 */
export const api = createBlogClient("");

export function createBlogClient(baseUrl: string) {
  return createOpenApiFetchClient<paths>({ baseUrl, fetch: blogFetch });
}

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

/** Published blog posts, newest first (public, no auth). */
export const blogPostsQuery = () =>
  queryOptions({
    queryKey: ["blog", "posts"],
    queryFn: async () => {
      const { data, error, response } = await api.GET("/api/blog/posts");
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return data;
    },
  });

/** Full document detail for one post (id is the full `document:<key>` record id). */
export const documentQuery = (id: string) =>
  queryOptions({
    queryKey: ["blog", "document", id],
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

export interface RelatedPosts {
  /** Entity names this post's chunks mention (graph depth-1 nodes). */
  entities: string[];
  /** Published posts sharing at least one entity (self already excluded). */
  posts: { id: string; title: string }[];
}

/**
 * 関連記事 — entities of this post plus other published posts sharing one.
 *
 * The graph result for `from = document:<id>` depth 1 contains ONLY the
 * depth-0 self node plus depth-1 entity nodes: the graph API walks
 * chunk→entity (mentions) and entity→entity (related_to), so it has no
 * entity→document direction and never lists other documents. The
 * entity→documents mapping therefore comes from the backlinks endpoint (the
 * server-side reverse-mentions walk, which already excludes self), and the
 * result is filtered to published posts because backlinks spans every
 * document, published or not.
 */
export const relatedQuery = (id: string) =>
  queryOptions({
    queryKey: ["blog", "related", id],
    queryFn: async (): Promise<RelatedPosts> => {
      const graph = await api.POST("/api/graph/query", { body: { from: id, depth: 1 } });
      if (graph.error !== undefined || graph.data === undefined) {
        throw toApiError(graph.error, graph.response.status);
      }
      const entities = graph.data.nodes.filter((node) => node.depth > 0).map((node) => node.name);

      const [backlinks, posts] = await Promise.all([
        api.GET("/api/documents/{id}/backlinks", { params: { path: { id } } }),
        api.GET("/api/blog/posts"),
      ]);
      if (backlinks.error !== undefined || backlinks.data === undefined) {
        throw toApiError(backlinks.error, backlinks.response.status);
      }
      if (posts.error !== undefined || posts.data === undefined) {
        throw toApiError(posts.error, posts.response.status);
      }
      const published = new Set(posts.data.map((post) => post.document_id));
      return {
        entities,
        posts: backlinks.data.documents
          .filter((doc) => published.has(doc.id))
          .map((doc) => ({ id: doc.id, title: doc.title })),
      };
    },
  });

/**
 * おすすめ — vector search seeded by the post title (content head fallback),
 * self excluded, capped at 5. SearchHit.document_id is already the full
 * `document:<key>` record id, directly comparable with the route id. Vector
 * search spans unpublished documents too, so hits are joined against the
 * published list (T18 residual) — only posts actually listed on "/" survive.
 */
export const recommendedQuery = (id: string, title: string, content: string) =>
  queryOptions({
    queryKey: ["blog", "recommended", id],
    queryFn: async () => {
      const seed = title.trim().length > 0 ? title : content.slice(0, 200);
      const [search, posts] = await Promise.all([
        api.POST("/api/search", {
          body: { query: seed, mode: "vector", top_k: 6 },
        }),
        api.GET("/api/blog/posts"),
      ]);
      if (search.error !== undefined || search.data === undefined) {
        throw toApiError(search.error, search.response.status);
      }
      if (posts.error !== undefined || posts.data === undefined) {
        throw toApiError(posts.error, posts.response.status);
      }
      const published = new Set(posts.data.map((post) => post.document_id));
      return search.data.hits
        .filter((hit) => hit.document_id !== id && published.has(hit.document_id))
        .slice(0, 5);
    },
  });

export async function loginQuery(email: string, password: string) {
  const { data, error, response } = await api.POST("/api/auth/login", {
    body: { email, password },
  });
  if (error !== undefined || data === undefined) {
    throw toApiError(error, response.status);
  }
  return data;
}

export async function registerQuery(email: string, password: string, role: string) {
  const { data, error, response } = await api.POST("/api/auth/register", {
    body: { email, password, role },
  });
  if (error !== undefined || data === undefined) {
    throw toApiError(error, response.status);
  }
  return data;
}

/**
 * Author upload. document_id is null when the exact content was already
 * ingested (sha256 skip) — callers treat that as an error because there is
 * nothing new to publish.
 */
export async function createPostQuery(title: string, content: string) {
  const { data, error, response } = await api.POST("/api/documents", {
    body: { title, content, metadata: { app: "blog" } },
  });
  if (error !== undefined || data === undefined) {
    throw toApiError(error, response.status);
  }
  return data;
}

export async function publishPostQuery(documentId: string) {
  const { data, error, response } = await api.POST("/api/blog/posts/{document_id}/publish", {
    params: { path: { document_id: documentId } },
  });
  if (error !== undefined || data === undefined) {
    throw toApiError(error, response.status);
  }
  return data;
}
