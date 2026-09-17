# skb-core

`skb-core` is the shared Rust library behind every SKB interface. It owns
configuration, embedded SurrealDB access, ingestion, chunking, embeddings,
hybrid search, the knowledge graph, CRUD operations, and reindexing.

The CLI, MCP server, and HTTP server are adapters over `KnowledgeBase`; add
product behavior here when it must be available through more than one surface.

## Responsibilities

- Store documents, chunks, vectors, and graph relations in embedded SurrealKV.
- Ingest text, Markdown, HTML, PDF, URL, stdin, and base64 content safely.
- Combine HNSW vector search with BM25 keyword search through reciprocal rank
  fusion.
- Extract rule-based entities and support graph traversal and search expansion.
- Keep embedding, tokenizer, and chunking metadata consistent; require
  reindexing when those settings change.

## Use from Rust

```rust
use skb_core::{config::Config, search::SearchRequest, KnowledgeBase};

let kb = KnowledgeBase::open(Config::load()?).await?;
let results = kb.search(&SearchRequest {
    query: "hybrid search".into(),
    mode: None,
    top_k: None,
    graph_expand: None,
    filter: None,
}).await?;
```

`Config::load()` searches `./skb.toml` first and then
`~/.config/skb/config.toml`. Use `onnx_path = "mock"` with `dimension = 8`
for fast local development. Real BAAI/bge-m3 inference requires the `ort`
feature.

## Development

Run these commands from the repository root:

```bash
cargo check -p skb-core
cargo clippy -p skb-core
cargo test -p skb-core -- --test-threads=1

# Real-embedding build
cargo build -p skb-core --features ort
```

Tests must run serially because SurrealKV is embedded. Keep TLS rustls-only:
do not add OpenSSL, `native-tls`, or remote SurrealDB protocol features.

## Public API and references

- [`KnowledgeBase`](src/lib.rs) is the public facade.
- Request and response types live in `config`, `ingest`, `search`, `graph`,
  `crud`, and `reindex` modules.
- The full data and behavioral contract is in the
  [repository specification](../../SPECIFICATION.md).
- For the user-facing entry points, see the [root README](../../README.md).
