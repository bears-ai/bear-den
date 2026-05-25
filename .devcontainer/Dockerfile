FROM debian:bookworm-slim

ARG RUST_VERSION=1.92.0

RUN apt-get update && apt-get install -y \
    curl git bash ca-certificates build-essential clang lld pkg-config libssl-dev openssh-client \
    python3 python3-pytest python3-requests \
    docker.io \
    && mkdir -p -m 755 /etc/apt/keyrings \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list \
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get update && apt-get install -y nodejs gh \
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

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | bash -s -- -y --no-modify-path --profile minimal --default-toolchain ${RUST_VERSION} \
    && rustup component add clippy rustfmt \
    && for bin in ${CARGO_HOME}/bin/*; do ln -sf "$bin" /usr/local/bin/"$(basename "$bin")"; done \
    && printf 'export PATH="%s/bin:$PATH"\n' "${CARGO_HOME}" > /etc/profile.d/cargo.sh \
    && chmod -R a+w ${RUSTUP_HOME} ${CARGO_HOME}

# Dev defaults duplicated from devcontainer.json so they are present even when
# a devcontainer launcher does not apply `containerEnv` at container-create time.
ENV OPENCODE_AUTH_PATH=/root/.local/share/opencode/auth.json \
    DATABASE_URL=postgres://bears:bears@bears-postgres:5432/den?sslmode=disable \
    LETTA_PG_URI=postgresql://bears:bears@bears-letta-postgres:5432/letta \
    MEMORY_SERVE_QUIET=1
