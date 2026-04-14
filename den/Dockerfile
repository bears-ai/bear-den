# syntax=docker/dockerfile:1

# Multi-stage build with cargo-chef so dependency compilation is cached in a
# Docker layer that only invalidates when Cargo.lock (recipe) changes — not
# when templates or other application sources change. GitHub Actions still
# uses buildx cache (gha); this structure makes layer reuse reliable even when
# mount caches miss.

ARG RUST_VERSION=1.92.0

FROM rust:${RUST_VERSION}-alpine AS rust-base
WORKDIR /app
RUN apk add --no-cache clang lld musl-dev git

FROM rust-base AS chef-base
RUN cargo install cargo-chef --version 0.1.71 --locked

FROM chef-base AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef-base AS cacher
COPY --from=planner /app/recipe.json recipe.json
COPY Cargo.toml Cargo.lock build.rs ./
# Target artifacts must live in the image layer (not only a cache mount) so a
# later stage can `COPY --from=cacher /app/target`. Compile into a cache mount,
# then snapshot into /app/target for the committed layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry/,id=den-cargo-registry \
    --mount=type=cache,target=/usr/local/cargo/git/db,id=den-cargo-git \
    --mount=type=cache,target=/mnt/cargo-target,id=den-cargo-target \
    CARGO_TARGET_DIR=/mnt/cargo-target cargo chef cook --release --recipe-path recipe.json \
    && mkdir -p /app/target && cp -a /mnt/cargo-target/. /app/target/

FROM rust-base AS build
ARG APP_NAME=den
ARG DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable
ARG SQLX_OFFLINE=false
COPY --from=cacher /app/target target
COPY . .
RUN export DATABASE_URL="${DATABASE_URL}" SQLX_OFFLINE="${SQLX_OFFLINE}" && \
    echo "DATABASE_URL=$DATABASE_URL" && echo "SQLX_OFFLINE=$SQLX_OFFLINE" && \
    cargo build --locked --release --features=production --message-format=short && \
    cp "./target/release/${APP_NAME}" /bin/server

FROM alpine:3.18 AS final

ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser


USER appuser

COPY --from=build /bin/server /bin/

EXPOSE 3000

CMD ["/bin/server"]
