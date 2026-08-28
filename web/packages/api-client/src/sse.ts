// SSE frame parser for POST /api/chat/stream (SPECIFICATION.md §20.3).
// EventSource is GET-only, so the stream is consumed over fetch + ReadableStream.
import type { components } from "./schema.gen";

export type SearchHit = components["schemas"]["SearchHit"];

export interface SseHandlers {
  onCitation: (hits: SearchHit[]) => void;
  onToken: (text: string) => void;
  onDone: () => void;
  onError: (code: string, message: string) => void;
}

const PROTOCOL_ERROR = "E_SSE_PROTOCOL";

export async function consumeSseStream(
  response: Response,
  handlers: SseHandlers,
  signal?: AbortSignal,
): Promise<void> {
  const body = response.body;
  if (body === null) throw new Error("SSE response has no body");
  if (signal?.aborted) return;

  const reader = body.getReader();
  const onAbort = () => {
    void reader.cancel().catch(() => {});
  };
  signal?.addEventListener("abort", onAbort, { once: true });

  const decoder = new TextDecoder();
  let buffer = "";
  let eventName: string | null = null;
  let dataLines: string[] = [];

  const dispatch = () => {
    if (eventName === null && dataLines.length === 0) return;
    const name = eventName ?? "message";
    const data = dataLines.join("\n");
    eventName = null;
    dataLines = [];
    dispatchEvent(name, data, handlers);
  };

  const handleLine = (line: string) => {
    if (line === "") {
      dispatch();
      return;
    }
    if (line.startsWith(":")) return;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") eventName = value;
    else if (field === "data") dataLines.push(value);
  };

  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let nl = buffer.indexOf("\n");
      while (nl !== -1) {
        handleLine(buffer.slice(0, nl).replace(/\r$/, ""));
        buffer = buffer.slice(nl + 1);
        nl = buffer.indexOf("\n");
      }
    }
    if (signal?.aborted) return;
    buffer += decoder.decode();
    if (buffer !== "") handleLine(buffer.replace(/\r$/, ""));
    dispatch();
  } catch (e) {
    if (signal?.aborted) return;
    throw e;
  } finally {
    signal?.removeEventListener("abort", onAbort);
  }
}

function dispatchEvent(name: string, data: string, handlers: SseHandlers): void {
  switch (name) {
    case "citation": {
      const hits = asRecord(parseJson(data))?.hits;
      if (Array.isArray(hits)) handlers.onCitation(hits);
      else handlers.onError(PROTOCOL_ERROR, "malformed citation payload");
      break;
    }
    case "token": {
      const text = asRecord(parseJson(data))?.text;
      if (typeof text === "string") handlers.onToken(text);
      else handlers.onError(PROTOCOL_ERROR, "malformed token payload");
      break;
    }
    case "done":
      handlers.onDone();
      break;
    case "error": {
      const record = asRecord(parseJson(data));
      const code = record?.code;
      const message = record?.message;
      if (typeof code === "string" && typeof message === "string") handlers.onError(code, message);
      else handlers.onError(PROTOCOL_ERROR, "malformed error payload");
      break;
    }
    default:
      handlers.onError(PROTOCOL_ERROR, `unknown event type: ${name}`);
  }
}

function parseJson(data: string): unknown {
  try {
    return JSON.parse(data) as unknown;
  } catch {
    return null;
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}
