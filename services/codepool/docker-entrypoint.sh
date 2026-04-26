#!/bin/sh
# Named volumes mount as root-owned; the app runs as `node` (see Dockerfile).
# Fix ownership of /home/node/.letta before dropping privileges (Letta Code CLI cache).
set -e
mkdir -p /home/node/.letta
if [ "$(id -u)" -eq 0 ]; then
  chown -R node:node /home/node/.letta
  exec gosu node "$@"
fi
exec "$@"
