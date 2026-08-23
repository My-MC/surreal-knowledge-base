#!/usr/bin/env node
// MCP smoke flow used by CI (npm smoke + per-target E2E):
// initialize -> tools/list -> skb_upload -> skb_search, each request
// awaited before the next is sent (rmcp handles requests concurrently,
// so piped batches would race). Exits non-zero on any assertion failure.
//
// Usage: node smoke-mcp.mjs <path-to-skb-mcp-binary>
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const bin = process.argv[2];
if (!bin) {
  console.error("usage: node smoke-mcp.mjs <skb-mcp binary>");
  process.exit(2);
}

const pending = new Map();
let nextId = 1;

// childRef is null until the child is spawned; a synchronous module
// evaluation failure before that must still exit with the original
// failure reason instead of tripping over the uninitialized child const.
let childRef = null;

// Registered before any await so an assertion failure anywhere in the flow
// reaches the top-level failure path (terminate child, wait for exit,
// exit non-zero) instead of dying with an unhandled rejection.
process.on("uncaughtException", (err) => {
  console.error("FAIL: " + (err?.message ?? err));
  shutdown(1);
});

const child = spawn(bin, [], { stdio: ["pipe", "pipe", "inherit"] });
childRef = child;
child.on("error", (err) => {
  for (const [id, { reject, timer }] of pending) {
    clearTimeout(timer);
    pending.delete(id);
    reject(new Error(`cannot start MCP binary '${bin}': ${err.message} (request ${id})`));
  }
});
const rl = createInterface({ input: child.stdout });
// Swallow EPIPE from stdin writes after the child exits; the exit/error
// handlers reject pending requests with the real failure reason.
child.stdin.on("error", () => {});

// If the child exits before responding (crash, bad config, etc.), fail all
// in-flight requests immediately with the exit code instead of timing out.
child.on("exit", (code, signal) => {
  for (const [id, { reject }] of pending) {
    clearTimeout(pending.get(id).timer);
    pending.delete(id);
    reject(
      new Error(`MCP subprocess exited early (code=${code}, signal=${signal}) while waiting for request ${id}`)
    );
  }
});

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.id !== undefined && pending.has(msg.id)) {
    const entry = pending.get(msg.id);
    clearTimeout(entry.timer);
    pending.delete(msg.id);
    entry.resolve(msg);
  }
});

function request(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error(`timeout waiting for response to ${method}`));
      }
    }, 30000);
    pending.set(id, { resolve, reject, timer });
    const msg = { jsonrpc: "2.0", id, method, params };
    child.stdin.write(JSON.stringify(msg) + "\n");
  });
}

function notify(method, params) {
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
}

// assert reports the failure and hands control to the top-level failure path,
// which terminates the child and waits for its exit (releasing SurrealKV file
// locks) before ending the parent with status 1. Throwing immediately keeps
// failed checks from continuing to schedule child shutdown.
function assert(cond, message) {
  if (!cond) {
    throw new Error(message);
  }
}

function shutdown(code) {
  // A synchronous module-evaluation failure before the child is spawned must
  // still exit with the original failure reason.
  if (childRef === null) {
    process.exit(code);
  }
  // If the child already exited, "exit" never fires again and the unref'd
  // timer below cannot keep the loop alive; exit with the intended code.
  process.exitCode = code;
  if (childRef.exitCode !== null || childRef.signalCode !== null) {
    process.exit(code);
  }
  // Wait for the child to exit so its SurrealKV file handles are released
  // before the job moves on; fall back to exiting after 5s.
  childRef.once("exit", () => process.exit(code));
  childRef.kill();
  setTimeout(() => process.exit(code), 5000).unref();
}

const INIT = {
  protocolVersion: "2025-03-26",
  capabilities: {},
  clientInfo: { name: "skb-smoke", version: "1.0" },
};

const initResp = await request("initialize", INIT);
assert(initResp.result && initResp.result.protocolVersion, "initialize handshake");
notify("notifications/initialized", {});

const tools = await request("tools/list", {});
const toolNames = (tools.result?.tools ?? []).map((t) => t.name);
assert(toolNames.includes("skb_upload"), "tools/list must list skb_upload");
assert(toolNames.includes("skb_search"), "tools/list must list skb_search");

// Unique content per run so an existing store never dedupes the upload to
// "skipped" (and the search assertion still finds the document). The run
// token makes the search assertion match only THIS run's upload, not a
// smoke-doc left by an earlier run in the default database.
const runToken = `smoketoken${Date.now()}x${process.pid}`;
const uniqueContent =
  `Smoke test document about HNSW vector search and BM25. ${runToken}`;

const up = await request("tools/call", {
  name: "skb_upload",
  arguments: {
    content: uniqueContent,
    title: "smoke-doc",
  },
});
assert(!up.result?.isError, "skb_upload must succeed");
const upText = up.result?.content?.[0]?.text ?? "";
let upJson;
try {
  upJson = JSON.parse(upText);
} catch {
  assert(false, `skb_upload must return JSON, got: ${upText}`);
}
assert(upJson.status === "created", "skb_upload must create the document");
assert(upJson.title === "smoke-doc", "skb_upload must echo the title");

const search = await request("tools/call", {
  name: "skb_search",
  // Older smoke docs share the "smoketoken" prefix after tokenization, so
  // request a larger window than a single leftover run.
  arguments: { query: runToken, mode: "keyword", top_k: 50 },
});
assert(!search.result?.isError, "skb_search must succeed");
const searchText = search.result?.content?.[0]?.text ?? "";
let searchJson;
try {
  searchJson = JSON.parse(searchText);
} catch {
  assert(false, `skb_search must return JSON, got: ${searchText}`);
}
assert(
  (searchJson.hits ?? []).some((h) => h.content?.includes(runToken)),
  "skb_search must find the uploaded document",
);

console.log("SMOKE OK");
shutdown(0);
