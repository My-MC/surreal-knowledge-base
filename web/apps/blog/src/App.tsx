import { Link, Outlet } from "@tanstack/react-router";

/**
 * App shell: brand + nav header over the route outlet. Nav links to the
 * skeleton routes (/new, /login) are part of the todo-18 scaffold; the
 * register form lives at /register and is linked from the login stub.
 */
export function AppLayout() {
  return (
    <div className="blog-shell">
      <header className="blog-header">
        <Link to="/" className="blog-brand">
          skb Blog
        </Link>
        <nav className="blog-nav">
          <Link to="/">記事一覧</Link>
          <Link to="/new">新規投稿</Link>
          <Link to="/login">ログイン</Link>
        </nav>
      </header>
      <main className="blog-main">
        <Outlet />
      </main>
    </div>
  );
}
