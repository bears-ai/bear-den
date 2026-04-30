# Letta's MemFS / Context Repository support shells out to the git CLI for
# repository operations behind /v1/git/{agent_id}/state.git. The upstream
# letta/letta image may not include git, so BEARS wraps it and installs git.
ARG LETTA_BASE_IMAGE=letta/letta:latest
FROM ${LETTA_BASE_IMAGE}

USER root

RUN if command -v git >/dev/null 2>&1; then \
        git --version; \
    elif command -v apt-get >/dev/null 2>&1; then \
        apt-get update \
        && apt-get install -y --no-install-recommends git ca-certificates \
        && rm -rf /var/lib/apt/lists/*; \
    elif command -v apk >/dev/null 2>&1; then \
        apk add --no-cache git ca-certificates; \
    elif command -v microdnf >/dev/null 2>&1; then \
        microdnf install -y git ca-certificates \
        && microdnf clean all; \
    elif command -v dnf >/dev/null 2>&1; then \
        dnf install -y git ca-certificates \
        && dnf clean all; \
    elif command -v yum >/dev/null 2>&1; then \
        yum install -y git ca-certificates \
        && yum clean all; \
    else \
        echo "No supported package manager found; cannot install git for Letta MemFS" >&2; \
        exit 1; \
    fi
