import type { SearchHit } from "@skb/api-client";
import styles from "./CitationPanel.module.css";

type CitationPanelProps = {
  hits: SearchHit[];
};

/**
 * Citation rail for the latest assistant response. 「なぜ引用されたか」 is
 * answered per hit: the RRF-fused score plus matched_entities — the origin
 * entities that pulled the hit in via graph expansion (null for direct
 * keyword/vector hits, rendered as —). Each title links to the vault
 * document route /doc/{document_id} (ids arrive document:-prefixed).
 */
export function CitationPanel({ hits }: CitationPanelProps) {
  return (
    <aside className="studio-panel" data-testid="citation-panel">
      <h2 className={styles.heading}>引用</h2>
      <p className={styles.subtitle}>なぜ引用されたか: スコアと経由エンティティ</p>
      {hits.length === 0 ? (
        <p className={styles.empty} data-testid="citation-empty">
          この応答には引用がありません。
        </p>
      ) : (
        <ul className={styles.list}>
          {hits.map((hit) => (
            <li
              key={`${hit.document_id}:${hit.chunk_idx}`}
              className={styles.item}
              data-testid="citation-item"
            >
              <a className={styles.link} href={`/doc/${hit.document_id}`}>
                {hit.title ?? hit.document_id}
              </a>
              <dl className={styles.meta}>
                <div className={styles.metaRow}>
                  <dt className={styles.metaName}>スコア</dt>
                  <dd className={styles.metaValue}>{hit.score.toFixed(4)}</dd>
                </div>
                <div className={styles.metaRow}>
                  <dt className={styles.metaName}>経由エンティティ</dt>
                  <dd className={styles.metaValue} data-testid="citation-entities">
                    {hit.matched_entities?.length ? hit.matched_entities.join("、") : "—"}
                  </dd>
                </div>
                <div className={styles.metaRow}>
                  <dt className={styles.metaName}>ハイライト</dt>
                  <dd className={styles.metaValue}>
                    {hit.highlights?.length ? hit.highlights.join("、") : "—"}
                  </dd>
                </div>
              </dl>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
