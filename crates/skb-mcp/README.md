# skb-mcp

`skb-mcp` exposes the local Surreal Knowledge Base to MCP clients over stdio.
It is a thin Rust adapter over `skb-core` and is distributed through the
`surreal-knowledge-base` npm package.

## Start the server

For a development build with mock embeddings, create `skb.toml` at the
repository root and run:

```toml
[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "./skb-data"
```

```bash
cargo run -p skb-mcp
```

For a packaged client installation, use the npm launcher:

```bash
npx -y surreal-knowledge-base
```

The transport is stdio. Keep stdout reserved for MCP protocol messages; use
stderr for diagnostics.

## MCP surface

The server provides ten tools: upload, search, list documents, get document,
delete document, statistics, graph query, entity upsert, graph link, and
reindex. It also provides document and statistics resources plus reusable
prompts. Tool schemas are generated from the same request and response types
used by `skb-core` and the CLI.

Example client configuration:

```jsonc
{
  "mcp": {
    "surreal-knowledge-base": {
      "type": "local",
      "command": ["npx", "-y", "surreal-knowledge-base"],
      "enabled": true
    }
  }
}
```

## Development

```bash
cargo check -p skb-mcp
cargo clippy -p skb-mcp
cargo test -p skb-mcp -- --test-threads=1

# Build with real BAAI/bge-m3 embeddings
cargo build --release -p skb-mcp --features ort
```

MCP/CLI golden tests live in `tests/golden.rs`. Do not run `skb-mcp` against a
storage path currently owned by `skb-server`.

See the [root README](../../README.md) and
[MCP specification](../../SPECIFICATION.md#8-mcp-server) for the full
contract.
