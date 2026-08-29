/**
 * Self-contained SSE smoke check for the chat stack (plan todo 16).
 *
 * Spawns its own mock_llm and skb-server (both `--port 0`, machine-parsed
 * stdout port protocols), seeds a document containing a unique term, then
 * POSTs /api/chat/stream and asserts the SSE event order: citation (hits
 * non-empty) → token+ → done. Exits 0 on success; non-zero with diagnostics
 * on any failure. No external deps, no manual steps.
 *
 * Run: bun web/e2e/scripts/sse-smoke.ts
 */
import { execSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { killAll, spawnDetached, waitForHttp, waitForPortLine } from "./proc";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const SERVER_BIN = path.join(repoRoot, "target", "debug", "skb-server");
const MOCK_LLM_BIN = path.join(repoRoot, "target", "debug", "examples", "mock_llm");
const DB_PATH = path.join(repoRoot, "target", "skb-smoke-db");

const SERVER_START_TIMEOUT_MS = 60_000; // ~8s typical startup; generous bound
const CHILD_START_TIMEOUT_MS = 30_000;
const STREAM_TIMEOUT_MS = 30_000;
const OVERALL_TIMEOUT_MS = 180_000;

interface SseEvent {
  event: string;
  data: string;
}

function fail(message: string): never {
  throw new Error(message);
}

/** Reads an SSE body to completion, returning events in arrival order. */
async function readSseEvents(response: Response, timeoutMs: number): Promise<SseEvent[]> {
  const body = response.body;
  if (body === null) fail("chat stream response has no body");
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
    if (Date.now() > deadline) fail(`SSE stream did not finish within ${timeoutMs}ms`);
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
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

async function main(): Promise<void> {
  for (const bin of [SERVER_BIN, MOCK_LLM_BIN]) {
    if (existsSync(bin)) continue;
    console.log(
      `[smoke] ${bin} missing — building: cargo build -p skb-server --bin skb-server --examples`,
    );
    execSync("cargo build -p skb-server --bin skb-server --examples", {
      cwd: repoRoot,
      stdio: "inherit",
    });
  }
  if (!existsSync(SERVER_BIN) || !existsSync(MOCK_LLM_BIN)) {
    fail("binaries still missing after guarded build");
  }

  rmSync(DB_PATH, { recursive: true, force: true });

  const mock = spawnDetached(MOCK_LLM_BIN, ["--port", "0"]);
  const mockPort = await waitForPortLine(mock, "MOCK_LLM_PORT", CHILD_START_TIMEOUT_MS);
  console.log(`[smoke] mock_llm on :${mockPort}`);
  await waitForHttp(`http://127.0.0.1:${mockPort}/v1/chat/completions`, CHILD_START_TIMEOUT_MS);

  const server = spawnDetached(SERVER_BIN, ["--port", "0"], {
    env: {
      ...process.env,
      SKB_STORAGE_PATH: DB_PATH,
      SKB_EMBEDDING_ONNX_PATH: "mock",
      SKB_EMBEDDING_DIMENSION: "8",
      SKB_EMBEDDING_TOKENIZER: "auto",
      SKB_EMBEDDING_MODEL: "BAAI/bge-m3",
      SKB_SERVER_HOST: "127.0.0.1",
      SKB_SERVER_JWT_SECRET: "skb-smoke-secret",
      SKB_LLM_BASE_URL: `http://127.0.0.1:${mockPort}/v1`,
    },
  });
  const serverPort = await waitForPortLine(server, "SKB_SERVER_PORT", CHILD_START_TIMEOUT_MS);
  console.log(`[smoke] skb-server on :${serverPort}`);
  await waitForHttp(`http://127.0.0.1:${serverPort}/api/health`, SERVER_START_TIMEOUT_MS);

  const uniqueTerm = `SKBSmoke${Date.now()}`;
  const seedResponse = await fetch(`http://127.0.0.1:${serverPort}/api/documents`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      content: `# ${uniqueTerm}\n\nThe ${uniqueTerm} term appears in this smoke-test document so the chat pipeline can cite it.`,
      title: uniqueTerm,
    }),
  });
  if (!seedResponse.ok) {
    fail(`seeding document failed: HTTP ${seedResponse.status} ${await seedResponse.text()}`);
  }
  console.log(`[smoke] seeded document containing ${uniqueTerm}`);

  const chatResponse = await fetch(`http://127.0.0.1:${serverPort}/api/chat/stream`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ message: uniqueTerm }),
  });
  if (!chatResponse.ok) {
    fail(`chat stream request failed: HTTP ${chatResponse.status}`);
  }
  const events = await readSseEvents(chatResponse, STREAM_TIMEOUT_MS);
  console.log(`[smoke] SSE events: ${events.map((e) => e.event).join(",")}`);

  const errors = events.filter((e) => e.event === "error");
  if (errors.length > 0) {
    fail(`in-band error events: ${errors.map((e) => e.data).join(" | ")}`);
  }
  const citationIndex = events.findIndex((e) => e.event === "citation");
  if (citationIndex === -1) fail("no citation event arrived");
  const citationHits = JSON.parse(events[citationIndex]?.data ?? "{}") as { hits?: unknown[] };
  if (!Array.isArray(citationHits.hits) || citationHits.hits.length === 0) {
    fail(`citation event has no hits: ${events[citationIndex]?.data}`);
  }
  const firstTokenIndex = events.findIndex((e) => e.event === "token");
  if (firstTokenIndex === -1 || firstTokenIndex < citationIndex) {
    fail("no token event after the citation event");
  }
  const doneIndex = events.findIndex((e) => e.event === "done");
  if (doneIndex === -1 || doneIndex < firstTokenIndex) {
    fail("no done event after the token events");
  }
  const tokenCount = events.filter((e) => e.event === "token").length;
  console.log(
    `[smoke] OK: citation(${citationHits.hits.length} hits) → ${tokenCount} tokens → done, in order`,
  );
}

const overallTimer = setTimeout(() => {
  console.error(`[smoke] FAIL: overall timeout after ${OVERALL_TIMEOUT_MS}ms`);
  void killAll().then(() => process.exit(1));
}, OVERALL_TIMEOUT_MS);

try {
  await main();
  clearTimeout(overallTimer);
  await killAll();
  console.log("[smoke] PASS");
  process.exit(0);
} catch (error) {
  clearTimeout(overallTimer);
  console.error(`[smoke] FAIL: ${error instanceof Error ? error.message : String(error)}`);
  await killAll();
  process.exit(1);
}
