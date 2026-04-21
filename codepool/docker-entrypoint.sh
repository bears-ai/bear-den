#!/bin/sh
# Named volumes mount as root-owned; the app runs as `node` (see Dockerfile).
# Fix ownership of writable roots before dropping privileges.
set -e
MEM_ROOT="${BEAR_MEMORY_ROOT:-}"
if [ -n "$MEM_ROOT" ]; then
  mkdir -p "$MEM_ROOT"
fi
mkdir -p /home/node/.letta
if [ "$(id -u)" -eq 0 ]; then
  if [ -n "$MEM_ROOT" ]; then
    chown -R node:node "$MEM_ROOT"
  fi
  chown -R node:node /home/node/.letta
  exec gosu node "$@"
fi
exec "$@"
