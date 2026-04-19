# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim
WORKDIR /app

COPY package.json package-lock.json* ./
RUN npm ci

COPY tsconfig.json ./
COPY src ./src
RUN npm run build && npm prune --omit=dev

ENV NODE_ENV=production
ENV PATH="/app/node_modules/.bin:${PATH}"
ENV HOME=/home/node

RUN mkdir -p /home/node/.letta && chown -R node:node /app /home/node
USER node

EXPOSE 3030
CMD ["node", "dist/index.js"]
