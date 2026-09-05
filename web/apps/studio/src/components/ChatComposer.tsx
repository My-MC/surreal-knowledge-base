import { useState } from "react";
import styles from "./ChatComposer.module.css";

type ChatComposerProps = {
  streaming: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
};

export function ChatComposer({ streaming, onSend, onStop }: ChatComposerProps) {
  const [draft, setDraft] = useState("");
  const trimmed = draft.trim();

  const submit = () => {
    if (trimmed === "" || streaming) return;
    onSend(trimmed);
    setDraft("");
  };

  return (
    <div className={styles.composer}>
      <textarea
        className={styles.input}
        data-testid="chat-input"
        value={draft}
        placeholder="知識ベースについて質問…"
        rows={2}
        onChange={(event) => setDraft(event.target.value)}
      />
      {streaming ? (
        <button type="button" className={styles.stop} data-testid="chat-stop" onClick={onStop}>
          止る
        </button>
      ) : (
        <button
          type="button"
          className={styles.send}
          data-testid="chat-send"
          onClick={submit}
          disabled={trimmed === ""}
        >
          送信
        </button>
      )}
    </div>
  );
}
