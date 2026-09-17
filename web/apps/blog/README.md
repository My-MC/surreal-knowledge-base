# skb Blog

Blog is the public publishing interface for knowledge-base documents. Visitors
can browse published posts, while invited authors can register, sign in, write
posts, and publish them.

## Prerequisites

Blog is a Vite application in the `web/` Bun workspace. It needs a running
`skb-server`; the development proxy forwards `/api` to `SKB_SERVER_PORT`,
which defaults to `8080`.

Set server authentication configuration before testing author flows:

```bash
export SKB_SERVER_JWT_SECRET='replace-with-a-strong-32-character-secret'
export SKB_SERVER_AUTHOR_INVITES='writer@example.com:invite-token'
cargo run -p skb-server --bin skb-server -- --port 8080
```

Then run Blog from the web workspace:

```bash
cd web
bun install
bun --filter @skb/blog dev
```

## Roles and publishing

- Public registration creates a `reader` account.
- An email listed in `SKB_SERVER_AUTHOR_INVITES` becomes an `author` only when
  its matching invite token is supplied at registration.
- Authors can create and publish Blog documents; readers can view only public
  posts.
- Session authentication uses an HttpOnly server cookie. The client keeps only
  the email and role needed to render navigation.

Posts use SKB document storage. The server records publishing state and
ownership separately, so authorization survives document content updates.

## Development

Run from `web/`:

```bash
bun --filter @skb/blog typecheck
bun --filter @skb/blog build

cargo build --manifest-path ../Cargo.toml -p skb-server --bin skb-server --examples
bunx playwright install chromium
bunx playwright test e2e/blog.spec.mts
```

See the [root README](../../../README.md) for shared commands and the
[server README](../../../crates/skb-server/README.md) for the API, security,
and environment-variable contract.
