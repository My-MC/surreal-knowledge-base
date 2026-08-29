import { useParams } from "@tanstack/react-router";
import { useEffect } from "react";
import { useVaultStore } from "../store";
import styles from "./DocumentPlaceholder.module.css";

/** Center-pane placeholder for todo 14 (editor + preview). Shows the routed id. */
export function DocumentPlaceholder() {
  const { id } = useParams({ from: "/doc/$id" });
  const selectDoc = useVaultStore((state) => state.selectDoc);

  useEffect(() => {
    selectDoc(id);
  }, [id, selectDoc]);

  return (
    <div className={styles.placeholder}>
      <p className={styles.hint}>エディタとプレビューは todo 14 で実装されます</p>
      <code className={styles.docId}>{id}</code>
    </div>
  );
}
