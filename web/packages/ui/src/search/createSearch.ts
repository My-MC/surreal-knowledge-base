/** One search result (mirrors the server DTO in crates/skb-server/src/dto/search.rs). */
export interface SearchHit {
  document_id: string;
  chunk_idx: number;
  content: string;
  score: number;
  title?: string | null;
  source?: string | null;
  highlights?: string[] | null;
  matched_entities?: string[] | null;
}

/** Search fn accepted by SearchPalette (the testable seam). */
export type SearchFn = (query: string) => Promise<SearchHit[]>;

/**
 * Default SearchPalette wiring: POST /api/search (hybrid, top_k 8).
 *
 * Documented choice: this deliberately does NOT import @skb/api-client. A
 * runtime import of its index loads useChatStream, whose `react` import
 * resolves to that package's react-shim.d.ts under bun (tsconfig paths are
 * honored at runtime) and crashes; a type-only import would still pull
 * api-client sources into this package's tsc program, where `react` is not
 * resolvable (it is not a declared dependency of api-client). The SearchHit
 * shape therefore mirrors the server DTO locally. `baseUrl` defaults to ""
 * (same-origin `/api` — the vite dev proxy or a reverse proxy in
 * production). Pass a custom `search` prop to override.
 */
export function createSearch(baseUrl = ""): SearchFn {
  return async (query) => {
    const response = await fetch(`${baseUrl}/api/search`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ query, mode: "hybrid", top_k: 8 }),
    });
    if (!response.ok) {
      throw new Error(`POST /api/search failed: HTTP ${response.status}`);
    }
    const data = (await response.json()) as { hits: SearchHit[] };
    return data.hits;
  };
}
