# skb CLI

`skb` is the command-line interface for the local Surreal Knowledge Base. It
is a thin adapter over `skb-core`, so its document, search, graph, and reindex
behavior matches the MCP surface.

## Build and run

Run from the repository root. For development, configure mock embeddings in
`skb.toml`:

```toml
[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "./skb-data"
```

Then invoke the CLI through Cargo:

```bash
cargo run -p skb -- upload README.md --title "Project README"
cargo run -p skb -- search "hybrid search" --mode hybrid --top-k 10
cargo run -p skb -- list --limit 20
cargo run -p skb -- doctor
```

Build with real BAAI/bge-m3 embeddings when the configuration does not use
`onnx_path = "mock"`:

```bash
cargo build --release -p skb --features ort
./target/release/skb search "hybrid search"
```

## Commands

The CLI supports document upload from files, URLs, stdin, and content; hybrid,
vector, and keyword search; document CRUD; graph management; configuration;
diagnostics; and reindexing. Use `--format json` for machine-readable output.

```bash
skb upload notes.md --tags "project,notes"
skb graph query "SurrealDB"
skb reindex --dry-run
skb config show
```

`skb query <surql>` is a CLI-only escape hatch for trusted local SurrealQL.
Do not interpolate untrusted input into it.

## Development

```bash
cargo check -p skb
cargo clippy -p skb
cargo test -p skb -- --test-threads=1
```

The contract tests spawn `target/debug/skb`, so run them through Cargo. When
`skb-server` owns a storage path, do not start this CLI against the same path;
embedded SurrealKV permits one owning process.

See the [root README](../../README.md) for the complete command list and
configuration reference.
