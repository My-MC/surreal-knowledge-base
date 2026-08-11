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

// Dedicated store under the repository's target directory (absolute path, so
// it is independent of the process working directory; SKB_STORAGE_PATH is the
// env override handled by Config::load).
const repoRoot = new URL("..", import.meta.url).pathname;
const dbPath = `${repoRoot}target/skb-smoke-db-${process.pid}`;
const child = spawn(bin, [], {
  stdio: ["pipe", "pipe", "inherit"],
  env: { ...process.env, SKB_STORAGE_PATH: dbPath },
});
const rl = createInterface({ input: child.stdout });
const pending = new Map();
let nextId = 1;

// Every failure path must terminate the child before exiting so its
// SurrealKV file handles are released (the smoke DB is under ./target).
let shuttingDown = false;
function fail(message) {
  console.error("FAIL: " + message);
  shuttingDown = true;
  try {
    child.kill();
  } catch {}
  process.exit(1);
}

child.on("exit", () => {
  shuttingDown = true;
});

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.id !== undefined && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
});

function request(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, resolve);
    const msg = { jsonrpc: "2.0", id, method, params };
    child.stdin.write(JSON.stringify(msg) + "\n");
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        if (!shuttingDown) {
          child.kill();
        }
        reject(new Error(`timeout waiting for response to ${method}`));
      }
    }, 30000);
  });
}

function notify(method, params) {
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
}

function assert(cond, message) {
  if (!cond) {
    fail(message);
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
assert(searchText.includes("smoke-doc"), "skb_search must find the uploaded document");

console.log("SMOKE OK");
shuttingDown = true;
child.kill();
process.exit(0);
