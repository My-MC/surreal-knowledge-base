import { Link, Outlet } from "@tanstack/react-router";
import { useCallback } from "react";
import { logoutQuery } from "./api";
import { useAuthStore } from "./auth";

/**
 * App shell: brand + nav header over the route outlet. Logged-out visitors
 * get ログイン/新規登録 links; logged-in users see their email (echoed from
 * the login response body — the cookie itself is HttpOnly) plus ログアウト,
 * which revokes the session server-side (jti list + cookie expiry) and then
 * clears the local store. 新規投稿 is author-only: readers are redirected
 * away from /new, so the link would be a dead end for them.
 */
export function AppLayout() {
  const email = useAuthStore((state) => state.email);
  const role = useAuthStore((state) => state.role);
  const clearAuth = useAuthStore((state) => state.clearAuth);

  const handleLogout = useCallback(async () => {
    try {
      await logoutQuery();
    } catch {
      // Local sign-out must proceed even when the server call fails
      // (offline, 503, already-revoked session); the stale cookie is
      // rejected server-side on the next request anyway.
    } finally {
      clearAuth();
    }
  }, [clearAuth]);

  return (
    <div className="blog-shell">
      <header className="blog-header">
        <Link to="/" className="blog-brand">
          skb Blog
        </Link>
        <nav className="blog-nav">
          <Link to="/">記事一覧</Link>
          {role === "author" && <Link to="/new">新規投稿</Link>}
          {email === null ? (
            <>
              <Link to="/login">ログイン</Link>
              <Link to="/register">新規登録</Link>
            </>
          ) : (
            <>
              <span className="blog-nav-email" data-testid="header-email">
                {email}
              </span>
              <button
                type="button"
                className="blog-nav-logout"
                data-testid="logout"
                onClick={() => void handleLogout()}
              >
                ログアウト
              </button>
            </>
          )}
        </nav>
      </header>
      <main className="blog-main">
        <Outlet />
      </main>
    </div>
  );
}
