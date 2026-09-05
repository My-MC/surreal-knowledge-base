import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { repoRoot } from "./helpers.mts";

const SERVER_PORT = 18080;
const MOCK_LLM_PORT = 18081;
const DEV_PORT = 5173;
const STUDIO_DEV_PORT = 5174;
const BLOG_DEV_PORT = 5175;
const DB_PATH = path.join(repoRoot, "target", "skb-e2e-db");
const SERVER_BIN = path.join(repoRoot, "target", "debug", "skb-server");
const MOCK_LLM_BIN = path.join(repoRoot, "target", "debug", "examples", "mock_llm");
const VAULT_APP_DIR = path.join(repoRoot, "web", "apps", "vault");
const STUDIO_APP_DIR = path.join(repoRoot, "web", "apps", "studio");
const BLOG_APP_DIR = path.join(repoRoot, "web", "apps", "blog");

const SERVER_START_TIMEOUT_MS = 120_000;
const CHILD_START_TIMEOUT_MS = 30_000;
const TERM_GRACE_MS = 5_000;

const children: ChildProcess[] = [];
const labels = new Map<ChildProcess, string>();
const tails = new Map<ChildProcess, string[]>();

function spawnDetached(
  label: string,
  command: string,
  args: string[],
  options: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
): ChildProcess {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env ?? process.env,
    // Own process group: teardown kills the whole tree (a `bun run` wrapper
    // does not reliably forward signals to its child, T10 cargo-run lesson).
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(child);
  labels.set(child, label);
  tails.set(child, []);
  const drain = (stream: NodeJS.ReadableStream | null) => {
    stream?.on("data", (chunk: Buffer) => {
      const tail = tails.get(child) ?? [];
      tail.push(chunk.toString());
      if (tail.length > 5) tail.shift();
      tails.set(child, tail);
    });
  };
  drain(child.stdout);
  drain(child.stderr);
  child.on("error", (error) => {
    tails.get(child)?.push(`spawn error: ${String(error)}`);
  });
  return child;
}

function tailOf(child: ChildProcess): string {
  return (tails.get(child) ?? []).join("").trimEnd();
}

async function waitForHttp(
  label: string,
  url: string,
  expectedStatus: number | null,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const response = await fetch(url);
      if (expectedStatus === null || response.status === expectedStatus) {
        return;
      }
    } catch {
      // not accepting connections yet
    }
    if (Date.now() > deadline) {
      const logs = children
        .map((child) => `--- ${labels.get(child) ?? "child"} ---\n${tailOf(child)}`)
        .join("\n");
      throw new Error(`${label} not ready at ${url} within ${timeoutMs}ms\n${logs}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}

function isAlive(child: ChildProcess): boolean {
  return child.pid !== undefined && child.exitCode === null && child.signalCode === null;
}

async function killAll(): Promise<void> {
  for (const child of [...children].reverse()) {
    if (!isAlive(child) || child.pid === undefined) continue;
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      // already gone
    }
  }
  const deadline = Date.now() + TERM_GRACE_MS;
  for (;;) {
    if (children.every((child) => !isAlive(child)) || Date.now() > deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  for (const child of children) {
    if (!isAlive(child) || child.pid === undefined) continue;
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      // already gone
    }
  }
}

/**
 * Spawns the e2e stack: mock_llm (prebuilt example binary), skb-server
 * (prebuilt binary, mock embeddings, wiped throwaway DB, fixed ports) and the
 * vault + studio + blog dev servers proxying to it. All server binaries must be
 * built up front (`cargo build -p skb-server --bin skb-server --examples`) —
 * cargo is deliberately not invoked from inside Playwright.
 */
export default async function globalSetup(): Promise<() => Promise<void>> {
  for (const bin of [SERVER_BIN, MOCK_LLM_BIN]) {
    if (!existsSync(bin)) {
      throw new Error(
        `${bin} not found — run: cargo build -p skb-server --bin skb-server --examples`,
      );
    }
  }
  rmSync(DB_PATH, { recursive: true, force: true });

  spawnDetached("mock_llm", MOCK_LLM_BIN, ["--port", String(MOCK_LLM_PORT)]);
  await waitForHttp(
    "mock_llm",
    `http://127.0.0.1:${MOCK_LLM_PORT}/v1/chat/completions`,
    null,
    CHILD_START_TIMEOUT_MS,
  );

  spawnDetached("skb-server", SERVER_BIN, ["--port", String(SERVER_PORT)], {
    env: {
      ...process.env,
      SKB_STORAGE_PATH: DB_PATH,
      SKB_EMBEDDING_ONNX_PATH: "mock",
      SKB_EMBEDDING_DIMENSION: "8",
      SKB_EMBEDDING_TOKENIZER: "auto",
      SKB_EMBEDDING_MODEL: "BAAI/bge-m3",
      SKB_SERVER_HOST: "127.0.0.1",
      // Auth endpoints 503 E_CONFIG without it (todo 7 semantics). The
      // 32+ char floor rejects weak secrets (503), so keep it long.
      SKB_SERVER_JWT_SECRET: "skb-e2e-secret-0123456789abcdef-0123456789abcdef",
      // Dynamic per-run emails (seeder<b>ts</b>@example.com) — the @ form
      // grants the whole domain at registration (server-side allowlist).
      SKB_SERVER_AUTHOR_EMAILS: "@example.com",
      SKB_LLM_BASE_URL: `http://127.0.0.1:${MOCK_LLM_PORT}/v1`,
    },
  });
  await waitForHttp(
    "skb-server",
    `http://127.0.0.1:${SERVER_PORT}/api/health`,
    200,
    SERVER_START_TIMEOUT_MS,
  );

  spawnDetached(
    "vite",
    "bun",
    ["--bun", "run", "dev", "--port", String(DEV_PORT), "--strictPort"],
    {
      cwd: VAULT_APP_DIR,
      env: { ...process.env, SKB_SERVER_PORT: String(SERVER_PORT) },
    },
  );
  await waitForHttp(
    "vault dev server",
    `http://localhost:${DEV_PORT}/`,
    null,
    CHILD_START_TIMEOUT_MS,
  );

  spawnDetached(
    "studio vite",
    "bun",
    ["--bun", "run", "dev", "--port", String(STUDIO_DEV_PORT), "--strictPort"],
    {
      cwd: STUDIO_APP_DIR,
      env: { ...process.env, SKB_SERVER_PORT: String(SERVER_PORT) },
    },
  );
  await waitForHttp(
    "studio dev server",
    `http://localhost:${STUDIO_DEV_PORT}/`,
    null,
    CHILD_START_TIMEOUT_MS,
  );

  spawnDetached(
    "blog vite",
    "bun",
    ["--bun", "run", "dev", "--port", String(BLOG_DEV_PORT), "--strictPort"],
    {
      cwd: BLOG_APP_DIR,
      env: { ...process.env, SKB_SERVER_PORT: String(SERVER_PORT) },
    },
  );
  await waitForHttp(
    "blog dev server",
    `http://localhost:${BLOG_DEV_PORT}/`,
    null,
    CHILD_START_TIMEOUT_MS,
  );

  return async () => {
    await killAll();
  };
}
