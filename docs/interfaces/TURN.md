# TURN and ICE-TCP

Most browsers reach a call's media node on its UDP media port ([WebRTC](WebRTC.md)). Two things help the rest:

- **ICE-TCP:** a TCP port on the media node. A browser that cannot send UDP tries it first; it costs no relay.
- **TURN:** a relay for browsers behind NAT or firewalls that pass only some ports or only TLS. The relay is
  embedded in the moqui binary (`crates/moqui-rtc` `turn/`, on `turn-server-proto`; the sockets in
  `crates/moqui-host/src/rtc/turn.rs`). coturn can take its place (below). The design is
  [plans/archive/Comms-Framework.md](../../plans/archive/Comms-Framework.md) §6.5.

Both need the `rtc` feature and a media node (`rtc_media_port`). Every listener is off until its setting is set.

## Settings

| Property | Default | |
| --- | --- | --- |
| `rtc_media_tcp_port` | empty (off) | ICE-TCP, on `rtc_media_host`; browsers are told it at `rtc_public_address` |
| `turn_port` | empty (off) | TURN over UDP and TCP; 3478 by convention |
| `turn_tls_port` | empty (off) | TURN over TLS; 5349 by convention |
| `turn_tls443_host` | empty (off) | An address of its own where TURN takes TLS on 443, for networks that let only HTTPS out. The HTTPS listener cannot share it |
| `turn_host` | `rtc_media_host` | The address the TURN listeners and relays bind |
| `turn_public_host` | the media node's public address | The name or address in the URLs browsers get. TLS needs a name with a certificate in `runtime/cert` |
| `turn_relay_ports` | `49152-65535` | The UDP range of the relays |
| `turn_auth_secret` | generated | Signs the credentials. Every media node and any coturn must have the same one |
| `turn_credential_hours` | `4` | How long a credential lasts |
| `turn_external_urls` | empty | TURN servers elsewhere (coturn), comma separated `turn:` and `turns:` URLs, given to browsers beside the embedded ones |
| `turn_max_allocations`, `turn_max_kbps` | `1000`, `4000` | Allocations in all, and each allocation's bandwidth each way |
| `turn_allowed_peers` | empty | Other media nodes a relay may reach besides this one: `address:port` (their media port), or an address alone for this node's `rtc_media_port` |

The TCP and TLS connections to TURN are bounded: 512 at once and 16 from one source (an IPv4 address or an IPv6 /64);
one must send a whole message, its TLS handshake included, within 10 seconds, and one with nothing either way for 10
minutes is closed, as is one that does not read what it is sent (512 messages waiting). ICE-TCP takes 1,000 connections,
64 from one source, and closes one that sends nothing for 30 seconds.

Open these ports on the firewall:

- the TURN port, UDP and TCP;
- the TLS port, TCP;
- 443 on the `turn_tls443_host` address;
- the relay range, UDP;
- `rtc_media_tcp_port`, TCP.

## How it is used

- A leg gets its ICE servers only once it is admitted and in the call (`CallServices.get#IceServers`, or
  `get#GuestIceServers` for a guest). They are the TURN URLs the call's media node registered, and
  `turn_external_urls`. Each comes with a credential for that leg in the TURN REST form: username
  `<expiry>:<callLegId>`, password base64 of HMAC-SHA1 of the username with `turn_auth_secret`.
- A relay reaches only the media nodes, on their media ports (`rtc_media_port` as the node is reached, and the
  `address:port` entries of `turn_allowed_peers`). A permission (by address alone, RFC 8656) to any other address is
  refused; a channel to any other address or port is refused; nothing is relayed to or from another port of a media
  node's address (its SSH, database or HTTP port) or from any other address. A plain TURN credential would let a
  client relay anywhere; this rule ties it to calls. TURN relays only UDP here, so the ICE-TCP port is no relay peer.
- The exception is an XMPP user's own credential, for calls between XMPP clients ([XMPP](XMPP.md), XEP-0215), issued
  only while `xmpp_turn_enabled` is true (off by default), never to an external user (`EXTERNAL_USER`), and at most
  `xmpp_turn_per_hour` (30) a user an hour: `XmppServices.get#IceServers` makes it with the leg `xmpp.<userId>`
  (`make#IceServers`), and such a relay may reach
  public unicast addresses too, never loopback, private, unique-local, link-local, shared (100.64/10), multicast or
  documentation ranges, nor an IPv6 address carrying an inside IPv4 one (6to4, IPv4-compatible, NAT64, Teredo)
  (`moqui-rtc` `turn::is_public`), nor another port of a media node's address or of the TURN server's own address
  (a relay there comes from the server itself, past a firewall that keeps the outside off those ports). It is not
  revoked with a call, as there is none; it expires.
  coturn sees it as any other credential, so its own peer rule decides there.
- When a leg leaves or the call ends, the media node revokes the leg. Its credential is refused from then on, its
  relays close, and nothing more is relayed for it. An expired credential is refused too, refreshes included.
- Quotas: two allocations per credential, `turn_max_allocations` in all, and `turn_max_kbps` each way for each
  allocation. Data over the bandwidth is dropped.

## On an edge node

