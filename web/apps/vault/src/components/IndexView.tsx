import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";
import { documentsQuery } from "../api";
import { ErrorState } from "./ErrorState";
import styles from "./IndexView.module.css";
import { useCreateDocument } from "./useCreateDocument";

/**
 * "/" — navigates to the latest document when the list is non-empty, or shows
 * the empty state with the create action. The redirect happens at component
 * level (not beforeLoad) so a failed list request renders the error UI instead
 * of a router-level error screen.
 */
export function IndexView() {
  const navigate = useNavigate();
  const { data: docs, isPending, isError, error } = useQuery(documentsQuery());
  const { createDocument, isPending: creating } = useCreateDocument();

  useEffect(() => {
    if (!docs || docs.length === 0) {
      return;
    }
    const latest = [...docs].sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
    if (latest) {
      void navigate({ to: "/doc/$id", params: { id: latest.id }, replace: true });
    }
  }, [docs, navigate]);

  if (isPending) {
    return <p className={styles.status}>読み込み中…</p>;
  }
  if (isError) {
    return <ErrorState title="ドキュメント一覧の取得に失敗しました" error={error} />;
  }
  if (docs.length > 0) {
    return <p className={styles.status}>最新のドキュメントを開いています…</p>;
  }
  return (
    <div className={styles.empty}>
      <h2 className={styles.heading}>Vaultは空です</h2>
      <p className={styles.hint}>最初のドキュメントを作成しましょう。</p>
      <button
        type="button"
        className={styles.createButton}
        onClick={() => createDocument()}
        disabled={creating}
      >
        {creating ? "作成中…" : "新規作成"}
      </button>
    </div>
  );
}
