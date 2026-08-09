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

const child = spawn(bin, [], { stdio: ["pipe", "pipe", "inherit"] });
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
const pending = new Map();
let nextId = 1;

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

function assert(cond, message) {
  if (!cond) {
    console.error("FAIL: " + message);
    child.kill();
    process.exit(1);
  }
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

const up = await request("tools/call", {
  name: "skb_upload",
  arguments: {
    content: "Smoke test document about HNSW vector search and BM25.",
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
  arguments: { query: "HNSW", mode: "keyword", top_k: 5 },
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
  (searchJson.hits ?? []).some((h) => h.title === "smoke-doc"),
  "skb_search must find the uploaded document",
);

console.log("SMOKE OK");
// Wait for the child to exit so its SurrealKV file handles are released
// before the job moves on; fall back to exiting after 5s.
child.once("exit", () => process.exit(0));
child.kill();
setTimeout(() => process.exit(0), 5000).unref();
