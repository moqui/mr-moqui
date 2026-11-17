#!/usr/bin/env python3
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
"""The network shaper of the calls' browser suite (plans/Comms-Messaging.md §8.4; scripts/call-network.sh starts it).

It listens on 127.0.0.1 in the client namespace and applies a named profile with tc netem (Linux network namespaces,
root or CAP_NET_ADMIN) to:
  - the client's link to the server, both ways (the veth egress in the client and in the server namespace): what the
    pages send to the media server and what it sends them (the relay path, and the signaling socket);
  - UDP on the client namespace's loopback: the two pages are in one Chromium in that namespace, so their direct path,
    peer to peer, runs over lo (TCP there, Chromium's DevTools socket, is left alone).
A spec (CallNetwork.spec.js) asks for a profile with POST /profile {"name": ...}; only the names below are taken.
"""
import http.server
import json
import subprocess
import sys

CLIENT_NS, SERVER_NS = sys.argv[1], sys.argv[2]
CLIENT_DEV, SERVER_DEV = sys.argv[3], sys.argv[4]
PORT = int(sys.argv[5]) if len(sys.argv) > 5 else 7799

# netem arguments of each profile, for each direction: up (the pages to the server), down (back), and the direct
# path (lo, each way once). None leaves that direction clean.
PROFILES = {
    "clean": (None, None, None),
    "lte": ("delay 25ms rate 10mbit", "delay 25ms rate 20mbit", "delay 25ms"),
    "congestedLte": ("delay 60ms 20ms loss 1% rate 1mbit", "delay 60ms 20ms loss 1% rate 2mbit", "delay 60ms 20ms loss 1% rate 2mbit"),
    "3g": ("delay 150ms loss 1% rate 384kbit", "delay 150ms loss 1% rate 1500kbit", "delay 150ms loss 1% rate 1500kbit"),
    "edge": ("delay 200ms rate 100kbit", "delay 200ms rate 200kbit", "delay 200ms rate 200kbit"),
    "loss5": ("loss 5%", "loss 5%", "loss 5%"),
    "loss10": ("loss 10%", "loss 10%", "loss 10%"),
    "loss20": ("loss 20%", "loss 20%", "loss 20%"),
    # Gilbert-Elliott: 3% into a burst, 33% out of it, all lost in it (about 8%, three in a row)
    "burst": ("loss gemodel 3% 33% 100% 0%", "loss gemodel 3% 33% 100% 0%", "loss gemodel 3% 33% 100% 0%"),
    "jitter200": ("delay 100ms 100ms distribution uniform", "delay 100ms 100ms distribution uniform",
                  "delay 100ms 100ms distribution uniform"),
    "bw5m": ("rate 5mbit", "rate 5mbit", "rate 5mbit"),
    "bw300k": ("rate 5mbit", "rate 300kbit", "rate 300kbit"),
    "outage": ("loss 100%", "loss 100%", "loss 100%"),
    "satellite": ("delay 300ms", "delay 300ms", "delay 300ms"),
    "uplink50k": ("rate 50kbit", None, "rate 50kbit"),
}


def run(ns, *args):
    cmd = ["ip", "netns", "exec", ns] + list(args)
    out = subprocess.run(cmd, capture_output=True, text=True)
    return out.returncode, (out.stdout + out.stderr).strip()


def shape(ns, dev, netem):
    run(ns, "tc", "qdisc", "del", "dev", dev, "root")
    if netem:
        code, text = run(ns, "tc", "qdisc", "add", "dev", dev, "root", "netem", *netem.split(), "limit", "1000")
        if code:
            raise RuntimeError(f"tc on {dev}: {text}")


def shape_lo_udp(netem):
    """UDP on the client namespace's lo through netem, TCP straight (a prio qdisc with a u32 filter on protocol 17)."""
    run(CLIENT_NS, "tc", "qdisc", "del", "dev", "lo", "root")
    if not netem:
        return
    for args in (
        ["tc", "qdisc", "add", "dev", "lo", "root", "handle", "1:", "prio", "bands", "3"],
        ["tc", "qdisc", "add", "dev", "lo", "parent", "1:3", "handle", "30:", "netem"] + netem.split() + ["limit", "1000"],
        ["tc", "filter", "add", "dev", "lo", "parent", "1:0", "protocol", "ip", "u32", "match", "ip", "protocol", "17", "0xff",
         "flowid", "1:3"],
    ):
        code, text = run(CLIENT_NS, *args)
        if code:
            raise RuntimeError(f"tc on lo: {text}")


def block_udp(on):
    """UDP to the server dropped in the client namespace (the server's TCP candidates, ICE-TCP or TURN over TCP, stay)."""
    run(CLIENT_NS, "iptables", "-D", "OUTPUT", "-o", CLIENT_DEV, "-p", "udp", "-j", "DROP")
    if on:
        code, text = run(CLIENT_NS, "iptables", "-A", "OUTPUT", "-o", CLIENT_DEV, "-p", "udp", "-j", "DROP")
        if code:
            raise RuntimeError(f"iptables: {text}")


def new_address():
    """Wi-Fi to mobile data: the client namespace's address changes (10.77.0.2 and .3 in turn, call-network.sh's
    subnet); the old one is gone, so its sockets and candidates stop, and the pages must restart ICE."""
    code, text = run(CLIENT_NS, "ip", "-4", "-o", "addr", "show", "dev", CLIENT_DEV)
    old = "10.77.0.3" if "10.77.0.3/" in text else "10.77.0.2"
    new = "10.77.0.2" if old == "10.77.0.3" else "10.77.0.3"
    for args in (
        ["ip", "addr", "add", new + "/24", "dev", CLIENT_DEV],
        ["ip", "addr", "del", old + "/24", "dev", CLIENT_DEV],
        ["ip", "route", "replace", "default", "via", "10.77.0.1", "src", new],
    ):
        code, text = run(CLIENT_NS, *args)
        if code:
            raise RuntimeError(f"{' '.join(args)}: {text}")


def apply(name):
    if name == "newAddress":
        new_address()
        return
    if name == "udpBlocked":
        apply("clean")
        block_udp(True)
        return
    if name not in PROFILES:
        raise KeyError(name)
    block_udp(False)
    up, down, direct = PROFILES[name]
    shape(CLIENT_NS, CLIENT_DEV, up)
    shape(SERVER_NS, SERVER_DEV, down)
    shape_lo_udp(direct)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
            apply(str(body.get("name", "")))
            self.answer(200, {"applied": body.get("name")})
        except KeyError as e:
            self.answer(400, {"error": f"no profile {e}"})
        except Exception as e:  # noqa: BLE001 - the spec gets the reason
            self.answer(500, {"error": str(e)})

    def do_GET(self):
        self.answer(200, {"profiles": sorted(list(PROFILES) + ["udpBlocked", "newAddress"])})

    def answer(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        sys.stderr.write("call-network-control: " + (fmt % args) + "\n")


if __name__ == "__main__":
    apply("clean")
    http.server.HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
