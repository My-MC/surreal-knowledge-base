# syntax=docker/dockerfile:1.7

FROM oven/bun:1.4-debian AS dependencies
WORKDIR /web
ENV NODE_ENV=development

COPY web/package.json web/bun.lock ./
COPY web/apps/blog/package.json ./apps/blog/
COPY web/apps/studio/package.json ./apps/studio/
COPY web/apps/vault/package.json ./apps/vault/
COPY web/packages/api-client/package.json ./packages/api-client/
COPY web/packages/ui/package.json ./packages/ui/
RUN --mount=type=cache,target=/root/.bun/install/cache \
    bun install --frozen-lockfile

FROM dependencies AS builder
COPY web ./
RUN bun --filter @skb/vault build \
    && bun --filter @skb/studio build -- --base=/studio/ \
    && bun --filter @skb/blog build -- --base=/blog/

FROM nginx:1.29-alpine AS runtime
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /web/apps/vault/dist /usr/share/nginx/html
COPY --from=builder /web/apps/studio/dist /usr/share/nginx/html/studio
COPY --from=builder /web/apps/blog/dist /usr/share/nginx/html/blog
EXPOSE 80
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -q -O /dev/null http://127.0.0.1/ || exit 1
