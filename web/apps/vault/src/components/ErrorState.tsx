import { ApiError } from "../api";
import styles from "./ErrorState.module.css";

type ErrorStateProps = {
  title: string;
  error: Error;
};

/** Shared failure UI — the QA failure evidence screenshots this. */
export function ErrorState({ title, error }: ErrorStateProps) {
  const detail = error instanceof ApiError ? `${error.code}: ${error.message}` : error.message;
  return (
    <div className={styles.error} role="alert">
      <p className={styles.title}>{title}</p>
      <p className={styles.detail}>{detail}</p>
    </div>
  );
}
