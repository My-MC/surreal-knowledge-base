import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { relatedQuery } from "../api";
import styles from "./PostDetail.module.css";

/**
 * 関連記事 — other published posts sharing an entity with this one
 * (see relatedQuery in ../api for the graph/backlinks split).
 */
export function RelatedPosts({ id }: { id: string }) {
  const { data, isPending, isError, error } = useQuery(relatedQuery(id));

  return (
    <section className={styles.section} data-testid="related">
      <h2 className={styles.sectionTitle}>関連記事</h2>
      {isPending ? (
        <p role="status">読み込み中…</p>
      ) : isError ? (
        <p role="alert">{error.message}</p>
      ) : (
        <>
          {data.entities.length > 0 && (
            <p className={styles.entities} data-testid="related-entities">
              エンティティ: {data.entities.join("、")}
            </p>
          )}
          {data.posts.length === 0 ? (
            <p data-testid="related-empty">関連記事はありません。</p>
          ) : (
            <ul className={styles.postList}>
              {data.posts.map((post) => (
                <li key={post.id}>
                  <Link to="/post/$id" params={{ id: post.id }}>
                    {post.title}
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}
