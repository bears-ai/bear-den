# Letta Code harness — `letta server` for BEARS (see COOLIFY_DEPLOY.md).
# Aligned with https://github.com/letta-ai/letta-code-server-deployment (Node + git + python for skills).

FROM node:20-bookworm-slim

ARG LETTA_CODE_VERSION=0.23.8

RUN apt-get update && \
    apt-get install -y git python3 make g++ && \
    npm install -g @letta-ai/letta-code@${LETTA_CODE_VERSION} && \
    apt-get purge -y make g++ && \
    apt-get autoremove -y && \
    rm -rf /var/lib/apt/lists/*

ENV ENV_NAME=cloud

# Den expects HTTP at LETTA_CODE_BASE_URL — publish the harness listen port in Coolify when your
# pinned @letta-ai/letta-code version documents it; until then, follow COOLIFY_DEPLOY.md smoke tests.
CMD ["sh", "-c", "exec letta server --env-name \"${ENV_NAME}\" --debug"]
