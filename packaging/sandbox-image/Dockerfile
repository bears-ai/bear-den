# Sandbox base image: a headless bear-armature that executes one Den work
# order inside a container. The sandbox provider (RUN_SANDBOX) launches this
# image per work run with env: DEN_API_URL, BEAR_SLUG, DEN_TOKEN,
# DEN_WORK_ORDER_ID, DEN_WORKSPACE (/workspace, bind-mounted),
# DEN_HEADLESS_DEADLINE_SECS.
#
# Build context is the repository root:
#   docker build -f packaging/sandbox-image/Dockerfile -t bears/sandbox:latest .
# or use scripts/build-sandbox-image.sh.
#
# Keep this base minimal — task toolchains (node, python, ...) belong in
# per-root image overrides passed via the provider's `image` field, layered on
# top of this one.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY tools/bear-armature ./tools/bear-armature
COPY services/den/Cargo.toml ./services/den/Cargo.toml
COPY services/den/crates/bearwire-protocol ./services/den/crates/bearwire-protocol
# BEAR_ARMATURE_BUILD_SHA lets build.rs stamp a version without a .git dir.
ARG BEAR_ARMATURE_BUILD_SHA=container
ENV BEAR_ARMATURE_BUILD_SHA=${BEAR_ARMATURE_BUILD_SHA}
RUN cargo build --release --manifest-path tools/bear-armature/Cargo.toml

FROM debian:bookworm-slim
# The provider also runs this image as the restricted-network relay. Hat-bound
# external hosts use the Den-checking script; the Den callback and no-hat
# compatibility relay still use socat directly.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates curl ripgrep socat jq coreutils \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bear
COPY --from=build /src/tools/bear-armature/target/release/bear-armature /usr/local/bin/bear-armature
COPY --chmod=0755 packaging/sandbox-image/egress-relay.sh /usr/local/bin/bears-egress-relay
USER bear
WORKDIR /workspace
ENTRYPOINT ["bear-armature", "headless"]
