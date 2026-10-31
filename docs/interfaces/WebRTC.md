# WebRTC calls

Calls in the Comms app go through an SFU (selective forwarding unit) inside the moqui binary, a two-person call too.
There is no peer-to-peer mode. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.4 and §6. The
user guide is [Comms](../tools/Comms.md#calls).

- **Media:** `crates/moqui-rtc`, on [str0m](https://github.com/algesten/str0m) (sans-IO: ICE-lite, DTLS-SRTP, simulcast,
  TWCC). The host's driver thread (`crates/moqui-host/src/rtc/`) owns the UDP socket.
- **Signaling:** `rtc` frames on the notification socket (`crates/moqui-host/src/notws/rtc.rs`).
- **Call state:** `org.moqui.impl.CallServices` and `Calls.js` (framework), with the model in
  `framework/entity/CallEntities.xml`.
- **Browser:** `webroot/screen/webroot/js/WebrootCall.js`.

## Turning it on

Calls need a binary with the `rtc` feature (Maximal, Enterprise, Debug, or `./moq build -- --features rtc`) and a media
port:

| Property | Default | |
| --- | --- | --- |
| `rtc_media_port` | empty (off) | The UDP port browsers send media to; `0` is a free port |
| `rtc_media_host` | `127.0.0.1` | The address it binds |
| `rtc_public_address` | empty: the bound address | What browsers are told: an IP, or IP:port, for a server behind 1:1 NAT. Required when `rtc_media_host` binds every address |
| `call_ring_seconds` | `45` | How long a call rings before it is missed |

On a single server reachable at its own address, `rtc_media_port=10000` with `rtc_media_host` set to that address is
enough. Behind 1:1 NAT, bind the private address and set `rtc_public_address` to the public one, then forward UDP 10000.
The advertised address must be reachable from the browsers. A configured address does not prove it: test a call from
outside the network. Until the embedded TURN server (W4), a browser that cannot reach the UDP port cannot make a call.

The Calls screen says when calls are unavailable (`get#Capabilities`): supported (the binary has `rtc`), configured
(this server's port is set) and active (a media server is up in the cluster).

## How a call goes

1. `start#Call` (the Calls screen, the page's call store) makes the CallSession, a leg for the caller and an offered leg
   per callee. It rings each callee's tabs on `chatUser:<userId>` and by Web Push, and appends a call event to the room.
2. The callee answers (`answer#Call`). The first answer wins, and the other tabs hear it.
3. Each page sends `{rtc: 'join', callSessionId, callLegId, sdp}` on its notification socket. The socket's node runs
   `bind#Connection` as the socket's user. That checks the leg and claims its next connection generation, and the frame
   goes to the call's media node with that generation.
4. The media node answers. As other people's tracks arrive, it offers them to the page, and the page answers. Video
   goes in three simulcast layers (rids `l`, `m`, `h`), and each subscriber gets the one that fits its bandwidth
   estimate.
5. Leaving (`leave#Call`, or the socket's `leave`) closes the leg. A direct call ends with either side, and a group call
   with its last leg. A socket that closes keeps its leg for 30 seconds. A reconnect joins again with the next
   generation, and the media node refuses the old connection's frames.

## Cluster

A call's media lives on one node, `CallSession.mediaNodeId`, chosen at start among the live `CallMediaNode` rows. A frame
from a page on another node goes to that node only, on the cluster's `rtc-signal` topic (`moqui_core::cluster::send_to`),
and the replies come back the same way. A media node that stops registering (20-second heartbeat) ends its calls with
the reason "media server lost"; the users start a new call. There is no seamless failover.

## Tests

- `cargo test -p moqui-rtc`: the media gate on an in-memory network with simulated time. It covers two and five parties,
  simulcast layers following the bandwidth estimate, audio only below the floor, renegotiation both ways and glare, ICE
  restart, ICE-TCP, generations, and media taken out and put in.
- `cargo test -p moqui-host --test it rtc::`: a direct call on a real server, with str0m peers on loopback UDP signaling
  through `/notws`.
- `./moq test framework --spec Calls`: the call state machine.
- `./moq test basic-comms --spec BasicCommsCalls`: the screens' transitions through both mounts.
- `./moq test-client basic-comms --serve`: a call in two Chromium pages with fake media. It needs `rtc` in the binary.
