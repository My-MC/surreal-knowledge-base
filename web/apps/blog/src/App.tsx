import { Link, Outlet } from "@tanstack/react-router";
import { useAuthStore } from "./auth";

/**
 * App shell: brand + nav header over the route outlet. Logged-out visitors
 * get ログイン/新規登録 links; logged-in users see their email (echoed from
 * the login response body — the cookie itself is HttpOnly) plus ログアウト,
 * which clears the local store only; the server cookie expires on its own.
 */
export function AppLayout() {
  const email = useAuthStore((state) => state.email);
  const clearAuth = useAuthStore((state) => state.clearAuth);

  return (
    <div className="blog-shell">
      <header className="blog-header">
        <Link to="/" className="blog-brand">
          skb Blog
        </Link>
        <nav className="blog-nav">
          <Link to="/">記事一覧</Link>
          <Link to="/new">新規投稿</Link>
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
                onClick={clearAuth}
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
