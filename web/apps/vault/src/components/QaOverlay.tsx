import { useChatStream } from "@skb/api-client";
import { MarkdownView } from "@skb/ui";
import { useEffect } from "react";
import styles from "./QaOverlay.module.css";

type QaOverlayProps = {
  question: string;
  onClose: () => void;
};

/**
 * Bottom-right overlay streaming an LLM answer for the editor selection.
 *
 * The stream starts in an effect with no started-guard and no local cleanup:
 * under StrictMode's mount → cleanup → remount cycle the hook's own abort
 * cleanup kills the first attempt and the re-run starts a fresh one, while a
 * real unmount is still handled by the hook's abort-on-unmount effect.
 */
export function QaOverlay({ question, onClose }: QaOverlayProps) {
  const { start, status, tokens, citations, error } = useChatStream("");

  useEffect(() => {
    void start(question);
  }, [start, question]);

  return (
    <div
      className={styles.overlay}
      data-testid="qa-overlay"
      role="dialog"
      aria-label="選択範囲について質問"
    >
      <div className={styles.header}>
        <h2 className={styles.title}>選択範囲について質問</h2>
        <button
          type="button"
          className={styles.closeButton}
          data-testid="qa-close"
          onClick={onClose}
        >
          閉じる
        </button>
      </div>
      <div className={styles.body}>
        {error !== null ? (
          <p className={styles.error} role="alert">
            {error.code}: {error.message}
          </p>
        ) : tokens === "" && status === "streaming" ? (
          <p className={styles.hint}>回答を生成中…</p>
        ) : (
          <MarkdownView content={tokens} streaming={status === "streaming"} />
        )}
        {citations.length > 0 && (
          <div className={styles.citations} data-testid="qa-citations">
            <h3 className={styles.citationsTitle}>引用</h3>
            <ul className={styles.citationsList}>
              {citations.map((hit) => (
                <li key={`${hit.document_id}:${hit.chunk_idx}`} className={styles.citation}>
                  {hit.title ?? hit.document_id}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
