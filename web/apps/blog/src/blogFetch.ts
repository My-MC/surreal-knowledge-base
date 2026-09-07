import { useAuthStore } from "./auth";

/**
 * Custom fetch used by every blog API request (wired in api.ts).
 *
 * 1. `credentials: "include"` — the session is the HttpOnly `skb_session`
 *    cookie set by POST /api/auth/login. Same-origin proxying would also work
 *    with the default "same-origin" policy, but the plan mandates include so
 *    the client keeps working unchanged if the app is ever served from a
 *    different origin than the API.
 * 2. Any 401 response clears the persisted auth store: the cookie is invalid
 *    or expired at that point, so a cached {email, role} would outlive the
 *    session it came from. (The login endpoint's own 401 — wrong password —
 *    is a harmless no-op: the store is empty in every state that can reach
 *    it.)
 *
 * `fetch` is resolved at call time so tests can stub globalThis.fetch.
 */
export function blogFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  return fetch(input, { ...init, credentials: "include" }).then((response) => {
    if (response.status === 401) {
      useAuthStore.getState().clearAuth();
    }
    return response;
  });
}
