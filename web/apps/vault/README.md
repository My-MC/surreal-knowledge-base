# skb Vault

Vault is the SKB document workspace. It provides a three-pane interface for
browsing documents, editing Markdown, viewing backlinks, searching with
Cmd/Ctrl+K, and inspecting graph relationships.

## Prerequisites

Vault is a Vite application inside the `web/` Bun workspace. It requires a
running `skb-server`; its development proxy sends `/api` requests to
`SKB_SERVER_PORT`, which defaults to `8080`.

From the repository root, start the server with mock embeddings:

```bash
cargo run -p skb-server -- --port 8080
```

In another terminal, install the web workspace and start Vault:

```bash
cd web
bun install
bun --filter @skb/vault dev
```

## Features

- Document tree, editor, and backlinks in a three-pane layout.
- Markdown editing with autosave and save-state feedback.
- WikiLink completion and navigation between knowledge-base documents.
- Search palette backed by SKB hybrid search.
- Markdown rendering and a graph overlay supplied by shared `@skb/ui`
  components.

The UI uses server APIs as the source of document data. Zustand holds only UI
state; TanStack Query owns cached server data.

## Development

Run from `web/`:

```bash
bun --filter @skb/vault typecheck
bun --filter @skb/vault build
```

The shared browser E2E suite is in `web/e2e/vault.spec.mts`. Build the server
and its mock LLM before running it:

```bash
cargo build --manifest-path ../Cargo.toml -p skb-server --bin skb-server --examples
bunx playwright test e2e/vault.spec.mts
```

See the [root README](../../../README.md) for shared setup and server
configuration.