An edge node ([Edge-Node](../../plans/archive/Edge-Node.md) §5.4) runs the embedded TURN of its local nodes' calls, beside
its media socket (`turn_port` 3478 in `conf/edge/MoquiEdgeConf.xml`, relays to its own media port). The local node gives
the legs their credentials, and the edge node's TURN checks them. Each pairing has a secret of its own
([Edge-Node2](../../plans/Edge-Node2.md) §4.1, E16): base64 of HMAC-SHA1 of the pairing's id, its key's
fingerprint and when it paired, with the edge node's `turn_auth_secret`, which only the edge node holds; a node paired
again under the same id gets a new secret, and the credentials from before are refused. A node id may not hold `@`. The edge node tells a local node its pairing's secret in the answer
to the link's Hello, on an up connection whose keys both ends pinned (never in a Hello, which a check before pairing
also gets). The local node keeps it in memory, never logged, stored or shown, replaced by each new link and forgotten
at unpairing, and `make#IceServers` signs the edge node's TURN URLs with it (`mediaNodeId`), with a username that
names the pairing: `<expiry>:<callLegId>@<pairing>` (the id the local node paired as; read so only on a node that
takes links, so an `@` in a leg elsewhere, an XMPP user's, is no pairing). `turn_external_urls`
get a second entry signed with the local node's own secret and the plain username. The edge node's TURN checks a
username that names a pairing with that pairing's secret, and takes it only while the pairing is active (it reads the
active pairings every second); a leg of a pairing is never an XMPP user's own, so it relays only to the media
ports. Changing the edge node's secret takes effect for the local nodes at their next link; credentials signed with the
old one stop working on the edge node at once.

The Edge Node wizard checks this end to end (Apply and Test, plans/archive/Edge-Node.md §5.7): Apply says whether the link
handed the secret over; Test has `make#IceServers` sign the edge node's URLs for a probe leg (`edgeSetupProbe.<random>`,
no leg of a call), then allocates with that credential on the edge node's TURN over UDP, as a browser would (over
TCP when its TURN has no UDP listener; one with TLS listeners only is not checked), asks a
permission for the IP of its media address (a permission is for an IP alone; the port rule is checked on data), and deletes the allocation. A refused allocation means the secret the edge node checks
is not the one it handed over (restart it after changing `turn_auth_secret`). With `turn_public_host` a name, Apply's
DNS list checks its A and AAAA records against the edge node's addresses or its media address.

**An edge cluster** ([Edge-Node2](../../plans/Edge-Node2.md) §4.6, E18). Each member runs its own TURN, with the same
`turn_auth_secret` in the cluster's `.env`, so a pairing's secret is the same on every member. The local node keeps it
for each member its links reach and signs each member's TURN URLs for the calls whose media is there; every member
reads the active pairings from the shared rows, so a revoke ends a pairing's TURN access on all of them.

**Not through a front node** ([Edge-Node2](../../plans/Edge-Node2.md) §4.7, E19). A front node runs no TURN and passes
no UDP: before an edge cluster ([Run-Deploy, Full stack](../Run-Deploy.md#full-stack)) each edge member's
`turn_public_host` is a name with an address record at that member's own address, not the front's, and its TURN ports
are open on it. A front pairing carries no TURN secret.

**Revoking a local node ends its TURN access.** Within a second or two of the revoke the edge node's TURN refuses every
username of that pairing and closes its relays, with no new `turn_auth_secret`; the other pairings go on. A pairing's
secret makes credentials for its own pairing only: one signed for another pairing's name fails. A call's legs on an
edge node are capped per pairing (`edge_call_legs_per_pairing`, 200 at once; 0 for no cap), so one local node's calls
do not take every leg of the media node.

The other choice, the local node's secret sent to the edge node, would put a secret the local node also uses for its own
media nodes and any coturn on the cloud host; a pairing's secret only ever opens the edge node's TURN, for that
pairing.

## coturn instead

coturn (BSD-3-Clause) can do the relaying, on its own machine. Set it up for the same credentials and the same peer
rule, then name it in `turn_external_urls` and leave the embedded TURN off:

```
use-auth-secret
static-auth-secret=<turn_auth_secret>
realm=<your domain>
listening-port=3478
tls-listening-port=5349
min-port=49152
max-port=65535
# relay only to the media nodes
denied-peer-ip=0.0.0.0-255.255.255.255
allowed-peer-ip=<media node public address>
# coturn's rule is by address: keep the media node's other ports closed to the relay range on its firewall
no-multicast-peers
no-cli
fingerprint
```

With the deny-all and allow-one rule, coturn relays to nothing but the media nodes, as the embedded server does.
coturn has no hook for Moqui's leg revocation. A coturn credential lasts until `turn_credential_hours`, so keep that
short. A revoked leg's media still stops at the SFU, which drops the peer.

## Checks

- TURN: `cargo test -p moqui-rtc turn::` covers:
  - a credential relaying to and from a media node;
  - expired and forged credentials refused;
  - a peer that is not a media node refused;
  - revocation closing the relay;
  - the per-credential quota.
- ICE-TCP and TURN on a real server: `cargo test -p moqui-host --test it rtc::ice_tcp_and_a_turn_relay`. One peer is
  relayed by the embedded TURN, and the other reaches the SFU over ICE-TCP only.
- Not yet checked:
  - TURN over TLS and on 443;
  - hairpin NAT;
  - a browser behind a real UDP-blocking firewall.

  Check these on the deployment with its real network and certificates before relying on them.
