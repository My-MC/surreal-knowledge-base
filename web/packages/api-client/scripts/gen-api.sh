#!/usr/bin/env bash
# Generate src/schema.gen.ts from the live skb-server OpenAPI document
# (plan todo 10). Pipeline:
#
#   1. recreate target/api-gen/ with an isolated skb.toml (mock embeddings,
#      DB strictly under the repo's target/ — never /tmp, never the real
#      ~/.local/share/skb default, never a DB another process owns)
#   2. start skb-server on an ephemeral port with cwd = target/api-gen so it
#      reads that skb.toml, and wait for its stdout line SKB_SERVER_PORT=<n>
#   3. run openapi-typescript against http://127.0.0.1:$PORT/api/openapi.json
#   4. stop the server (trap guarantees this even on failure)
#
# Idempotent: the gen dir is wiped on every run. Fails loudly if the server
# does not come up.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$PKG_DIR/../../.." && pwd)"
GEN_DIR="$REPO_ROOT/target/api-gen"
DB_DIR="$GEN_DIR/db"

command -v cargo >/dev/null || { echo "error: cargo not found" >&2; exit 1; }
command -v bunx >/dev/null || { echo "error: bunx not found" >&2; exit 1; }

rm -rf "$GEN_DIR"
mkdir -p "$GEN_DIR"

# Absolute storage path: cwd-independent and always under the repo's target/.
# (skb-core resolves storage.path against the process cwd; an absolute path
# removes that ambiguity entirely.)
cat > "$GEN_DIR/skb.toml" <<EOF
[storage]
path = "$DB_DIR"

[embedding]
onnx_path = "mock"
dimension = 8
EOF

cd "$GEN_DIR"
echo "[gen-api] skb.toml:"
cat skb.toml
echo "[gen-api] starting skb-server (ephemeral port, mock embeddings)..."

# setsid: own process group so the cleanup kill takes down cargo AND the
# server binary (cargo run does not reliably forward SIGTERM to its child).
setsid cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -p skb-server --bin skb-server -- --port 0 \
  > "$GEN_DIR/server.log" 2>&1 &
SERVER_PID=$!

cleanup() {
  kill -- "-$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# stdout protocol (verified in plan todo 2): with --port 0 the server prints
# exactly one line SKB_SERVER_PORT=<n> once the DB is open and it is bound.
PORT=""
for _ in $(seq 1 600); do # 300s: cold KB open ~8s; a cold cargo rebuild of the dep tree can take minutes
  PORT="$(sed -n 's/^SKB_SERVER_PORT=//p' "$GEN_DIR/server.log" 2>/dev/null | head -1)"
  if [ -n "$PORT" ]; then
    break
  fi
  kill -0 "$SERVER_PID" 2>/dev/null || break
  sleep 0.5
done
if [ -z "$PORT" ]; then
  echo "error: skb-server did not report SKB_SERVER_PORT; server.log:" >&2
  cat "$GEN_DIR/server.log" >&2
  exit 1
fi
echo "[gen-api] skb-server up on 127.0.0.1:$PORT"

# The workspace typescript is 7.0.2 (Go-native, no JS API); openapi-typescript
# 7 embeds the classic ts.factory compiler API and needs typescript@5.x, so the
# generator runs under its own bunx-resolved copy. Keep the openapi-typescript
# pin in sync with package.json devDependencies.
(
  cd "$PKG_DIR"
  bunx --package typescript@5.9.3 --package openapi-typescript@7.13.0 \
    openapi-typescript "http://127.0.0.1:$PORT/api/openapi.json" -o src/schema.gen.ts
  # openapi-typescript emits 4-space indent; normalize to the workspace biome style.
  bunx biome format --write src/schema.gen.ts
)

echo "[gen-api] wrote $PKG_DIR/src/schema.gen.ts"
echo "[gen-api] stopping skb-server"
