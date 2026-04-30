# syntax=docker/dockerfile:1

# Multi-stage build with cargo-chef so dependency compilation is cached in a
# Docker layer that only invalidates when Cargo.lock / Cargo.toml / build.rs
# dependency metadata changes — not when templates or other application sources
# change. GitHub Actions imports/exports buildx cache; this structure makes
# dependency layer reuse visible as a BuildKit `CACHED` step instead of running
# `cargo chef cook` on every build and merely reporting Cargo artifacts as
# `Fresh`.

ARG RUST_VERSION=1.92.0

FROM rust:${RUST_VERSION}-alpine AS rust-base
WORKDIR /app
RUN apk add --no-cache clang lld musl-dev git

FROM rust-base AS chef-base
RUN cargo install cargo-chef --version 0.1.71 --locked

FROM chef-base AS planner
# Only copy inputs that affect `cargo metadata` / the dependency recipe. Omitting
# docs, migrations, .sqlx, templates, assets, etc. keeps this layer (and thus the
# cacher stage) stable when those files change — so CI reuses compiled deps.
# Keep Cargo config in the chef stages too: rustflags/linker settings affect
# Cargo fingerprints, so dependencies cooked without this config are considered
# stale when the final build runs after `COPY . .`.
COPY .cargo ./.cargo
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY tests ./tests
RUN cargo chef prepare --recipe-path recipe.json

FROM chef-base AS cacher
COPY --from=planner /app/recipe.json recipe.json
COPY .cargo ./.cargo
COPY Cargo.toml Cargo.lock build.rs ./
# Keep dependency build artifacts in the committed layer at /app/target. Do not
# put CARGO_TARGET_DIR behind a cache mount here: doing so makes CI rerun
# `cargo chef cook` and print many `Fresh` lines instead of reusing this step as
# a normal Docker layer. Registry/git cache mounts still avoid re-downloading
# crates when this layer genuinely has to be rebuilt.
RUN --mount=type=cache,target=/usr/local/cargo/registry/,id=den-cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git/db,id=den-cargo-git,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json

FROM rust-base AS build
ARG APP_NAME=den
ARG GIT_SHA=
ARG DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable
ARG SQLX_OFFLINE=false
ENV GIT_SHA=${GIT_SHA}
COPY --from=cacher /app/target target
COPY . .
# Same registry/git cache mount IDs as cacher so this step does not re-download
# crates.io sources that chef already populated in one build. The target dir is
# copied from the dependency layer above, so dependencies should be `Fresh`; only
# the Den crate itself should rebuild when source files or GIT_SHA change.
RUN --mount=type=cache,target=/usr/local/cargo/registry/,id=den-cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git/db,id=den-cargo-git,sharing=locked \
    export DATABASE_URL="${DATABASE_URL}" SQLX_OFFLINE="${SQLX_OFFLINE}" && \
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

# Runtime env (set in Coolify / compose — not baked into the image):
# When RUN_WEB=true (default for production web):
#   CODEPOOL_BASE_URL — optional in image; production builds default to http://bears-codepool:3030 when unset.
#     (Letta Code SDK harness; bear chat streaming does not use the Letta server HTTP API directly).
#   CODEPOOL_INTERNAL_TOKEN — optional; must match the Codepool container if set.
# See ../../services/codepool/COOLIFY_DEPLOY.md and services/den/COOLIFY_DEPLOY.md.

EXPOSE 3000

CMD ["/bin/server"]
