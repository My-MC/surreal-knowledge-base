# skb-server

`skb-server` is SKB's HTTP API server. It opens the embedded knowledge base
once at startup and shares that instance across document, search, graph, chat,
authentication, and blog endpoints.

## Start locally

Create a repository-root `skb.toml` for mock embeddings:

```toml
[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "./skb-data"

[server]
host = "127.0.0.1"
port = 8080
```

Then run:

```bash
cargo run -p skb-server --bin skb-server -- --port 8080
```

The generated API contract is at `http://127.0.0.1:8080/api/openapi.json` and
the interactive API explorer is at `http://127.0.0.1:8080/swagger-ui`.

For real embeddings, build with `--features ort`. `--port 0` selects an
ephemeral port and writes `SKB_SERVER_PORT=<port>` to stdout; logs are written
to stderr.

## Runtime model

SurrealKV is embedded, so one process owns a storage path at a time. While
this server is running, do not open the same `[storage].path` through `skb` or
`skb-mcp`.

The server supports an OpenAI-compatible streaming chat upstream. Configure it
with `SKB_LLM_BASE_URL`, `SKB_LLM_MODEL`, and, when needed,
`SKB_LLM_API_KEY`. Authentication-dependent endpoints also need a strong
`SKB_SERVER_JWT_SECRET`; author registration uses operator-managed
`SKB_SERVER_AUTHOR_INVITES` entries.

## API areas

- Documents: create, list, get, replace, delete, and backlinks.
- Knowledge retrieval: hybrid search, graph query, and graph-expanded search.
- Chat: `POST /api/chat/stream` sends citations and answer tokens as SSE.
- Accounts and publishing: registration, login, logout, published blog posts,
  and author-only publication.

The exact routes, auth rules, error mappings, and environment variables are
defined in the [HTTP API specification](../../SPECIFICATION.md#20-http-apiserverskb-server).

## Development

```bash
cargo check -p skb-server
cargo clippy -p skb-server
cargo test -p skb-server -- --test-threads=1

# Build the server and mock OpenAI-compatible LLM used by web E2E tests
cargo build -p skb-server --bin skb-server --examples
```

All TLS dependencies must remain rustls-only. The server deliberately accepts
content and URL uploads over HTTP but never a client-provided filesystem path.
