import { createClient } from "@skb/api-client";
import { MarkdownView } from "@skb/ui";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import styles from "./DocView.module.css";

const api = createClient("");

/**
 * `/doc/$documentId` — the citation panel's link target. Studio registers
 * this route so a citation click resolves on the same origin and renders the
 * referenced document instead of landing on an unregistered path.
 */
export function DocView() {
  const { documentId } = useParams({ from: "/doc/$documentId" });
  const { data, isPending, isError } = useQuery({
    queryKey: ["studio", "document", documentId],
    queryFn: async () => {
      const { data, error, response } = await api.GET("/api/documents/{id}", {
        params: { path: { id: documentId } },
      });
      if (error !== undefined || data === undefined) {
        throw new Error(`HTTP ${response.status}`);
      }
      return data;
    },
  });

  if (isPending) {
    return <p className={styles.state}>読み込み中…</p>;
  }
  if (isError || data === undefined) {
    return <p className={styles.state}>ドキュメントを表示できませんでした。</p>;
  }
  return (
    <article className={styles.doc}>
      <h2 className={styles.title}>{data.title}</h2>
      <div className={styles.body} data-testid="doc-content">
        <MarkdownView content={data.content} />
      </div>
    </article>
  );
}
