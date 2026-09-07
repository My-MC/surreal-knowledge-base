import { existsSync } from "node:fs";
import path from "node:path";

/** Walk up from `start` to the repo root (marker: Cargo.toml + web/). */
export function findRepoRoot(start: string = process.cwd()): string {
  let dir = path.resolve(start);
  for (;;) {
    if (existsSync(path.join(dir, "Cargo.toml")) && existsSync(path.join(dir, "web"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      throw new Error(`repo root not found walking up from ${start}`);
    }
    dir = parent;
  }
}

export const repoRoot = findRepoRoot();
export const evidenceDir = path.join(repoRoot, "target", "evidence", "15");
