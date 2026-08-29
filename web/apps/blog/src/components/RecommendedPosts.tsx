import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { recommendedQuery } from "../api";
import styles from "./PostDetail.module.css";

interface RecommendedPostsProps {
  id: string;
  title: string;
  content: string;
}

/** おすすめ — vector-search neighbors of this post, self excluded, up to 5. */
export function RecommendedPosts({ id, title, content }: RecommendedPostsProps) {
  const { data, isPending, isError, error } = useQuery(recommendedQuery(id, title, content));

  return (
    <section className={styles.section} data-testid="recommended">
      <h2 className={styles.sectionTitle}>おすすめ</h2>
      {isPending ? (
        <p role="status">読み込み中…</p>
      ) : isError ? (
        <p role="alert">{error.message}</p>
      ) : data.length === 0 ? (
        <p data-testid="recommended-empty">おすすめの記事はありません。</p>
      ) : (
        <ul className={styles.postList}>
          {data.map((hit) => (
            <li key={hit.document_id}>
              <Link to="/post/$id" params={{ id: hit.document_id }}>
                {hit.title ?? "無題"}
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
