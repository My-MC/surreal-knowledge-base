import styles from "./Stub.module.css";

/**
 * Skeleton only — todo 19 implements the posting flow (author-only form →
 * POST /api/documents with metadata app=blog → publish).
 */
export function NewPostStub() {
  return (
    <section className={styles.stub}>
      <h2 className={styles.title}>新規投稿</h2>
      <p className={styles.note}>
        投稿には author 権限のアカウントでのログインが必要です（認証機能は準備中です）。
      </p>
    </section>
  );
}
