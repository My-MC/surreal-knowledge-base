import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { documentsQuery } from "../api";
import { useVaultStore } from "../store";
import styles from "./DocumentTree.module.css";
import { ErrorState } from "./ErrorState";
import { useCreateDocument } from "./useCreateDocument";

/** Left pane: document list (title + created_at) with the create action. */
export function DocumentTree() {
  const navigate = useNavigate();
  const { data: docs, isPending, isError, error } = useQuery(documentsQuery());
  const { createDocument, isPending: creating } = useCreateDocument();
  const selectedDocId = useVaultStore((state) => state.selectedDocId);
  const selectDoc = useVaultStore((state) => state.selectDoc);

  return (
    <div className={styles.tree}>
      <div className={styles.header}>
        <h1 className={styles.title}>ドキュメント</h1>
        <button
          type="button"
          className={styles.createButton}
          onClick={() => createDocument()}
          disabled={creating}
        >
          {creating ? "作成中…" : "新規作成"}
        </button>
      </div>
      {isPending ? (
        <p className={styles.status}>読み込み中…</p>
      ) : isError ? (
        <ErrorState title="ドキュメント一覧の取得に失敗しました" error={error} />
      ) : docs.length === 0 ? (
        <p className={styles.status}>ドキュメントがありません</p>
      ) : (
        <ul className={styles.list}>
          {docs.map((doc) => (
            <li key={doc.id}>
              <button
                type="button"
                className={
                  doc.id === selectedDocId ? `${styles.item} ${styles.itemActive}` : styles.item
                }
                onClick={() => {
                  selectDoc(doc.id);
                  void navigate({ to: "/doc/$id", params: { id: doc.id } });
                }}
              >
                <span className={styles.itemTitle}>{doc.title}</span>
                <time className={styles.itemDate}>{new Date(doc.created_at).toLocaleString()}</time>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
