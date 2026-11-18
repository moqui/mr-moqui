#!/usr/bin/env bash
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
#
# The calls' browser suite on shaped networks (plans/Comms-Messaging.md §8.4; basic-comms
# test/acceptance/client/CallNetwork.spec.js). It needs root, through sudo from the user's own account, with ip, tc,
# iptables and python3:
#
#   sudo scripts/call-network.sh [--debug|--release] [--spec CallNetwork] [moqui test-client options...]
#
# Two network namespaces joined by a veth pair: mcall-srv (10.77.0.1) runs the server (moqui test basic-comms
# --acceptance --serve, with its fixtures, call media on 10.77.0.1, ICE-TCP on 10001), mcall-cli (10.77.0.2) runs the
# client (moqui test-client, and so its Chromium with both pages) and the shaper (scripts/call-network-control.py on
# 127.0.0.1:7799), which the spec asks for each profile. Only the namespaces, the links and the shaper run as root: the
# server and the client run as the user who called sudo, in that user's own environment (embedded Postgres does not
# run as root). Everything is removed at the end, and what an earlier run left is removed before. Without root the
# spec skips and says so.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE_DIR=release
SPEC=CallNetwork
EXTRA=()
while [ $# -gt 0 ]; do
    case "$1" in
        --debug) PROFILE_DIR=debug ;;
        --release) PROFILE_DIR=release ;;
        --spec) SPEC="$2"; shift ;;
        *) EXTRA+=("$1") ;;
    esac
    shift
done
BIN="$ROOT/target/$PROFILE_DIR/moqui"
SRV=mcall-srv
CLI=mcall-cli
VETH_S=mcall-veth-s
VETH_C=mcall-veth-c

if [ "$(id -u)" != 0 ]; then
    echo "call-network: needs root for network namespaces and tc: sudo $0" >&2
    exit 2
fi
USER_NAME="${SUDO_USER:-}"
if [ -z "$USER_NAME" ] || [ "$USER_NAME" = root ]; then
    echo "call-network: run it with sudo from your own account: the server and the browsers run as that user" >&2
    exit 2
fi
for tool in ip tc iptables python3 sudo; do command -v "$tool" >/dev/null || { echo "call-network: $tool is missing" >&2; exit 2; }; done
[ -x "$BIN" ] || { echo "call-network: no $BIN: build it first (./moq build, or ./moq build --debug with --debug)" >&2; exit 2; }
cd "$ROOT"
# as the user, in their own environment (HOME, PATH, ...), with only the variables named
as_user() { sudo -u "$USER_NAME" -H "$@"; }

# what an earlier run left: its namespaces, and a veth still in this namespace if it stopped half way
clear_links() {
    ip netns del "$CLI" 2>/dev/null || true
    ip netns del "$SRV" 2>/dev/null || true
    ip link del "$VETH_S" 2>/dev/null || true
    ip link del "$VETH_C" 2>/dev/null || true
}
clear_links

OUT="$(as_user mktemp -d "/tmp/call-network.XXXXXX")"
READY="$OUT/ready"
# the server's standard input: it stops when this closes (moqui test --serve), so no process is left to hold it
STDIN="$OUT/stdin"
as_user mkfifo "$STDIN"
PIDS=()
cleanup() {
    exec 3>&- 2>/dev/null || true
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    clear_links
}
trap cleanup EXIT

ip netns add "$SRV"
ip netns add "$CLI"
ip link add "$VETH_S" type veth peer name "$VETH_C"
ip link set "$VETH_S" netns "$SRV"
ip link set "$VETH_C" netns "$CLI"
ip -n "$SRV" addr add 10.77.0.1/24 dev "$VETH_S"
ip -n "$CLI" addr add 10.77.0.2/24 dev "$VETH_C"
for ns in "$SRV" "$CLI"; do ip -n "$ns" link set lo up; done
ip -n "$SRV" link set "$VETH_S" up
ip -n "$CLI" link set "$VETH_C" up
ip -n "$CLI" route add default via 10.77.0.1

echo "call-network: the server in $SRV (log $OUT/server.log)"
exec 3<>"$STDIN"
ip netns exec "$SRV" sudo -u "$USER_NAME" -H env moqui_conf__media_server__host=10.77.0.1 moqui_conf__media_server__port=10000 moqui_conf__media_server__tcp_port=10001 \
    "$BIN" test basic-comms --acceptance --spec CallClientFixtures,CryptoClientFixtures --db snapshot \
    --serve 10.77.0.1:8080 --ready-file "$READY" < "$STDIN" > "$OUT/server.log" 2>&1 &
PIDS+=($!)
ip netns exec "$CLI" python3 "$ROOT/scripts/call-network-control.py" "$CLI" "$SRV" "$VETH_C" "$VETH_S" 7799 \
    > "$OUT/control.log" 2>&1 &
PIDS+=($!)

for _ in $(seq 1 1800); do
    [ -s "$READY" ] && break
    if ! kill -0 "${PIDS[0]}" 2>/dev/null; then echo "call-network: the server stopped" >&2; tail -20 "$OUT/server.log" >&2; exit 1; fi
    sleep 1
done
[ -s "$READY" ] || { echo "call-network: the server did not start" >&2; exit 1; }

echo "call-network: $SPEC in $CLI"
ip netns exec "$CLI" sudo -u "$USER_NAME" -H env MOQUI_TEST_NETEM_CONTROL=http://127.0.0.1:7799 \
    "$BIN" test-client "$ROOT/runtime/components/base/basic-comms" --acceptance --server http://10.77.0.1:8080 --allow-http \
    --spec "$SPEC" --out "$OUT" "${EXTRA[@]}"
