import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { useEffect } from "react";
import { documentQuery } from "../api";
import { useVaultStore } from "../store";
import styles from "./DocumentEditor.module.css";
import { EditorSurface } from "./EditorSurface";
import { ErrorState } from "./ErrorState";

export function DocumentEditor() {
  const { id } = useParams({ from: "/doc/$id" });
  const selectDoc = useVaultStore((state) => state.selectDoc);

  useEffect(() => {
    selectDoc(id);
  }, [id, selectDoc]);

  const { data: doc, isPending, isError, error } = useQuery(documentQuery(id));

  if (isPending) {
    return (
      <p className={styles.loadStatus} role="status">
        読み込み中…
      </p>
    );
  }
  if (isError) {
    return <ErrorState title="ドキュメントの取得に失敗しました" error={error} />;
  }
  return <EditorSurface doc={doc} />;
}
