import { useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { ApiError, createPostQuery, publishPostQuery } from "../api";
import styles from "./Form.module.css";

/**
 * /new — author-only posting flow (the route's beforeLoad guard redirects
 * non-authors to /login). Submit ingests the document with metadata
 * app=blog; the 投稿完了 state then offers 公開する, which publishes the
 * registered blog post and returns to the list.
 */
export function NewPost() {
  const navigate = useNavigate();
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [documentId, setDocumentId] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      const uploaded = await createPostQuery(title, content);
      if (uploaded.document_id === undefined || uploaded.document_id === null) {
        throw new ApiError(
          "E_UPLOAD_SKIPPED",
          "同じ内容の文書が既に存在するため、新規作成できませんでした。",
        );
      }
      setDocumentId(uploaded.document_id);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "予期しないエラーが発生しました。");
    } finally {
      setPending(false);
    }
  }

  async function handlePublish() {
    if (documentId === null) return;
    setPending(true);
    setError(null);
    try {
      await publishPostQuery(documentId);
      await navigate({ to: "/" });
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "予期しないエラーが発生しました。");
    } finally {
      setPending(false);
    }
  }

  if (documentId !== null) {
    return (
      <section className={styles.stub} data-testid="new-success">
        <h2 className={styles.title}>投稿完了</h2>
        <p className={styles.note}>
          下書きを保存しました（document_id: <code>{documentId}</code>
          ）。公開すると記事一覧に表示されます。
        </p>
        <div className={styles.form}>
          <button
            type="button"
            data-testid="new-publish"
            onClick={handlePublish}
            disabled={pending}
          >
            公開する
          </button>
          {error !== null && (
            <p className={styles.error} role="alert" data-testid="new-error">
              {error}
            </p>
          )}
        </div>
      </section>
    );
  }

  return (
    <section className={styles.stub}>
      <h2 className={styles.title}>新規投稿</h2>
      <form className={styles.formWide} onSubmit={handleSubmit}>
        <input
          type="text"
          placeholder="タイトル"
          aria-label="タイトル"
          data-testid="new-title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          required
        />
        <textarea
          placeholder="本文（Markdown、[[ウィキリンク]] 利用可）"
          aria-label="本文"
          data-testid="new-content"
          rows={12}
          value={content}
          onChange={(event) => setContent(event.target.value)}
          required
        />
        <button type="submit" data-testid="new-submit" disabled={pending}>
          {pending ? "送信中…" : "投稿"}
        </button>
        {error !== null && (
          <p className={styles.error} role="alert" data-testid="new-error">
            {error}
          </p>
        )}
      </form>
    </section>
  );
}
