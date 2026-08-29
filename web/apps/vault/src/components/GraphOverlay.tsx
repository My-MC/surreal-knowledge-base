import { GraphView } from "@skb/ui";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { documentsQuery, graphQueryOptions } from "../api";
import { ErrorState } from "./ErrorState";
import styles from "./GraphOverlay.module.css";

type GraphOverlayProps = {
  docId: string;
  onClose: () => void;
};

/**
 * Fullscreen knowledge-graph overlay around the routed document.
 *
 * The depth-0 document node is always present for a `document:` origin, so
 * GraphView's own empty state can never fire here — the overlay renders its
 * empty state when no depth>0 (entity) nodes exist instead. Entity nodes
 * carry no document back-reference, so a click resolves through the cached
 * document list by title; unmatched entities stay inert.
 */
export function GraphOverlay({ docId, onClose }: GraphOverlayProps) {
  const navigate = useNavigate();
  const { data: graph, isPending, isError, error } = useQuery(graphQueryOptions(docId));
  const { data: docs } = useQuery(documentsQuery());

  const onNodeClick = (nodeId: string) => {
    if (nodeId.startsWith("document:")) {
      void navigate({ to: "/doc/$id", params: { id: nodeId } });
      return;
    }
    const node = graph?.nodes.find((candidate) => candidate.id === nodeId);
    const id = docs?.find((doc) => doc.title === node?.name)?.id;
    if (id !== undefined) {
      void navigate({ to: "/doc/$id", params: { id } });
    }
  };

  let body: ReactNode;
  if (isPending) {
    body = <p className={styles.hint}>読み込み中…</p>;
  } else if (isError) {
    body = <ErrorState title="グラフの取得に失敗しました" error={error} />;
  } else if (graph.nodes.filter((node) => node.depth > 0).length === 0) {
    body = (
      <div className={styles.empty} data-testid="graph-empty">
        <p className={styles.emptyMessage}>関連エンティティがありません</p>
      </div>
    );
  } else {
    body = <GraphView nodes={graph.nodes} edges={graph.edges} onNodeClick={onNodeClick} />;
  }

  return (
    <div
      className={styles.overlay}
      data-testid="graph-overlay"
      role="dialog"
      aria-label="ナレッジグラフ"
    >
      <div className={styles.header}>
        <h2 className={styles.title}>ナレッジグラフ</h2>
        <button type="button" className={styles.closeButton} onClick={onClose}>
          閉じる
        </button>
      </div>
      <div className={styles.body}>{body}</div>
    </div>
  );
}
