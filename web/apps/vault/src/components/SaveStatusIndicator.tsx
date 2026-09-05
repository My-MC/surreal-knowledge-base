import styles from "./DocumentEditor.module.css";
import type { SaveStatus } from "./useAutosave";

export function formatClock(at: Date): string {
  const hours = String(at.getHours()).padStart(2, "0");
  const minutes = String(at.getMinutes()).padStart(2, "0");
  return `${hours}:${minutes}`;
}

type SaveStatusIndicatorProps = {
  status: SaveStatus;
  onRetry: () => void;
};

export function SaveStatusIndicator({ status, onRetry }: SaveStatusIndicatorProps) {
  switch (status.kind) {
    case "idle":
      return null;
    case "saving":
      return (
        <p className={styles.saveStatus} role="status">
          保存中…
        </p>
      );
    case "saved":
      return (
        <p className={styles.saveStatus} role="status">
          保存済み {formatClock(status.at)}
        </p>
      );
    case "error":
      return (
        <div className={styles.saveError} role="alert">
          <p className={styles.saveErrorText}>保存に失敗: {status.error.message}</p>
          <button type="button" className={styles.retryButton} onClick={onRetry}>
            再試行
          </button>
        </div>
      );
  }
}
