#!/bin/sh
set -e

mkdir -p /app/data

# `services/bifrost/config.json` is BEARS' source of truth and includes a
# top-level `bears` extension object. Upstream Bifrost validates its runtime
# config against the official schema and rejects unknown top-level keys, so
# split the baked config into:
#   - /app/data/config.json: schema-valid upstream Bifrost config
#   - /app/data/bears-metadata.json: BEARS-only metadata sidecar payload
if command -v python3 >/dev/null 2>&1; then
  python3 - <<'PY'
import json
from pathlib import Path

source_path = Path("/app/default-config.json")
runtime_config_path = Path("/app/data/config.json")
metadata_path = Path("/app/data/bears-metadata.json")

cfg = json.loads(source_path.read_text(encoding="utf-8"))
bears = cfg.pop("bears", {}) or {}
runtime_config_path.write_text(json.dumps(cfg, indent=2) + "\n", encoding="utf-8")
metadata_path.write_text(json.dumps(bears, indent=2) + "\n", encoding="utf-8")
PY
else
  # Without python we cannot safely split JSON; keep the original behavior and
  # let the upstream image report config validation errors.
  cp /app/default-config.json /app/data/config.json
fi

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

METADATA_PATH = Path(os.environ.get("BEARS_BIFROST_METADATA", "/app/data/bears-metadata.json"))
PORT = int(os.environ.get("BEARS_METADATA_PORT", "8081"))


def load_payload():
    with METADATA_PATH.open("r", encoding="utf-8") as f:
        bears = json.load(f) or {}
    models = [m for m in bears.get("models", []) if m.get("enabled", True)]
    return {
        "object": "bears.bifrost_model_metadata",
        "metadata_version": bears.get("model_metadata_version", 1),
        "source": str(METADATA_PATH),
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
