#!/usr/bin/env bash
# Build the sandbox base image (headless bear-armature) used by the
# RUN_SANDBOX provider, plus optional toolchain variants layered on it.
# See packaging/sandbox-image/Dockerfile*.
#
#   scripts/build-sandbox-image.sh              # base image only
#   scripts/build-sandbox-image.sh rust node    # base + selected variants
#   scripts/build-sandbox-image.sh all          # base + rust + node + godot
#
# Tags: bears/sandbox:latest (override SANDBOX_IMAGE) and
# bears/sandbox-<variant>:latest. Declare them in the provider's roots-file
# `images` catalog to make them selectable for dispatch.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${SANDBOX_IMAGE:-bears/sandbox:latest}"
BUILD_SHA="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo container)"

build_image() {
    if docker buildx version >/dev/null 2>&1; then
        docker buildx build --load "$@"
    else
        docker build "$@"
    fi
}

build_image \
    -f "$REPO_ROOT/packaging/sandbox-image/Dockerfile" \
    --build-arg BEAR_ARMATURE_BUILD_SHA="$BUILD_SHA" \
    -t "$TAG" \
    "$REPO_ROOT"

variants=("$@")
if [[ "${1:-}" == "all" ]]; then
    variants=(rust node godot)
fi
for variant in "${variants[@]}"; do
    dockerfile="$REPO_ROOT/packaging/sandbox-image/Dockerfile.$variant"
    if [[ ! -f "$dockerfile" ]]; then
        echo "unknown sandbox image variant: $variant" >&2
        exit 1
    fi
    build_image \
        -f "$dockerfile" \
        --build-arg BASE_IMAGE="$TAG" \
        -t "bears/sandbox-$variant:latest" \
        "$REPO_ROOT/packaging/sandbox-image"
done
