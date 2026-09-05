import { beforeEach, describe, expect, test } from "bun:test";
import type { SearchHit } from "@skb/api-client";
import { createSessionStore } from "./session";

function hit(chunkIdx: number): SearchHit {
  return {
    chunk_idx: chunkIdx,
    content: `chunk ${chunkIdx}`,
    document_id: "document:smoke",
    score: 1 - chunkIdx * 0.1,
    title: "Smoke Doc",
  };
}

beforeEach(() => {
  localStorage.clear();
});

describe("session store actions", () => {
  test("appendMessage appends user and assistant messages in order", () => {
    const store = createSessionStore();
    store.getState().appendMessage({ role: "user", content: "質問" });
    store.getState().appendMessage({ role: "assistant", content: "" });
    expect(store.getState().messages).toEqual([
      { role: "user", content: "質問" },
      { role: "assistant", content: "" },
    ]);
  });

  test("appendTokenToLast appends sequential deltas to the last message", () => {
    const store = createSessionStore();
    store.getState().appendMessage({ role: "user", content: "質問" });
    store.getState().appendMessage({ role: "assistant", content: "" });
    store.getState().appendTokenToLast("Based on ");
    store.getState().appendTokenToLast("the excerpts, ");
    store.getState().appendTokenToLast("this is a mock answer");
    expect(store.getState().messages.at(-1)?.content).toBe(
      "Based on the excerpts, this is a mock answer",
    );
  });

  test("appendTokenToLast ignores empty deltas and an empty transcript", () => {
    const store = createSessionStore();
    store.getState().appendTokenToLast("");
    expect(store.getState().messages).toEqual([]);
    store.getState().appendTokenToLast("orphan");
    expect(store.getState().messages).toEqual([]);
  });

  test("setCitationsOnLast attaches hits to the last message", () => {
    const store = createSessionStore();
    store.getState().appendMessage({ role: "assistant", content: "answer" });
    store.getState().setCitationsOnLast([hit(0), hit(1)]);
    expect(store.getState().messages.at(-1)?.citations).toEqual([hit(0), hit(1)]);
  });

  test("clearSession empties the transcript but keeps the store usable", () => {
    const store = createSessionStore();
    store.getState().appendMessage({ role: "user", content: "質問" });
    store.getState().clearSession();
    expect(store.getState().messages).toEqual([]);
    store.getState().appendMessage({ role: "user", content: "次" });
    expect(store.getState().messages).toHaveLength(1);
  });
});

describe("session store persistence", () => {
  test("a new store instance reads the transcript persisted by a previous one", () => {
    const first = createSessionStore();
    first.getState().appendMessage({ role: "user", content: "質問" });
    first.getState().appendMessage({ role: "assistant", content: "回答" });
    first.getState().setCitationsOnLast([hit(2)]);

    const second = createSessionStore();
    expect(second.getState().messages).toEqual(first.getState().messages);
    expect(second.getState().messages.at(-1)?.citations).toEqual([hit(2)]);
  });

  test("status is not persisted — a fresh instance starts idle", () => {
    const first = createSessionStore();
    first.getState().setStatus("streaming");
    const second = createSessionStore();
    expect(second.getState().status).toBe("idle");
  });
});

describe("session store abort behavior", () => {
  test("stop keeps partial content, marks stopped, and settles status to idle", () => {
    // Drives the exact action sequence ChatApp performs around stream.stop().
    const store = createSessionStore();
    const act = store.getState();
    act.appendMessage({ role: "user", content: "質問" });
    act.appendMessage({ role: "assistant", content: "" });
    act.setStatus("streaming");
    act.appendTokenToLast("partial answer");
    act.markLastStopped();
    act.setStatus("idle");

    const state = store.getState();
    expect(state.status).toBe("idle");
    expect(state.messages.at(-1)?.content).toBe("partial answer");
    expect(state.messages.at(-1)?.stopped).toBe(true);
    expect(state.messages.at(-1)?.error).toBeUndefined();
  });

  test("an in-band error lands on the assistant message with error status", () => {
    const store = createSessionStore();
    const act = store.getState();
    act.appendMessage({ role: "assistant", content: "partial" });
    act.setErrorOnLast({ code: "E_LLM_UPSTREAM", message: "connection reset" });
    act.setStatus("error");

    const state = store.getState();
    expect(state.status).toBe("error");
    expect(state.messages.at(-1)?.error).toEqual({
      code: "E_LLM_UPSTREAM",
      message: "connection reset",
    });
    expect(state.messages.at(-1)?.content).toBe("partial");
  });
});
