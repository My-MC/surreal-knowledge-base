# skb Studio

Studio is the SKB RAG chat interface. It streams answers from the HTTP server,
persists the local chat session, and displays the documents that grounded the
latest assistant response.

## Prerequisites

Studio is a Vite application inside the `web/` Bun workspace. Start
`skb-server` first; the Vite proxy forwards `/api` requests to
`SKB_SERVER_PORT`, which defaults to `8080`.

```bash
# Repository root: start the API server
cargo run -p skb-server --bin skb-server -- --port 8080

# In another terminal
cd web
bun install
bun --filter @skb/studio dev
```

For useful chat output, configure the server with an OpenAI-compatible LLM,
for example through `SKB_LLM_BASE_URL` and `SKB_LLM_MODEL`. Without a reachable
upstream, Studio renders the server's stream error.

## Interaction model

1. A user sends a message from the composer.
2. Studio calls `POST /api/chat/stream` through the shared `useChatStream`
   hook.
3. The server sends citations, text tokens, and a terminal `done` or `error`
   SSE event.
4. Studio stores the transcript locally and shows the latest citations in the
   side panel. Citation links open a document detail route.

The stop action aborts the active stream. Starting a new session clears the
persisted transcript.

## Development

Run from `web/`:

```bash
bun --filter @skb/studio typecheck
bun --filter @skb/studio build

# End-to-end chat verification
cargo build --manifest-path ../Cargo.toml -p skb-server --bin skb-server --examples
bunx playwright install chromium
bunx playwright test e2e/studio.spec.mts
```

The standalone SSE smoke test is `bun run sse-smoke` from `web/`; it starts its
own mock LLM and server.

See the [root README](../../../README.md) and
[server README](../../../crates/skb-server/README.md) for shared runtime
configuration.
