#!/bin/sh
set -e

mkdir -p /app/data
cp /app/default-config.json /app/data/config.json

# BEARS extension: expose normalized model metadata from the same checked-in
# Bifrost config that controls provider availability. The upstream Bifrost
# process owns the OpenAI-compatible API on APP_PORT; this lightweight local
# sidecar serves metadata on a distinct port for Den/preflight.
if command -v python3 >/dev/null 2>&1; then
  BEARS_METADATA_PORT="${BEARS_METADATA_PORT:-8081}"
  export BEARS_METADATA_PORT
  python3 - <<'PY' &
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

CONFIG_PATH = Path(os.environ.get("BEARS_BIFROST_CONFIG", "/app/data/config.json"))
PORT = int(os.environ.get("BEARS_METADATA_PORT", "8081"))


def load_payload():
    with CONFIG_PATH.open("r", encoding="utf-8") as f:
        cfg = json.load(f)
    bears = cfg.get("bears") or {}
    models = [m for m in bears.get("models", []) if m.get("enabled", True)]
    return {
        "object": "bears.bifrost_model_metadata",
        "metadata_version": bears.get("model_metadata_version", 1),
        "source": str(CONFIG_PATH),
        "models": models,
    }


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path not in ("/bears/models", "/bears/models/"):
            self.send_response(404)
            self.end_headers()
            return
        try:
            body = json.dumps(load_payload(), separators=(",", ":")).encode("utf-8")
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except Exception as exc:
            body = json.dumps({"error": str(exc)}).encode("utf-8")
            self.send_response(500)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, fmt, *args):
        return


ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
PY
else
  echo "python3 not found; BEARS /bears/models metadata endpoint disabled" >&2
fi

exec /app/docker-entrypoint.sh "$@"
