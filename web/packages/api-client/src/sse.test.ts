import { describe, expect, it } from "bun:test";
import { consumeSseStream, type SearchHit, type SseHandlers } from "./sse";

const enc = new TextEncoder();

interface Collected {
  events: string[];
  tokens: string[];
  citations: SearchHit[][];
  errors: { code: string; message: string }[];
  handlers: SseHandlers;
  firstToken: Promise<void>;
}

function collect(): Collected {
  const events: string[] = [];
  const tokens: string[] = [];
  const citations: SearchHit[][] = [];
  const errors: { code: string; message: string }[] = [];
  let resolveFirstToken: () => void = () => {};
  const firstToken = new Promise<void>((resolve) => {
    resolveFirstToken = resolve;
  });
  let seenToken = false;
  return {
    events,
    tokens,
    citations,
    errors,
    firstToken,
    handlers: {
      onCitation: (hits) => {
        events.push("citation");
        citations.push(hits);
      },
      onToken: (text) => {
        events.push("token");
        tokens.push(text);
        if (!seenToken) {
          seenToken = true;
          resolveFirstToken();
        }
      },
      onDone: () => events.push("done"),
      onError: (code, message) => {
        events.push("error");
        errors.push({ code, message });
      },
    },
  };
}

function sseResponse(...chunks: Uint8Array[]): Response {
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

const HIT =
  '{"document_id":"document:readme","chunk_idx":0,"content":"hello","score":1.5,"title":"README","source":null,"highlights":null,"matched_entities":null}';

describe("consumeSseStream", () => {
  it("dispatches citation, tokens, and done in wire order", async () => {
    const wire = `event: citation\ndata: {"hits":[${HIT}]}\n\nevent: token\ndata: {"text":"he"}\n\nevent: token\ndata: {"text":"llo"}\n\nevent: done\ndata: {}\n\n`;
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode(wire)), c.handlers);
    expect(c.events).toEqual(["citation", "token", "token", "done"]);
    expect(c.citations[0]?.[0]?.document_id).toBe("document:readme");
    expect(c.tokens.join("")).toBe("hello");
    expect(c.errors).toEqual([]);
  });

  it("handles chunks split mid-event, mid-line, and mid-multibyte-char", async () => {
    const wire =
      'event: citation\ndata: {"hits":[]}\n\nevent: token\ndata: {"text":"こんにちは"}\n\nevent: done\ndata: {}\n\n';
    const all = enc.encode(wire);
    const midEvent = enc.encode('event: citation\ndata: {"hits":[]}\n\nevent: token\n').length;
    const midLine = midEvent + enc.encode('data: {"text":"').length;
    const midChar = midLine + 1; // inside the 3-byte sequence of こ
    const c = collect();
    await consumeSseStream(
      sseResponse(
        all.subarray(0, midEvent),
        all.subarray(midEvent, midLine),
        all.subarray(midLine, midChar),
        all.subarray(midChar),
      ),
      c.handlers,
    );
    expect(c.events).toEqual(["citation", "token", "done"]);
    expect(c.tokens.join("")).toBe("こんにちは");
  });

  it("accepts CRLF line endings", async () => {
    const wire = 'event: token\r\ndata: {"text":"hi"}\r\n\r\nevent: done\r\ndata: {}\r\n\r\n';
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode(wire)), c.handlers);
    expect(c.events).toEqual(["token", "done"]);
    expect(c.tokens).toEqual(["hi"]);
  });

  it("ignores keep-alive comment lines", async () => {
    const wire = ': ping\nevent: token\ndata: {"text":"a"}\n\n: ping\n\nevent: done\ndata: {}\n\n';
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode(wire)), c.handlers);
    expect(c.events).toEqual(["token", "done"]);
    expect(c.errors).toEqual([]);
  });

  it("routes unknown event types to onError", async () => {
    const c = collect();
    await consumeSseStream(
      sseResponse(enc.encode('event: mystery\ndata: {"x":1}\n\n')),
      c.handlers,
    );
    expect(c.events).toEqual(["error"]);
    expect(c.errors[0]?.code).toBe("E_SSE_PROTOCOL");
    expect(c.errors[0]?.message).toContain("mystery");
    expect(c.tokens).toEqual([]);
  });

  it("treats a bare data event (default message type) as unknown", async () => {
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode('data: {"text":"x"}\n\n')), c.handlers);
    expect(c.events).toEqual(["error"]);
    expect(c.errors[0]?.code).toBe("E_SSE_PROTOCOL");
  });

  it("delivers in-band error events with code and message", async () => {
    const c = collect();
    await consumeSseStream(
      sseResponse(enc.encode('event: error\ndata: {"code":"E_DB","message":"boom"}\n\n')),
      c.handlers,
    );
    expect(c.errors).toEqual([{ code: "E_DB", message: "boom" }]);
    expect(c.events).toEqual(["error"]);
  });

  it("routes malformed payloads on known events to onError", async () => {
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode("event: token\ndata: not-json\n\n")), c.handlers);
    expect(c.events).toEqual(["error"]);
    expect(c.errors[0]?.code).toBe("E_SSE_PROTOCOL");
  });

  it("parses fields without a space after the colon", async () => {
    const c = collect();
    await consumeSseStream(
      sseResponse(enc.encode('event:token\ndata:{"text":"hi"}\n\n')),
      c.handlers,
    );
    expect(c.tokens).toEqual(["hi"]);
  });

  it("dispatches a final event terminated only by EOF", async () => {
    const c = collect();
    await consumeSseStream(sseResponse(enc.encode("event: done\ndata: {}")), c.handlers);
    expect(c.events).toEqual(["done"]);
  });

  it("stops cleanly when the signal aborts mid-stream", async () => {
    let releaseGate: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    let pulled = false;
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (!pulled) {
          pulled = true;
          controller.enqueue(enc.encode('event: token\ndata: {"text":"a"}\n\n'));
          return;
        }
        return gate.then(() => {
          controller.close();
        });
      },
    });
    const controller = new AbortController();
    const c = collect();
    const finished = consumeSseStream(new Response(body), c.handlers, controller.signal);
    await c.firstToken;
    controller.abort();
    releaseGate();
    await finished;
    expect(c.tokens).toEqual(["a"]);
    expect(c.errors).toEqual([]);
  });
});
