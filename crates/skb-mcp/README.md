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

The npm package is not published yet. Use the Cargo command above for local
development; after publication, the package will provide the `skb-mcp` launcher.

The transport is stdio. Keep stdout reserved for MCP protocol messages; use
stderr for diagnostics.

## MCP surface

The server provides ten tools: upload, search, list documents, get document,
delete document, statistics, graph query, entity upsert, graph link, and
reindex. It also provides document and statistics resources plus reusable
prompts. Tool schemas are generated from the same request and response types
used by `skb-core` and the CLI.

OpenCode configuration (replace `/absolute/path/to/surreal-knowledge-base`):

```jsonc
{
  "mcp": {
    "surreal-knowledge-base": {
      "type": "local",
      "command": [
        "cargo", "run",
        "--manifest-path", "/absolute/path/to/surreal-knowledge-base/Cargo.toml",
        "-p", "skb-mcp", "--bin", "skb-mcp"
      ],
      "enabled": true
    }
  }
}
```

Claude Desktop uses a string command and separate arguments:

```jsonc
{
  "mcpServers": {
    "surreal-knowledge-base": {
      "command": "/absolute/path/to/cargo",
      "args": [
        "run",
        "--manifest-path", "/absolute/path/to/surreal-knowledge-base/Cargo.toml",
        "-p", "skb-mcp", "--bin", "skb-mcp"
      ]
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
