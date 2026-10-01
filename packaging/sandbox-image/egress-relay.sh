#!/bin/sh
# Provider-owned relay process. Only a newly accepted connection which Den
# authorizes for this run/host may dial the pinned public address.
set -eu

: "${DEN_EGRESS_API_URL:?}"
: "${DEN_EGRESS_BEAR_SLUG:?}"
: "${DEN_EGRESS_RUN_ID:?}"
: "${DEN_EGRESS_HOST:?}"
: "${DEN_EGRESS_IP:?}"
: "${DEN_EGRESS_TOKEN:?}"

case "$DEN_EGRESS_API_URL" in http://*) ;; *) exit 1 ;; esac
case "$DEN_EGRESS_BEAR_SLUG" in *[!a-z0-9-]* | '') exit 1 ;; esac
case "$DEN_EGRESS_RUN_ID" in *[!0-9a-f-]* | '') exit 1 ;; esac
case "$DEN_EGRESS_HOST" in *[!a-z0-9.-]* | '') exit 1 ;; esac
case "$DEN_EGRESS_IP" in *[!0-9.]* | '') exit 1 ;; esac

case "${1:-}" in
  serve)
    exec socat TCP4-LISTEN:443,fork,reuseaddr EXEC:'/usr/local/bin/bears-egress-relay connect',nofork
    ;;
  connect)
    request=$(printf '{"jsonrpc":"2.0","id":"egress","method":"work.egress.check","params":{"bear_slug":"%s","work_run_id":"%s","host":"%s"}}' \
        "$DEN_EGRESS_BEAR_SLUG" "$DEN_EGRESS_RUN_ID" "$DEN_EGRESS_HOST")
    response=$(curl --fail --silent --show-error --max-time 3 --connect-timeout 2 --max-filesize 4096 \
        --noproxy '*' --proto '=http' -H "Authorization: Bearer $DEN_EGRESS_TOKEN" \
        -H 'BearWire-Version: 1' -H 'Content-Type: application/json' \
        --data-binary "$request" \
        "${DEN_EGRESS_API_URL%/}/bearwire/v1/rpc") || exit 1
    printf '%s' "$response" | jq -e '.error == null and .result.allowed == true' >/dev/null || exit 1
    # An already-authorized TCP stream can outlive a revocation. Bound its
    # maximum lifetime, independently of socat's idle timeout.
    exec timeout -s KILL 60 socat -T 30 STDIO "TCP4:${DEN_EGRESS_IP}:443"
    ;;
  *) exit 1 ;;
esac
