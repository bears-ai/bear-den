#!/bin/bash
set -e
SERVICE=$1
if [ -z "$SERVICE" ]; then
  echo "Usage: ./scripts/logs.sh <service>"
  exit 1
fi
docker compose logs -f "$SERVICE"
