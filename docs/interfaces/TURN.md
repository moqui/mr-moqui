# TURN and ICE-TCP

Most browsers reach a call's media node on its UDP media port ([WebRTC](WebRTC.md)). Two things help the rest:

- **ICE-TCP:** a TCP port on the media node. A browser that cannot send UDP tries it first; it costs no relay.
- **TURN:** a relay for browsers behind NAT or firewalls that pass only some ports or only TLS. The relay is
  embedded in the moqui binary (`crates/moqui-rtc` `turn/`, on `turn-server-proto`; the sockets in
  `crates/moqui-host/src/rtc/turn.rs`). coturn can take its place (below). The design is
  [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §6.5.

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
- The exception is an XMPP user's own credential, for calls between XMPP clients ([XMPP](XMPP.md), XEP-0215):
  `XmppServices.get#IceServers` makes it with the leg `xmpp.<userId>` (`make#IceServers`), and such a relay may reach
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
