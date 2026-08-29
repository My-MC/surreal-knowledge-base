import { MarkdownView } from "@skb/ui";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { documentQuery } from "../api";
import styles from "./PostDetail.module.css";
import { RecommendedPosts } from "./RecommendedPosts";
import { RelatedPosts } from "./RelatedPosts";

/**
 * /post/$id — full (non-streaming) post content via @skb/ui MarkdownView,
 * with the 関連記事 (graph/backlinks) and おすすめ (vector search) sections
 * below. The route param is the full `document:<key>` record id.
 */
export function PostDetail() {
  const { id } = useParams({ from: "/post/$id" });
  const { data, isPending, isError, error } = useQuery(documentQuery(id));

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
  return (
    <article className={styles.article}>
      <h1 className={styles.title}>{data.title}</h1>
      <MarkdownView content={data.content} />
      <RelatedPosts id={id} />
      <RecommendedPosts id={id} title={data.title} content={data.content} />
    </article>
  );
}
