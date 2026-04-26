# FROM mcr.microsoft.com/devcontainers/rust:1-1-bullseye
FROM rust:1.92-bookworm

# Include lld linker to improve build times either by using environment variable
# RUSTFLAGS="-C link-arg=-fuse-ld=lld" or with Cargo's configuration file (i.e see .cargo/config.toml).
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install clang lld \
    && apt-get autoremove -y && apt-get clean -y

RUN apt-get update \
    && apt-get install -qq --no-install-recommends \
    curl \
    coreutils \
    postgresql-client \
    locales \
    && sed -i '/en_US.UTF-8/s/^# *//' /etc/locale.gen \
    && locale-gen \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8
ENV LC_ALL=en_US.UTF-8

RUN --mount=type=cache,target=/usr/local/cargo/target/ \
    cargo install --target-dir /usr/local/cargo/target sqlx-cli --no-default-features --features rustls,postgres && \
    chmod -R a+w /usr/local/cargo/
