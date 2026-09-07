/**
 * SSE body reader for the chat smoke check, extracted from sse-smoke.ts so
 * the deadline behavior can be regression-tested (the smoke script spawns
 * processes at import time and must stay side-effect only in its own file).
 */

export interface SseEvent {
  event: string;
  data: string;
}

async function cancelQuietly(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
  // The body is already dead — the deadline error is what matters; a cancel
  // on an aborted stream is best-effort only.
  try {
    await reader.cancel();
  } catch {
    // ignore: nothing to recover here
  }
}

/** Reads an SSE body to completion, returning events in arrival order. */
export async function readSseEvents(response: Response, timeoutMs: number): Promise<SseEvent[]> {
  const body = response.body;
  if (body === null) throw new Error("chat stream response has no body");
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const events: SseEvent[] = [];
  let buffer = "";
  let eventName: string | null = null;
  let dataLines: string[] = [];
  const deadline = Date.now() + timeoutMs;

  const dispatch = () => {
    if (eventName === null && dataLines.length === 0) return;
    events.push({ event: eventName ?? "message", data: dataLines.join("\n") });
    eventName = null;
    dataLines = [];
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

  for (;;) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) {
      await cancelQuietly(reader);
      throw new Error(`SSE stream did not finish within ${timeoutMs}ms`);
    }
    // A read that never resolves (headers sent, body stalled) must not hang
    // the runner: race it against the deadline and cancel the body on expiry
    // so the caller's killAll cleanup path can proceed.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const deadlineHit = new Promise<never>((_, reject) => {
      timer = setTimeout(
        () => reject(new Error(`SSE stream did not finish within ${timeoutMs}ms`)),
        remaining,
      );
    });
    let chunk: ReadableStreamReadResult<Uint8Array>;
    try {
      chunk = await Promise.race([reader.read(), deadlineHit]);
    } catch (error) {
      await cancelQuietly(reader);
      throw error;
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
    if (chunk.done) break;
    buffer += decoder.decode(chunk.value, { stream: true });
    let nl = buffer.indexOf("\n");
    while (nl !== -1) {
      handleLine(buffer.slice(0, nl).replace(/\r$/, ""));
      buffer = buffer.slice(nl + 1);
      nl = buffer.indexOf("\n");
    }
    if (events.some((e) => e.event === "done" || e.event === "error")) break;
  }
  dispatch();
  return events;
}
