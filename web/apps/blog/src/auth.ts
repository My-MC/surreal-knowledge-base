import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Client-side auth state. The session itself is the HttpOnly `skb_session`
 * cookie (invisible to JS); the server echoes {email, role} in the login and
 * register response bodies, and that echo is what this store persists so the
 * header and the /new guard can react without a /api/auth/me endpoint.
 *
 * A 401 from any API call clears the store (see createBlogClient in api.ts) —
 * the cookie is invalid or expired at that point, so the cached role would
 * otherwise outlive the session it came from.
 */
export type AuthState = {
  /** Logged-in user's email; null when logged out. */
  email: string | null;
  /** "reader" | "author"; null when logged out. */
  role: string | null;
  setAuth: (email: string, role: string) => void;
  clearAuth: () => void;
};

const STORAGE_KEY = "skb-blog-auth";

/**
 * Factory so tests can mint isolated stores against the same persisted key;
 * the app uses the singleton below. zustand persist hydrates synchronously
 * from localStorage during store creation.
 */
export function createAuthStore() {
  return create<AuthState>()(
    persist(
      (set) => ({
        email: null,
        role: null,
        setAuth: (email, role) => set({ email, role }),
        clearAuth: () => set({ email: null, role: null }),
      }),
      {
        name: STORAGE_KEY,
        // Only the identity survives reloads; actions stay ephemeral.
        partialize: (state) => ({ email: state.email, role: state.role }),
      },
    ),
  );
}

export const useAuthStore = createAuthStore();
