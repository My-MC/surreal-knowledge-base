import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { backlinksQuery, graphQueryOptions } from "../api";
import { useVaultStore } from "../store";
import styles from "./DocPane.module.css";
import { ErrorState } from "./ErrorState";
import { GraphOverlay } from "./GraphOverlay";

/**
 * Right pane. Server data hangs off the routed document id, which
 * DocumentEditor mirrors into the UI store — on "/" (no document) the pane
 * shows a placeholder instead of firing requests.
 */
export function DocPane() {
  const selectedDocId = useVaultStore((state) => state.selectedDocId);
  if (selectedDocId === null) {
    return (
      <div className={styles.pane}>
        <h2 className="vault-aside-title">バックリンク</h2>
        <p className="vault-aside-hint">ドキュメントを開くと表示されます</p>
      </div>
    );
  }
  return <DocPaneContent docId={selectedDocId} />;
}

type DocPaneContentProps = {
  docId: string;
};

function DocPaneContent({ docId }: DocPaneContentProps) {
  const navigate = useNavigate();
  const [graphOpen, setGraphOpen] = useState(false);
  const backlinks = useQuery(backlinksQuery(docId));
  const graph = useQuery(graphQueryOptions(docId));

  const entities = graph.data?.nodes.filter((node) => node.depth > 0) ?? [];

  return (
    <div className={styles.pane}>
      <section className={styles.section}>
        <h2 className="vault-aside-title">バックリンク</h2>
        {backlinks.isPending ? (
          <p className={styles.hint}>読み込み中…</p>
        ) : backlinks.isError ? (
          <ErrorState title="バックリンクの取得に失敗しました" error={backlinks.error} />
        ) : backlinks.data.documents.length === 0 ? (
          <p className={styles.hint}>バックリンクはありません</p>
        ) : (
          <ul className={styles.list} data-testid="backlinks-list">
            {backlinks.data.documents.map((doc) => (
              <li key={doc.id}>
                <button
                  type="button"
                  className={styles.item}
                  data-testid="backlink-item"
                  onClick={() => void navigate({ to: "/doc/$id", params: { id: doc.id } })}
                >
                  {doc.title}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className={styles.section}>
        <h2 className="vault-aside-title">関連エンティティ</h2>
        {graph.isPending ? (
          <p className={styles.hint}>読み込み中…</p>
        ) : graph.isError ? (
          <ErrorState title="グラフの取得に失敗しました" error={graph.error} />
        ) : entities.length === 0 ? (
          <p className={styles.hint}>関連エンティティはありません</p>
        ) : (
          <ul className={styles.list} data-testid="related-entities">
            {entities.map((node) => (
              <li key={node.id} className={styles.entity}>
                <span className={styles.entityName}>{node.name}</span>
                <span className={styles.entityKind}>{node.kind}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
      <button type="button" className={styles.graphButton} onClick={() => setGraphOpen(true)}>
        全画面グラフ
      </button>
      {graphOpen && <GraphOverlay docId={docId} onClose={() => setGraphOpen(false)} />}
    </div>
  );
}
