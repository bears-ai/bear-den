# Memory Manager: git http-backend for Letta /v1/git proxy; needs full git (not git-minimal) for http-backend
FROM python:3.12-slim
RUN apt-get update && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY git_memfs_server.py .
EXPOSE 8285
# Default matches Letta LocalStorageBackend: ~/.letta/memfs/repository/...
ENV MEMFS_BASE=/root/.letta/memfs/repository
CMD ["python", "git_memfs_server.py"]
