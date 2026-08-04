# AGENTS.md

Local-first knowledge base: SurrealDB (embedded SurrealKV) + BAAI/bge-m3 embeddings, exposed as MCP server and CLI.

## Layout

- `crates/skb-core/` — all logic (db, embed, tokenize, ingest, search, crud, graph). Public API: `KnowledgeBase` in `src/lib.rs`.
- `crates/skb-cli/` — `skb` binary (thin wrapper over skb-core).
- `crates/skb-mcp/` — `skb-mcp` binary (rmcp 3.0, stdio transport), launched via the npm package.
- `npm/` — meta package + `packages/<platform>/` (only `package.json` templates committed; CI injects binaries). `bin/skb-mcp.js` resolves the platform package at runtime.
- `schema/001_init.surql` — DB schema. `skills/` — agent skill. `spike/` — experiments, **excluded from workspace**, do not build.
- `SPECIFICATION.md` is the authoritative spec; update it when behavior changes. `CONTRIBUTING.md` holds dev conventions (kept current — follow it).

## GitHub Workflow

- Create a dedicated branch for each feature, bug fix, cleanup, or documentation task. Do not combine unrelated tasks in one branch.
- Commit incrementally as coherent work is completed. Prefer small, focused commits over one large end-of-task commit.
- Open exactly one pull request per task branch as soon as that independent task is complete. When multiple independent tasks are received, open each task's PR separately rather than waiting to bundle them at the end. The PR description must identify the task and summarize verification.
- After opening a PR, wait for the CodeRabbitAI review to be published before merging. Inspect both the review summary and every inline comment, including suppressed comments when available.
- Address valid CodeRabbitAI findings, or document the rationale for not addressing them, then re-run the relevant checks and request/wait for a follow-up review after any code, documentation, or configuration change.
- Do not merge a PR while its CodeRabbitAI review is pending unless the user explicitly authorizes an exception.

## Commands

- Build (fast, mock embeddings): `cargo build --workspace`
- Test: `cargo test --workspace -- --test-threads=1` — **serial execution required** (embedded SurrealKV).
- Real-embedding build: `cargo build --release -p skb-mcp --features ort` — first build takes ~15 min; ort-sys downloads ONNX Runtime to `~/.cache/ort.pyke.io`.
- `ort` feature only exists on `skb-core`; `skb-cli`/`skb-mcp` forward it (`--features ort` on those packages).
- Contract tests (`crates/skb-cli/tests/contract.rs`) spawn the real `target/debug/skb` binary; run via `cargo test`, not standalone.
- Benchmarks (mock): `cargo bench` — 4 groups (tokenize, embed, search, mcp_startup).
- Benchmarks (real bge-m3): `cargo bench --features ort -p skb-core --bench skb` — first run downloads bge-m3 ONNX model (~2.2 GB). No external deps needed (rustls-only).

## Toolchain workflow (cargo fmt / check / clippy / fix)

Use each tool for its own job, in this order when finishing a change:

1. `cargo check --workspace` — fast compile feedback while editing (no codegen).
2. `cargo clippy --workspace` — lint pass before considering work done; keep warning-free.
3. `cargo fix` / `cargo clippy --fix` — apply machine-fixable suggestions instead of hand-editing.
   - Requires a clean git tree; with uncommitted changes pass `--allow-dirty`.
   - Lib-target fixes may need explicit selection: `cargo fix --lib -p skb-core` (plain `--workspace` can skip them).
4. `cargo fmt --all` — normalize formatting last; verify with `cargo fmt --all -- --check`.
5. `cargo test --workspace -- --test-threads=1` — final verification.

## Hard constraints

- **Never write to `/tmp`.** Test DBs go under `./target/` (e.g. `target/skb-test-*`); runtime DB default is `~/.local/share/skb/db`.
- **TLS is rustls-only.** OpenSSL is not a build or runtime dep; `libssl-dev`/`pkg-config` are not needed. CI fails if `cargo tree -i openssl-sys` or `-i native-tls` matches. ort-sys downloader uses `tls-rustls` (opt-in via `default-features = false`).
- SurrealDB is embedded-only: `default-features = false, features = ["kv-surrealkv"]`. Remote mode is unimplemented (surrealdb 3.x lacks `From<Surreal<Db>> for Surreal<Any>`) — don't add protocol features back without discussion.
- Tokenizer crate is `tokenizers` (HF official). Do not switch to gigatoken — it doesn't build (unpublished, nightly-only).

## Config gotchas (`skb.toml`)

- Key is `[storage] path = "..."`, **not** `[database]` — wrong key silently falls back to default path.
- Search order: `./skb.toml` (cwd), then `~/.config/skb/config.toml`. Root `skb.toml` is gitignored.
- `onnx_path = "mock"` selects MockEmbedder (dim from config, tests use 8). There is **no** `tokenizer = "mock"` — `tokenizer` is a file path or `"auto"` (default: downloads bge-m3 `tokenizer.json` from HF, ~17MB, so even mock-mode first run needs network/HF cache).

## SurrealQL quirks (hard-won)

- Never select `id` / `document` fields directly. Use `meta::id()` or `string::concat('table:', meta::id(id))` for string-representable IDs.
- `value` / `val` are reserved words — field is named `meta_value`.
- Full-text search uses `FULLTEXT ANALYZER skb_text BM25` with the `class` tokenizer and `lowercase` filter; `ngram` is intentionally omitted because it degraded BM25 results.

## CI / packaging

- Linux build runners: ubuntu-24.04 (glibc 2.39 floor; pyke ONNX Runtime prebuilts require glibc >= 2.38). `CXXSTDLIB=""` (suppresses ort-sys `-lstdc++`) + `-C link-arg=-l:libstdc++.a` for static libstdc++. `libgcc_s` stays dynamic (not checked by smoke). Windows uses dynamic CRT (ORT prebuilt is `/MD`; binary depends on VC++ runtime DLLs, matching ORT's own runtime requirements). Verify with `ldd`: no libonnxruntime, libssl, libcrypto, libstdc++.
- macOS runner: `macos-latest` (arm64). Intel Mac (darwin-x64) is unsupported — ort has no x86_64-apple-darwin prebuilt.
- Release binaries are fully static-linked ONNX Runtime (ort `download-binaries`); no `.so` bundling. Ship `THIRD_PARTY_LICENSES.md` (ONNX Runtime MIT) in every npm package.
- npm publish flow not yet automated; registry publish resolves scoped platform packages (local `npm install <dir>` breaks symlinks — install tarballs instead).
