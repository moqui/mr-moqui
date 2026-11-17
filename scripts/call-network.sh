#!/usr/bin/env bash
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
#
# The calls' browser suite on shaped networks (plans/Comms-Messaging.md §8.4; basic-comms
# test/acceptance/client/CallNetwork.spec.js). It needs root (or CAP_NET_ADMIN with ip netns), tc and iptables:
#
#   sudo scripts/call-network.sh [--debug|--release] [--spec CallNetwork] [moqui test-client options...]
#
# Two network namespaces joined by a veth pair: mcall-srv (10.77.0.1) runs the server (moqui test basic-comms
# --acceptance --serve, with its fixtures, call media on 10.77.0.1, ICE-TCP on 10001), mcall-cli (10.77.0.2) runs the
# client (moqui test-client, and so its Chromium with both pages) and the shaper (scripts/call-network-control.py on
# 127.0.0.1:7799), which the spec asks for each profile. The server and the client run as the user who called sudo
# (embedded Postgres does not run as root). Everything is removed at the end. Without root the spec skips and says so.
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
USER_NAME="${SUDO_USER:-$(id -un)}"
as_user() { if [ "$(id -u)" = 0 ] && [ "$USER_NAME" != root ]; then sudo -u "$USER_NAME" -E "$@"; else "$@"; fi; }

if [ "$(id -u)" != 0 ] && ! ip netns add mcall-probe 2>/dev/null; then
    echo "call-network: needs root or CAP_NET_ADMIN for network namespaces and tc (sudo $0)" >&2
    exit 2
fi
ip netns del mcall-probe 2>/dev/null || true
for tool in ip tc iptables python3; do command -v "$tool" >/dev/null || { echo "call-network: $tool is missing" >&2; exit 2; }; done
[ -x "$BIN" ] || { echo "call-network: no $BIN: build it first (./moq build, or ./moq build --debug with --debug)" >&2; exit 2; }
cd "$ROOT"

OUT="$(as_user mktemp -d "${TMPDIR:-/tmp}/call-network.XXXXXX")"
READY="$OUT/ready"
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    ip netns del "$CLI" 2>/dev/null || true
    ip netns del "$SRV" 2>/dev/null || true
}
trap cleanup EXIT

ip netns add "$SRV"
ip netns add "$CLI"
ip link add mcall-veth-s type veth peer name mcall-veth-c
ip link set mcall-veth-s netns "$SRV"
ip link set mcall-veth-c netns "$CLI"
ip -n "$SRV" addr add 10.77.0.1/24 dev mcall-veth-s
ip -n "$CLI" addr add 10.77.0.2/24 dev mcall-veth-c
for ns in "$SRV" "$CLI"; do ip -n "$ns" link set lo up; done
ip -n "$SRV" link set mcall-veth-s up
ip -n "$CLI" link set mcall-veth-c up
ip -n "$CLI" route add default via 10.77.0.1

echo "call-network: the server in $SRV (log $OUT/server.log)"
ip netns exec "$SRV" env rtc_media_host=10.77.0.1 rtc_media_port=10000 rtc_media_tcp_port=10001 \
    sudo -u "$USER_NAME" -E "$BIN" test basic-comms --acceptance --spec CallClientFixtures,CryptoClientFixtures --db snapshot \
    --serve 10.77.0.1:8080 --ready-file "$READY" < <(sleep infinity) > "$OUT/server.log" 2>&1 &
PIDS+=($!)
ip netns exec "$CLI" python3 "$ROOT/scripts/call-network-control.py" "$CLI" "$SRV" mcall-veth-c mcall-veth-s 7799 \
    > "$OUT/control.log" 2>&1 &
PIDS+=($!)

for _ in $(seq 1 1800); do
    [ -s "$READY" ] && break
    if ! kill -0 "${PIDS[0]}" 2>/dev/null; then echo "call-network: the server stopped" >&2; tail -20 "$OUT/server.log" >&2; exit 1; fi
    sleep 1
done
[ -s "$READY" ] || { echo "call-network: the server did not start" >&2; exit 1; }

echo "call-network: $SPEC in $CLI"
ip netns exec "$CLI" sudo -u "$USER_NAME" -E env MOQUI_TEST_NETEM_CONTROL=http://127.0.0.1:7799 \
    "$BIN" test-client "$ROOT/runtime/components/base/basic-comms" --acceptance --server http://10.77.0.1:8080 --allow-http \
    --spec "$SPEC" --out "$OUT" "${EXTRA[@]}"
