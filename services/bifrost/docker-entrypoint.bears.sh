#!/bin/sh
set -e

mkdir -p /app/data
cp /app/default-config.json /app/data/config.json

exec /app/docker-entrypoint.sh "$@"
