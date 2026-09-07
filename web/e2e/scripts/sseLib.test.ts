import { describe, expect, test } from "bun:test";
import { readSseEvents } from "./sseLib";

describe("readSseEvents", () => {
  /// Given: a 200 response that sent headers but whose body never emits.
  /// When:  reading with a short deadline.
  /// Then:  the read rejects with the deadline error and cancels the body —
  ///        instead of pending forever, which is what kept the smoke
  ///        runner's killAll() cleanup from ever running.
  test("a headers-only body times out and is cancelled instead of hanging", async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start() {
        // Headers sent; body deliberately left open.
      },
      cancel() {
        cancelled = true;
      },
    });
    const response = new Response(body, { status: 200 });
    const started = Date.now();
    try {
      await readSseEvents(response, 250);
      throw new Error("readSseEvents should have timed out");
    } catch (error) {
      expect(String(error)).toContain("did not finish within 250ms");
    }
    expect(Date.now() - started).toBeLessThan(5_000);
    expect(cancelled).toBe(true);
  });
});
