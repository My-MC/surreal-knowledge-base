/**
 * Child-process lifecycle helpers for the e2e scripts: detached spawns with
 * their own process groups, stdout port-line parsing, HTTP readiness polls,
 * and group teardown with output-tail diagnostics.
 */
import { type ChildProcess, spawn } from "node:child_process";
import path from "node:path";

const children: ChildProcess[] = [];
const tails = new Map<ChildProcess, string[]>();

export function describeChildren(): string {
  return children
    .map((child) => `--- ${path.basename(child.spawnfile)} ---\n${tailOf(child)}`)
    .join("\n");
}

function tailOf(child: ChildProcess): string {
  return (tails.get(child) ?? []).join("").trimEnd();
}

export function spawnDetached(
  command: string,
  args: string[],
  options: { env: NodeJS.ProcessEnv; cwd?: string } = { env: process.env },
): ChildProcess {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    // Own process group so teardown kills the whole tree.
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(child);
  tails.set(child, []);
  const drain = (stream: NodeJS.ReadableStream | null) => {
    stream?.on("data", (chunk: Buffer) => {
      const tail = tails.get(child) ?? [];
      tail.push(chunk.toString());
      if (tail.length > 10) tail.shift();
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

/** Resolves with the first stdout line matching /KEY=(\d+)/. */
export function waitForPortLine(
  child: ChildProcess,
  key: string,
  timeoutMs: number,
): Promise<number> {
  return new Promise((resolve, reject) => {
    let buffer = "";
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`${key}=<n> not printed within ${timeoutMs}ms; stdout: ${buffer}`));
    }, timeoutMs);
    const onData = (chunk: Buffer) => {
      buffer += chunk.toString();
      const match = buffer.match(new RegExp(`^${key}=(\\d+)`, "m"));
      if (match?.[1] !== undefined) {
        cleanup();
        resolve(Number(match[1]));
      }
    };
    const cleanup = () => {
      clearTimeout(timer);
      child.stdout?.off("data", onData);
    };
    child.stdout?.on("data", onData);
  });
}

export async function waitForHttp(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      await fetch(url);
      return;
    } catch {
      // not accepting connections yet
    }
    if (Date.now() > deadline) {
      throw new Error(`not ready at ${url} within ${timeoutMs}ms\n${describeChildren()}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}

function isAlive(child: ChildProcess): boolean {
  return child.pid !== undefined && child.exitCode === null && child.signalCode === null;
}

/** SIGTERM to every process group, then SIGKILL after a grace period. */
export async function killAll(graceMs = 5_000): Promise<void> {
  for (const child of [...children].reverse()) {
    if (!isAlive(child) || child.pid === undefined) continue;
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      // already gone
    }
  }
  const deadline = Date.now() + graceMs;
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
