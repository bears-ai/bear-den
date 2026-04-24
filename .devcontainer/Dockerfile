# Root devcontainer image for BEARS:
# - Den (Rust + SQLx + Postgres client)
# - Codepool (Node.js 20 + npm)

FROM rust:1.92-bookworm AS dev_containers_target_stage

# Base OS packages and Rust build tooling.
RUN apt-get update \
    && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        gnupg \
        dirmngr \
        coreutils \
        postgresql-client \
        locales \
        clang \
        lld \
    && sed -i '/en_US.UTF-8/s/^# *//' /etc/locale.gen \
    && locale-gen \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8
ENV LC_ALL=en_US.UTF-8

# Install Node.js 20 (for Codepool).
RUN mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
      | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_20.x nodistro main" \
      > /etc/apt/sources.list.d/nodesource.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

ENV PATH="/usr/local/bin:/root/.cargo/bin:${PATH}"
ENV CARGO_HOME="/usr/local/cargo"
ENV RUSTUP_HOME="/usr/local/rustup"

# Install SQLx CLI (for Den migrations/dev workflows) and Rust components used in CI/dev checks.
RUN cargo install \
      --target-dir /usr/local/cargo/target \
      sqlx-cli \
      --no-default-features \
      --features rustls,postgres \
    && rustup component add rustfmt clippy
