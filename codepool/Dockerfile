# syntax=docker/dockerfile:1
# node-pty (via letta-code) compiles from source when no prebuild exists (e.g. linux-arm64);
# node-gyp needs Python and a C++ toolchain during npm install only.
FROM node:22-bookworm-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 make g++ \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

COPY package.json package-lock.json* ./
RUN npm ci

COPY tsconfig.json ./
COPY src ./src
RUN npm run build && npm prune --omit=dev

FROM node:22-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

ARG CODEPOOL_GIT_SHA=unknown
ENV CODEPOOL_GIT_SHA=${CODEPOOL_GIT_SHA}

COPY --from=builder /app /app

ENV NODE_ENV=production
ENV PATH="/app/node_modules/.bin:${PATH}"
ENV HOME=/home/node

RUN mkdir -p /home/node/.letta && chown -R node:node /app /home/node
USER node

EXPOSE 3030
CMD ["node", "dist/index.js"]
