import { useChatStream } from "@skb/api-client";
import { useEffect, useRef } from "react";
import { ChatComposer } from "./components/ChatComposer";
import { ChatMessageList } from "./components/ChatMessageList";
import { useSessionStore } from "./store/session";

/**
 * Studio chat: the transcript lives in the persisted session store; the
 * stream hook owns the transport. Hook state is synced into the store —
 * token deltas (the hook's `tokens` is cumulative, so only the unsynced
 * suffix is appended), citations, and terminal states.
 */
export function ChatApp() {
  const messages = useSessionStore((s) => s.messages);
  const status = useSessionStore((s) => s.status);
  const appendMessage = useSessionStore((s) => s.appendMessage);
  const appendTokenToLast = useSessionStore((s) => s.appendTokenToLast);
  const setCitationsOnLast = useSessionStore((s) => s.setCitationsOnLast);
  const markLastStopped = useSessionStore((s) => s.markLastStopped);
  const setErrorOnLast = useSessionStore((s) => s.setErrorOnLast);
  const setStatus = useSessionStore((s) => s.setStatus);
  const clearSession = useSessionStore((s) => s.clearSession);

  const stream = useChatStream("");
  const syncedTokensRef = useRef(0);

  useEffect(() => {
    if (stream.tokens.length > syncedTokensRef.current) {
      appendTokenToLast(stream.tokens.slice(syncedTokensRef.current));
      syncedTokensRef.current = stream.tokens.length;
    }
  }, [stream.tokens, appendTokenToLast]);

  useEffect(() => {
    if (stream.citations.length > 0) {
      setCitationsOnLast(stream.citations);
    }
  }, [stream.citations, setCitationsOnLast]);

  useEffect(() => {
    if (stream.status === "done") {
      setStatus("done");
    } else if (stream.status === "error" && stream.error !== null) {
      setErrorOnLast(stream.error);
      setStatus("error");
    }
  }, [stream.status, stream.error, setStatus, setErrorOnLast]);

  const send = (text: string) => {
    syncedTokensRef.current = 0;
    appendMessage({ role: "user", content: text });
    appendMessage({ role: "assistant", content: "" });
    setStatus("streaming");
    void stream.start(text);
  };

  const stop = () => {
    stream.stop();
    markLastStopped();
    setStatus("idle");
  };

  return (
    <div className="studio-layout">
      <header className="studio-header">
        <h1 className="studio-title">skb Studio</h1>
        <button type="button" className="studio-clear" onClick={clearSession}>
          新規セッション
        </button>
      </header>
      <ChatMessageList messages={messages} streaming={status === "streaming"} />
      <ChatComposer streaming={status === "streaming"} onSend={send} onStop={stop} />
    </div>
  );
}
