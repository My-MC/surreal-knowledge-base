# syntax=docker/dockerfile:1.7

FROM rust:1.94-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates/skb-core/Cargo.toml crates/skb-core/Cargo.toml
COPY crates/skb-cli/Cargo.toml crates/skb-cli/Cargo.toml
COPY crates/skb-mcp/Cargo.toml crates/skb-mcp/Cargo.toml
COPY crates/skb-server/Cargo.toml crates/skb-server/Cargo.toml
RUN for package in skb-core skb-cli skb-mcp skb-server; do \
        mkdir -p "crates/${package}/src"; \
        printf 'pub fn placeholder() {}\n' > "crates/${package}/src/lib.rs"; \
        printf 'fn main() {}\n' > "crates/${package}/src/main.rs"; \
    done \
    && mkdir -p crates/skb-core/benches \
    && printf 'fn main() {}\n' > crates/skb-core/benches/skb.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch --locked

COPY crates ./crates
COPY schema ./schema
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    cargo build --release --locked -p skb-server \
    && mkdir -p /artifacts \
    && cp /workspace/target/release/skb-server /artifacts/skb-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 skb
COPY --from=builder /artifacts/skb-server /usr/local/bin/skb-server
COPY deploy/api-entrypoint.sh /usr/local/bin/api-entrypoint
RUN chmod 755 /usr/local/bin/api-entrypoint
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=6 \
    CMD curl --fail --silent http://127.0.0.1:8080/api/health || exit 1
ENTRYPOINT ["api-entrypoint"]
