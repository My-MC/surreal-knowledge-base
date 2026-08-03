# Development Conventions

## Branches, Commits, and Pull Requests

- Use one dedicated branch per feature, bug fix, cleanup, or documentation task. Keep unrelated work on separate branches.
- Make commits incrementally at coherent checkpoints. Commits should be focused and reviewable; do not wait until the entire task is finished to create the only commit.
- Open one GitHub pull request for each task branch. Keep the PR scope aligned with the branch scope and include verification results in the description.
- After opening a PR, wait for the CodeRabbitAI review before merging. Review the overview, review state, inline comments, and suppressed comments when available.
- Resolve valid CodeRabbitAI findings or record why a finding is not applicable. If code changes are made in response, rerun checks and wait for the updated CodeRabbitAI review before merging.
- Do not merge while the CodeRabbitAI review is pending unless the user explicitly approves an exception.

## File System Access

- Do not write test data or runtime storage to `/tmp`. 
- Use project-local paths: `./target/skb-test-{n}` for test databases, `~/.local/share/skb/db` for production storage.
- All `/tmp` references must be avoided; the project is self-contained.

## Build

- Default build: `cargo build --workspace` (uses MockEmbedder, no ONNX Runtime required)
- With real embeddings: `cargo build --workspace --features ort -p skb-mcp`
  - ort 2.0-rc statically links ONNX Runtime via `download-binaries` (pyke.io prebuilt static libs).
  - No `libonnxruntime.so` needed at runtime — the binary is self-contained.
- ONNX Runtime prebuilt dists are cached at `~/.cache/ort.pyke.io` (auto-downloaded by ort-sys build.rs).
- Workspace profile strips release symbols automatically (`strip = "symbols"`).
- Tests: `cargo test --workspace -- --test-threads=1` (SurrealKV embedded mode requires serial execution).

## Config

- Project-local: `./skb.toml` (highest priority)
- User-global: `~/.config/skb/config.toml`
- Embedding: set `onnx_path = "mock"` for testing without ONNX model download

## TLS

- All TLS paths use rustls: hf-hub → reqwest 0.13 (aws-lc-rs + platform-verifier), ureq 3 (ring + webpki-roots embedded CAs).
- OpenSSL is NOT a build or runtime dependency. `pkg-config` / `libssl-dev` are not required.
- CI enforces this via `cargo tree -i openssl-sys` / `cargo tree -i native-tls` guard steps.

## Runtime Dependencies (Linux, ort-enabled binary)

- glibc >= 2.35 (build runner: ubuntu-22.04)
- libz, libzstd (from ONNX Runtime prebuilt static lib)
- ca-certificates (for hf-hub model download; ureq uses embedded webpki-roots)
- libstdc++ is statically linked (RUSTFLAGS: -static-libstdc++ -static-libgcc)

## SurrealDB Versions

- Using surrealdb 3.x with `default-features = false, features = ["kv-surrealkv"]` (embedded mode only)
- Remote mode is not yet implemented (no `From<Surreal<Db>> for Surreal<Any>` in surrealdb 3.x)
- Record IDs: always use `meta::id()` / `string::concat('table:', meta::id(id))` 
  to get string-representable IDs. Never select `id` or `document` fields directly.
- Field names: avoid SurrealQL reserved words (`value`, `val`). Use `meta_value`.

## Tokenizer

- Using `tokenizers` crate (HuggingFace official), NOT gigatoken.
  Gigatoken failed to build: crates.io unpublished, requires nightly Rust + `profile-rustflags`.
- Tokenizer downloads from HuggingFace Hub on first use (~17MB).

## Embedding

- Default: MockEmbedder (returns deterministic vectors based on index)
- Real: `OrtEmbedder` behind `ort` feature flag. Requires `BAAI/bge-m3` ONNX model (~2GB download).
  Uses `ndarray 0.17` (must match ort's dependency version).
