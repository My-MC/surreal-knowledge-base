import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { blogPostsQuery } from "../api";
import styles from "./PostList.module.css";

/** RFC3339 created_at → the YYYY-MM-DD prefix (server datetimes are RFC3339). */
function formatDate(created_at: string): string {
  return created_at.slice(0, 10);
}

/**
 * "/" — the published post list. Public: no auth, newest first (server order).
 */
export function PostList() {
  const { data, isPending, isError, error } = useQuery(blogPostsQuery());

  if (isPending) {
    return (
      <p className={styles.state} role="status">
        読み込み中…
      </p>
    );
  }
  if (isError) {
    return (
      <p className={styles.state} role="alert">
        {error.message}
      </p>
    );
  }
  if (data.length === 0) {
    return (
      <p className={styles.state} data-testid="posts-empty">
        公開中の記事はありません。
      </p>
    );
  }
  return (
    <ul className={styles.list} data-testid="post-list">
      {data.map((post) => (
        <li key={post.document_id} className={styles.card}>
          <Link to="/post/$id" params={{ id: post.document_id }} className={styles.title}>
            {post.title}
          </Link>
          <p className={styles.meta}>
            <span>{post.author}</span>
            <time dateTime={post.created_at}>{formatDate(post.created_at)}</time>
          </p>
        </li>
      ))}
    </ul>
  );
}
