import { MarkdownView } from "@skb/ui";
import { useEffect, useRef } from "react";
import type { ChatMessage } from "../store/session";
import styles from "./ChatMessageList.module.css";

type ChatMessageListProps = {
  messages: ChatMessage[];
  streaming: boolean;
};

export function ChatMessageList({ messages, streaming }: ChatMessageListProps) {
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the newest tokens in view while the transcript grows.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the effect is a scroll trigger keyed on transcript changes
  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [messages]);

  return (
    <div ref={listRef} className={styles.list} data-testid="chat-messages">
      {messages.length === 0 ? (
        <p className={styles.empty}>メッセージを入力して知識ベースに質問してください。</p>
      ) : (
        messages.map((message, index) => (
          <ChatBubble
            // biome-ignore lint/suspicious/noArrayIndexKey: the transcript is append-only and cleared as a whole
            key={index}
            message={message}
            streaming={streaming && index === messages.length - 1 && message.role === "assistant"}
          />
        ))
      )}
    </div>
  );
}

function ChatBubble({ message, streaming }: { message: ChatMessage; streaming: boolean }) {
  if (message.role === "user") {
    return (
      <div className={styles.rowUser}>
        <div className={styles.bubbleUser}>
          <p className={styles.userText}>{message.content}</p>
        </div>
      </div>
    );
  }
  return (
    <div className={styles.rowAssistant} data-testid="chat-bubble-assistant">
      <div className={styles.bubbleAssistant}>
        {message.content === "" && !message.stopped && message.error === undefined && streaming ? (
          <p className={styles.hint}>回答を生成中…</p>
        ) : (
          <MarkdownView content={message.content} streaming={streaming} />
        )}
        {message.stopped && <p className={styles.stopped}>（ここで停止しました）</p>}
        {message.error !== undefined && (
          <p className={styles.error} role="alert">
            {message.error.code}: {message.error.message}
          </p>
        )}
      </div>
    </div>
  );
}
