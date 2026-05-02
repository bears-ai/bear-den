# syntax=docker/dockerfile:1
FROM python:3.12-alpine
WORKDIR /app

# Build from the repository root so config validation uses the same Git-tracked
# Bifrost config that is baked into the Bifrost image. Do not bind-mount source
# files at runtime; smoke/prod-like compose should run immutable artifacts.
COPY services/preflight/preflight.py /app/preflight.py
COPY services/bifrost/config.json /app/bifrost/config.json

ENTRYPOINT ["python3", "/app/preflight.py"]
