# Surreal Knowledge Base

[![CI](https://github.com/My-MC/surreal-knowledge-base/actions/workflows/ci.yml/badge.svg)](https://github.com/My-MC/surreal-knowledge-base/actions/workflows/ci.yml)

Local-first knowledge base with hybrid search (vector + BM25) and knowledge graph, powered
by embedded [SurrealDB](https://surrealdb.com/) and [BAAI/bge-m3](https://huggingface.co/BAAI/bge-m3)
embeddings. It is available through an MCP server, CLI, HTTP API, and three web applications.

> 日本語版: [README.ja.md](README.ja.md)

## Features

- **Hybrid search** — vector (HNSW) + keyword (BM25) combined via Reciprocal Rank Fusion (RRF)
- **Knowledge graph** — rule-based entity extraction from documents, with link querying
- **Multi-format ingestion** — Markdown, plain text, PDF (text extraction)
- **Idempotent uploads** — SHA-256 content hashing with deduplication
- **Configurable chunking** — adjustable max tokens and overlap
- **Embedded-first** — SurrealKV storage engine, zero external services
- **Reindex** — switch embedding models or chunk settings on existing data
- **MCP server** — 10 tools for Claude Desktop / opencode / any MCP client
- **CLI** — full feature parity with `skb` command
- **HTTP API** — OpenAPI-documented API with document, search, graph, SSE chat, auth, and blog endpoints
- **Web applications** — Vault for editing, Studio for cited RAG chat, and Blog for publishing knowledge
- **Real embeddings** — BAAI/bge-m3 ONNX Runtime inference (optional, `ort` feature)

## Architecture

```
crates/
├── skb-core/    Core library (DB, embedding, tokenization, search, graph, ingestion)
├── skb-cli/     CLI binary (skb)
├── skb-mcp/     MCP server binary (skb-mcp)
└── skb-server/  HTTP API server (skb-server)
npm/             Meta-package + platform-specific npm packages
web/             Bun workspace with Vault, Studio, Blog, shared API client, and UI
schema/          SurrealDB migration (001_init.surql)
skills/          opencode agent skill
```

## Prerequisites

- [Rust](https://rustup.rs/) 1.88+
- For real embeddings (`--features ort`): ONNX Runtime is downloaded automatically on first build (~15 minutes, cached at `~/.cache/ort.pyke.io`)
- First CLI/MCP/HTTP-server run (even with mock embeddings): tokenizer.json is auto-downloaded from Hugging Face (~17 MB, cached at `~/.cache/huggingface`)
- For web applications: [Bun](https://bun.sh/) 1.4+

## Quick Start

### Build

```bash
# Fast build with mock embeddings (for testing/development)
cargo build

# Production build with real BAAI/bge-m3 embeddings
cargo build --release -p skb-mcp --features ort

# Production HTTP API build with real BAAI/bge-m3 embeddings
cargo build --release -p skb-server --features ort
```

### Configuration

Create `skb.toml` (searched from cwd, then `~/.config/skb/config.toml`):

```toml
# Mock embeddings (fast, no GPU, predictable outputs)
[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "./skb-data"

[server]
host = "127.0.0.1"
port = 8080

# Real embeddings with BAAI/bge-m3
# [embedding]
# model = "BAAI/bge-m3"
# onnx_path = "auto"
# [storage]
# path = "~/.local/share/skb/db"
```

### CLI

```bash
# Upload a document
skb upload README.md --title "README"

# Upload from URL
skb upload --url https://example.com/doc.md --tags "docs,example"

# Upload via stdin
cat notes.txt | skb upload --stdin --title "Meeting Notes"

# Search (hybrid = vector + keyword)
skb search "vector database" --mode hybrid --top-k 10

# List documents
skb list --limit 20

# Get document details
skb get <doc-id>

# Delete document
skb delete <doc-id> --yes

# Statistics
skb stats

# Run diagnostics
skb doctor
```

### MCP Server

For development, start the stdio server from the repository root:

```bash
cargo run -p skb-mcp --bin skb-mcp
```

#### Client configuration (opencode / Claude Desktop)

```jsonc
{
  "mcp": {
    "surreal-knowledge-base": {
      "type": "local",
      "command": ["cargo", "run", "-p", "skb-mcp", "--bin", "skb-mcp"],
      "enabled": true
    }
  }
}
```

### HTTP API server

`skb-server` owns the embedded database for its lifetime. Do not start the CLI
or MCP server against the same `[storage].path` while it is running.

```bash
# The mock configuration above lets this run without the ort feature.
cargo run -p skb-server --bin skb-server -- --port 8080

# Or run the real-embedding release build.
./target/release/skb-server --port 8080
```

The API is available at `http://127.0.0.1:8080`. Inspect its generated contract
at [`/api/openapi.json`](http://127.0.0.1:8080/api/openapi.json) or use
[`/swagger-ui`](http://127.0.0.1:8080/swagger-ui). The chat endpoint streams
answers from an OpenAI-compatible LLM; server and LLM configuration are
documented in [SPECIFICATION.md](SPECIFICATION.md).

### Web applications

Start `skb-server` first, then install the Bun workspace dependencies:

```bash
cd web
bun install
```

Run an application from `web/` (each Vite development server proxies `/api` to
`SKB_SERVER_PORT`, which defaults to `8080`):

```bash
bun --filter @skb/vault dev   # Markdown knowledge-base editor
bun --filter @skb/studio dev  # RAG chat with source citations
bun --filter @skb/blog dev    # Public knowledge blog and authoring flow
```

| Application | Purpose |
|---|---|
| Vault | Browse, edit, autosave, search, and graph-link Markdown documents. |
| Studio | Ask questions over the knowledge base and inspect streaming answers with citations. |
| Blog | Read published documents; invited authors can sign in, create, and publish posts. |

## CLI Commands

| Command | Description |
|---|---|
| `skb upload <FILE>` | Upload a file (`--recursive`, `--metadata JSON`, `--force`) |
| `skb upload --url <URL>` | Upload from URL |
| `skb upload --stdin` | Upload from stdin |
| `skb search <QUERY>` | Search documents (`--mode hybrid\|vector\|keyword --top-k N --filter KEY=VALUE`) |
| `skb list` | List documents (`--limit N --offset N --order ...`) |
| `skb get <ID>` | Get document details (`--chunks`) |
| `skb delete <ID>` | Delete a document (`--yes`) |
| `skb stats` | Show statistics |
| `skb graph query <ENTITY>` | Query knowledge graph |
| `skb graph entity <NAME> --kind <KIND>` | Add/update an entity |
| `skb graph link <FROM> <TO>` | Link two entities |
| `skb reindex` | Reindex all documents (`--dry-run` supported) |
| `skb config init\|show\|set` | Manage configuration |
| `cargo run -p skb-mcp --bin skb-mcp` | Start the development MCP server |
| `skb doctor` | Run diagnostics |

All commands support `--format json` for structured output.

## MCP Tools

| Tool | Description |
|---|---|
| `skb_upload` | Upload document (path, url, content, or content_base64) |
| `skb_search` | Search documents (hybrid, vector, or keyword) |
| `skb_list_documents` | List all documents |
| `skb_get_document` | Get document details |
| `skb_delete_document` | Delete a document |
| `skb_stats` | Show statistics |
| `skb_graph_query` | Query knowledge graph |
| `skb_graph_upsert_entity` | Create or update entity |
| `skb_graph_link` | Link two entities |
| `skb_reindex` | Reindex all documents |

## Configuration Reference

### `[storage]`

| Key | Default | Description |
|---|---|---|
| `path` | `~/.local/share/skb/db` | Database directory path |
| `namespace` | `"skb"` | SurrealDB namespace |
| `database` | `"knowledge"` | SurrealDB database name |

> Note: the config key is `[storage]`, not `[database]`. Using the wrong key silently falls back to defaults.

### `[embedding]`

| Key | Default | Description |
|---|---|---|
| `model` | `"BAAI/bge-m3"` | HuggingFace model ID |
| `onnx_path` | `"auto"` | ONNX model path; `"mock"` for fast mock embeddings |
| `dimension` | `0` (auto-detect) | Embedding dimension (`8` for mock) |
| `batch_size` | `32` | Inference batch size |
| `max_input_tokens` | `0` (auto = 8192) | Max tokens per input |

### `[chunking]`

| Key | Default | Description |
|---|---|---|
| `max_tokens` | `512` | Max tokens per chunk |
| `overlap_tokens` | `64` | Overlap between adjacent chunks |

### `[search]`

| Key | Default | Description |
|---|---|---|
| `default_mode` | `"hybrid"` | Default search mode (`hybrid\|vector\|keyword`) |
| `top_k` | `10` | Default result count |
| `rrf_k` | `60` | RRF rank constant |

### `[upload]`

| Key | Default | Description |
|---|---|---|
| `max_file_mb` | `100` | Max upload file size |

## Development

```bash
# Fast compile feedback
cargo check --workspace

# Lint (must be clean)
cargo clippy --workspace

# Format
cargo fmt --all

# Tests (serial execution required — embedded SurrealKV)
cargo test --workspace -- --test-threads=1

# Benchmarks (mock embeddings)
cargo bench

# Benchmarks (real BAAI/bge-m3, requires ort feature)
cargo bench --features ort
```

### Web development

```bash
cd web

# Format and lint the workspace
bun run biome

# Type-check every application and shared package
bun --filter '*' typecheck

# Run the SSE smoke test (starts its own mock LLM and HTTP server)
bun run sse-smoke

# Run browser end-to-end tests (build skb-server and mock_llm first)
cargo build --manifest-path ../Cargo.toml -p skb-server --bin skb-server --examples
bunx playwright install chromium
bunx playwright test
```

## Documentation

- [SPECIFICATION.md](SPECIFICATION.md) — Authoritative spec
- [CONTRIBUTING.md](CONTRIBUTING.md) — Development conventions
- [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) — Phase-based implementation plan
- [AGENTS.md](AGENTS.md) — Agent instructions

## License

MIT. ONNX Runtime is statically linked under its [MIT license](npm/THIRD_PARTY_LICENSES.md).
