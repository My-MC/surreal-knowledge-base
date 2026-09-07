import type { ChatStreamError, ChatStreamStatus, SearchHit } from "@skb/api-client";
import { create } from "zustand";
import { persist } from "zustand/middleware";

export type ChatRole = "user" | "assistant";

export type ChatMessage = {
  role: ChatRole;
  content: string;
  citations?: SearchHit[];
  /** Set when the user aborted the stream mid-answer; partial content stays. */
  stopped?: boolean;
  /** Set when the stream ended with an in-band or transport error. */
  error?: ChatStreamError;
};

export type SessionState = {
  /** Ephemeral session id; regenerated on every load (not persisted). */
  id: string;
  messages: ChatMessage[];
  status: ChatStreamStatus;
  appendMessage: (message: ChatMessage) => void;
  appendTokenToLast: (text: string) => void;
  setCitationsOnLast: (hits: SearchHit[]) => void;
  markLastStopped: () => void;
  setErrorOnLast: (error: ChatStreamError) => void;
  setStatus: (status: ChatStreamStatus) => void;
  clearSession: () => void;
};

const STORAGE_KEY = "skb-studio-session";

/** Patch the last message; a no-op when the transcript is empty. */
function patchLast(
  state: SessionState,
  patch: (last: ChatMessage) => ChatMessage,
): Partial<SessionState> {
  const last = state.messages.at(-1);
  if (last === undefined) return state;
  return { messages: [...state.messages.slice(0, -1), patch(last)] };
}

/**
 * Factory so tests (and future multi-session surfaces) can mint isolated
 * stores against the same persisted key; the app uses the singleton below.
 */
export function createSessionStore() {
  return create<SessionState>()(
    persist(
      (set) => ({
        id: crypto.randomUUID(),
        messages: [],
        status: "idle",
        appendMessage: (message) => set((state) => ({ messages: [...state.messages, message] })),
        appendTokenToLast: (text) =>
          set((state) => {
            if (text === "") return state;
            return patchLast(state, (last) => ({ ...last, content: last.content + text }));
          }),
        setCitationsOnLast: (hits) =>
          set((state) => patchLast(state, (last) => ({ ...last, citations: hits }))),
        markLastStopped: () =>
          set((state) => patchLast(state, (last) => ({ ...last, stopped: true }))),
        setErrorOnLast: (error) => set((state) => patchLast(state, (last) => ({ ...last, error }))),
        setStatus: (status) => set({ status }),
        clearSession: () => set({ messages: [] }),
      }),
      {
        name: STORAGE_KEY,
        // Only the transcript survives reloads; id/status/actions stay ephemeral.
        partialize: (state) => ({ messages: state.messages }),
      },
    ),
  );
}

export const useSessionStore = createSessionStore();
