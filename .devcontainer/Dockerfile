FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    curl git bash ca-certificates \
    python3 python3-pytest python3-requests \
    docker.io \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /usr/local/lib/docker/cli-plugins \
    && arch="$(uname -m)" \
    && case "$arch" in \
      x86_64) compose_arch="x86_64" ;; \
      aarch64|arm64) compose_arch="aarch64" ;; \
      *) echo "Unsupported architecture: $arch" >&2; exit 1 ;; \
    esac \
    && curl -fsSL "https://github.com/docker/compose/releases/download/v2.29.7/docker-compose-linux-${compose_arch}" \
      -o /usr/local/lib/docker/cli-plugins/docker-compose \
    && chmod +x /usr/local/lib/docker/cli-plugins/docker-compose
