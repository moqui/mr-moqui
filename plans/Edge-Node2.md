# Edge Node 2: domains, pools, edge clusters and the front

Status 2026-11-14: plan; E13 (example confs), E14 (root screens by host name, [§4.2](#42-root-screens-by-host-name)) and E15 (certificates by ACME, [§4.8](#48-certificates-and-acme)) are built ([§5](#5-phases)), and E16's routing by owner for web, mail, chat, PASS, delegate and the queue, with claims and limits between pairings, its comms (TURN, SIP, XMPP and ports per pairing) ([§4.1](#41-domains-on-one-edge-node), As built), and its System screens and `conf/multi-domain` ([§4.10](#410-the-system-app), As built), so E16 is built; E17 part A: the roles, every member of a local cluster linking, the member list, admission, member state and drain, and failover to a standby with its notice and Deliver again since ([§4.3](#43-the-front-option-set) to [§4.5](#45-failover-to-a-standby), As built); E17 part B: balancing by protocol with sticky sessions, health and retry, and call signaling per member ([§4.4](#44-pools-load-balancing), As built); E17 part C: work from a pool (the outbox, replication by a lock row) and web protection's rules per host, shared bans and view ([§4.3](#43-the-front-option-set), [§4.4](#44-pools-load-balancing), As built); E17 part D: the Clients screen's members, takeovers, hold and Deliver again, the Balancing tab and the sticky cookie on Domains, a local node's own cluster and drain on Edge Node, and the `local-cluster` (with an edge node) and `local-failover` confs ([§4.10](#410-the-system-app), As built); E18 part A: a local node's link to every member of an edge cluster ([§4.6](#46-a-cluster-of-edge-nodes), As built); E18 part B: the edge members' in-memory state made right by a cluster notice, admission and revocation across the edge cluster, SIP trunks registered from one member and calls on every member ([§4.6](#46-a-cluster-of-edge-nodes), As built); E18 part C: the Edge cluster tab, System › Edge Node's links to each edge member and `conf/edge-cluster` ([§4.10](#410-the-system-app), As built); E19 part A: the front node, an edge node that dials its front, and a local node's second pairing ([§4.7](#47-a-front-node), As built); E19 part B: TCP ports on a front (SMTP to an edge cluster with a PROXY header) and the edge members' link names through it ([§4.7](#47-a-front-node), As built); E19 part C: `conf/front` and `conf/full-stack`, the guides and the front node on the System screens ([§4.9](#49-example-confs-for-each-topology), [§4.10](#410-the-system-app), As built); so E13 to E19 are built; left before 0.1.0 are the hand checks of [§2.1](#21-checks-on-a-deployed-edge-node), E19's among them (the full stack with real DNS and an mr-pass site, a front's port 25 to a local cluster, two front nodes), and after it E11 and Later. It follows [Edge-Node](archive/Edge-Node.md), archived the same day
with E0–E10 and E12 built: the link, web access, delegate, queue and replicate, mr-pass on gRPC, mail, chat webhooks,
calls, SIP, the XMPP tunnel, the attach wizard and the all-in-one node. This plan carries what that one left open
([§2](#2-open-from-edge-node)) and adds what larger deployments need, from the user's notes and answers of Nov 12,
2026: several domains on one edge node, root screens by host name, the front as an option set of its own, pools of
local nodes with load balancing, failover to a standby, a cluster of edge nodes, a front node, certificates by ACME
from a screen, an example conf for every topology, and the System app screens to set it all up. Phases E13 to E19 are
for the first release ([§5](#5-phases)). The questions are answered ([§7](#7-decisions)); what the user's review
pass should look at is in [§8](#8-for-the-review-pass).

Sections: [why](#why-and-why-before-010) · [1 terms](#1-terms) · [2 open from Edge-Node](#2-open-from-edge-node) ·
[3 topologies](#3-topologies) · [4 design](#4-design) · [5 phases](#5-phases) ·
[6 documents to change](#6-documents-to-change) · [7 decisions](#7-decisions) ·
[8 for the review pass](#8-for-the-review-pass)

---

## Why, and why before 0.1.0

The user's call of Nov 12, 2026: the bulk of this plan comes before the first release.

- **It sets MR Moqui apart.** Other systems leave the layers around the application to other products: a load
  balancer, a reverse proxy, a web application firewall, a mail gateway, a TURN server, a SIP edge, certificate
  tooling, a VPN or port forwards with dynamic DNS. Here each is a role of the same binary.
- **It saves administration.** Nodes that know each other set each other up. A local node pairs with a code, reads
  what the edge node offers, applies its part on both sides, checks the DNS records and tests the result. Nothing is
  kept in step by hand across products, and one System app shows all of it.
- **It opens these topologies to small businesses and homelabs.** A server at home or in the office with a public
  presence, a pair of machines with a standby, a small cluster behind a front: each is out of reach today for someone
  without an operations team. The same pieces then scale up to a shop on an edge cluster.

## 1. Terms

- **Local node.** The node that holds the data and dials the edge node, as in Edge-Node. It keeps the name wherever
  it runs, an office, a home or a data center: there is no place like localhost.
- **Pairing.** One `EdgeClient` row on the edge node: one key, pinned at pairing. Today a pairing is one node. With
  pools it is one local node or one cluster of them.
- **Domain.** What an edge node answers for one pairing: its host names, mail domains, XMPP domain, SIP realm and
  numbers, chat bridges and PASS sites.
- **Pool.** The linked members of one pairing: cluster members on one database that serve the same names.
- **Standby.** A second machine for a local node, started when the first one dies
  ([§4.5](#45-failover-to-a-standby)).
- **Front.** The option set that faces browsers: links taken, web access by host name, load balancing, web
  protection, TLS and certificates ([§4.3](#43-the-front-option-set)). A **front node** (`node_role=front`) has
  nothing else. An edge node as built has it with the edge set (`front_edge`), and one behind a front node may leave
  it out (`edge`).
- **Edge cluster.** Several edge nodes on one database, each taking links.

## 2. Open from Edge-Node

Everything the archived plan left open, in one place. Each row says where it goes: a phase here, Later (§5), or
another plan.

### 2.1 Checks on a deployed edge node

None has been run. Each needs a served Maximal build with a publicly trusted certificate and real DNS. They are hand
checks, done once, as E4's two-runtime check was over REST and direct gRPC.

| Check | From |
|---|---|
| mr-pass's two-runtime check (RemoteHttp) with the authority on the link | E4 |
| Mail out DKIM-signed and passing DMARC at a large provider: with the MTA in moqui (MTA-STS, reverse DNS), and with Postfix and Rspamd beside the edge node | E5a, E5b |
| A Telnyx round trip: a text in while the local node is away, an MMS out served by the edge node | E6 |
| A call between an outside browser and an office browser over the internet, through the edge node's media address and TURN (a real `turn_public_host` in DNS); captions through the edge node, which need a transcription provider | E7, E10 calls |
| A real trunk and desk phones, with real SRV records; the voicemail fallback's forward end to end | E8, E10 phones |
| An XMPP client (Conversations) through the tunnel ports | E9 |
| The whole wizard in one sitting, pairing through the EdgeUrl and Connect screens included | E10 |
| A real local node paired with a served all-in-one server, with the server's own mail, chat, calls, SIP and XMPP beside the node's | E12 |
| A shell through the edge node in both web access modes, across a dropped link; the desktop app's pairing checks | [Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md) |
| Real issuance from System › Certificates, Let's Encrypt staging then production, for an edge node's terminate host and for a local node's passthrough name through the tunnel, and their renewal by the job; the set-up check through a home router | E15 |
| The full stack with real DNS: an mr-pass site through a front node over two edge members with a local cluster as its authority (the acceptance test serves a plain screen site), mail through the front's port 25 with the SPF, MX certificate and MTA-STS of [Mail-Inbound](../docs/interfaces/Mail-Inbound.md), each tier's node stopped in turn; a front's port 25 to a local cluster's members (no test); two front nodes as a front cluster (no test) | E19 |

### 2.2 Not built

| Item | What is missing | Goes to |
|---|---|---|
| Several local nodes on one edge node (decision 1) | Built (E16): every area by its owner, claims and limits per pairing ([§4.1](#41-domains-on-one-edge-node)), the Clients, Domains and Ports screens ([§4.10](#410-the-system-app)) and `conf/multi-domain` | E16, built |
| A clustered local node | One member was to hold the link through a lock row; the lock row is not built. The link is matched by key, so members that share a `.env` would replace each other's link (to verify). A call's socket on a member without the link is refused | E17: every member links. Built in part A ([§4.4](#44-pools-load-balancing), As built): each member's link apart, admission by the member list; in part B balancing by protocol, health and call signaling per member; in part C the outbox from any member and replication from the holder of a lock row |
| A second edge node | A secondary MX, a second webhook URL | E18, built. Part A: a local node links to every edge member ([§4.6](#46-a-cluster-of-edge-nodes), As built); part B: the in-memory state, admission and revocation across members, SIP and calls on every member; part C: the screens and `conf/edge-cluster` ([§4.10](#410-the-system-app), As built) |
| A clustered all-in-one server | Which member holds the edge role | E18: every member takes links. Part A: verified, and its acceptance test; [Mobile-Desktop](Mobile-Desktop.md)'s open question answered (part C) |
| PASS sites for a paired node on an all-in-one node | `pass_backend_url=edge:` means one authority for the whole node | E16: the authority by site, built: `edge:` goes to the pairing whose Configure names the call's site. Left: the server's own sites on the same node, which `edge:` sends to the one linked node when no pairing names them |
| XMPP for two domains on one port | Built (E16 part B): SNI on direct TLS, the stream's `to` on STARTTLS, on the edge node's tunnel ports and an all-in-one node's own ports; a pair of ports per node is `EdgePort` rows | E16, built |
| All-in-one gaps | Built (E16 part B): a voicemail sidecar names its pairing, so a requeue after a restart goes to it; a paired number with no fallback row is found by its `PhoneNumber` copy (`voiceTrunks`) | E16, built |
| A TURN secret per pairing | Built (E16 part B): a secret per pairing derived from `turn_auth_secret`, the pairing in the TURN username, refused once the pairing is revoked | E16, built |
| ACME's other challenges | TLS-ALPN-01 from System › Certificates is built (E15, [§4.8](#48-certificates-and-acme); [Dogfood](Dogfood.md) F10). HTTP-01 where port 80 is open, and DNS-01 for wildcard names, are not | Later |
| The System menu | Built (E16): no framework change was needed, as `default-menu-include` already takes an expression (`Tools.xml` hides Load by one); Edge Clients lists itself only where `node_role` is set (`front`, `front_edge`, `edge` or `all-in-one`, E17). Edge Node still shows on every node | E16, built |
| The edge node's backup job | Built (E13): the property `embedded_db_backup_scheduled=true`, which each boot applies, sets the paused seed job `backup_EmbeddedDatabase_daily` running; `conf/edge/MoquiEdgeConf.xml` and `conf/all-in-one/MoquiAllInOneConf.xml` set it | E13, built |
| Filtering in moqui (E11) | Content and reputation scoring for mail, internal mail, public forms and outside chat, in place of the external Rspamd | E11, after the first release |
| A PASS read cache on the edge node | Public pages read while the local node is away; writes fail closed | Later |
| Monitoring over the link (§5.9) | The edge node's OTLP export to the local node's Monitor, the alerts (link down, an old queued item, a quota nearly reached), the wizard's Monitoring area | Later; with several pairings each gets its own part |
| Sealing queued items | Each queued item sealed to the local node's public key once the edge node has made its checks | Later, with [Sealed-Secrets](Sealed-Secrets.md) |
| XMPP federation (C3b) and Matrix federation (C6b) on the edge node | Designed in [Edge-Node §5.4](archive/Edge-Node.md#54-comms-on-the-edge-node) | [Future-Plans, Comms](Future-Plans.md#comms) |
| A local media node cascading to the edge node; an edge node with regional media nodes | W7's relay links need a shared database; the link carries none | Later |
| The voicemail fallback | It follows no call-handling rules or schedules | Later |
| The wizard | Turning an area off leaves what an earlier Apply set; the mail-out test sends nothing to an outside address | Later |
| PASS over direct gRPC | Mutual TLS or request signing in place of the client secret; typed messages (decision 7) | Later |
| QUIC for the link; DANE for the MTA's outbound TLS | | Later |

### 2.3 Test gaps

- The `_edge_` refusals of `SipDevices.js` while phones are off: the spec engine can set a process property now
  (`setProperty`, `withProperty`, which `EdgeScreens.spec.js` uses for `node_role`), so this spec is left to write.
- Mantle's own `handle#Call` through the host: loading mr-mantle in a host test is too slow for a fast test.
- The TURN check over TLS only (over TCP is checked since E16 part B, `rtc::turn_probe`); the 409 on the chat media
  path of an all-in-one node. The same SIP username in two domains is checked since E16 part B (`sip::owners`
  `phone_key`, by its key; not with two real phones).
- `edge_link::pairing_links_both_ways_and_unpairing_cuts_the_link` failed once on a port already in use
  ([Remaining-Work](Remaining-Work.md#tests)).

## 3. Topologies

The topologies this plan ends with. Each gets a directory of example confs under `runtime/conf/`, named for what
sets it apart ([§4.9](#49-example-confs-for-each-topology)).

| Topology | Use it for | Conf directory | State |
|---|---|---|---|
| One node | Development, one production server, the demo | `conf/` itself | Built |
| One node, several sites by host name | Separate sites on sub-domains or related domains, one server | `conf/` (`MoquiSitesConf.xml`) | Built (E14) |
| PASS servers with a BOSS authority | Public sites with no business data on the public machines, direct REST or gRPC | `conf/pass/` | Built, with its confs (E13) |
| Dev instances with a group server | A team | `conf/dev-group/` | Built, with its confs (E13) |
| A local node with an edge node | A server in the office or at home with a public presence; the desktop and mobile apps as its clients | `conf/edge/` | Built, with its confs (E13); the apps' exit checks are open |
| A local node with an all-in-one node | The same, with a full public server as the edge node | `conf/all-in-one/` | Built, with its confs (E13) |
| Several local nodes on one edge node | Branch offices, or separate organizations of one group, each with its own domains | `conf/multi-domain/` | Built (E16), with its confs |
| A local cluster | More capacity, or a member that can fail: pooled and balanced by an edge node, or behind a load balancer of your own | `conf/local-cluster/` | Built behind your own balancer, with its conf (E13); with an edge node, `MoquiPooledConf.xml` (E17) |
| A local node with a standby | A second machine started when the first one dies | `conf/local-failover/` | Built, with its conf (E17) |
| An edge cluster | An edge node that can fail, or more capacity at the edge | `conf/edge-cluster/` | Built (E18 parts A to C), with its conf; cascading call media between members is Later, so a phone rings only for calls whose media is on its member |
| A front node before a local cluster | Load balancing and web protection, with none of the edge node's comms | `conf/front/` | Built (E19 part A), with its confs (part C) |
| Full stack | A higher-volume shop or other public site on mr-pass: a local cluster, an edge cluster, a front node, the desktop and mobile apps | `conf/full-stack/` | Built (E19 parts A and B), with its confs (part C) |

```
 visitors, mail servers, providers
    |  443, 25
 front node                   balances by host name, web protection, certificates; no data of its own      (E19)
    |  each edge member dials the front
 edge cluster                 edge 1 .. edge n on one database: mr-pass sites, mail, webhooks, media, SIP  (E18)
    |  each local member dials every edge member
 local cluster                local 1 .. local n on one database: the apps, the data, the PASS authority   (E17)
```

## 4. Design

### 4.1 Domains on one edge node

One edge node serves several pairings, each with its own domains, and routes everything by name to its owner. This is
"several local nodes per edge node" of Edge-Node's decision 1, which E12 half built: an all-in-one node already
routes each item to the server itself or to its one paired node.

**The model.** A dedicated edge node becomes an all-in-one node with no items of its own. E12's owner scope
(`moqui_core::edge::owner`), claims (`edge/claims.rs`) and replica conflicts then apply between any two owners, not
only between the server and one node.

**What still takes "the only paired node"** and must take an owner instead:
- `EdgeServer::only_link`: a delegated call on a dedicated edge node (refused with several linked), and the PASS
  authority behind `pass_backend_url=edge:`;
- `queue::enqueue` for an item that names no node;
- the SIP engine's config and fallbacks (`sip/edge.rs` `paired_node`), and a voicemail sidecar requeued after a
  restart (built, E16 part B: every pairing's, by owner);
- the SSH tunnel (`edge_ssh_tunnel_node`) and the XMPP tunnels (`edge_xmpp_node`): one node per port (built, E16
  part B: `EdgePort` rows and XMPP by domain);
- a chat bridge id that two nodes have, which gets 401;
- the offer's `other_nodes` warnings for calls and phones (E16 part B: no longer read on the local node; the offer
  still sets them until part A changes `offer.rs`).

**Routing by area.**

| Area | The key that names the owner | Change |
|---|---|---|
| Web access | The host name (`EdgeHost`) | None. With pools the target is a pool ([§4.4](#44-pools-load-balancing)) |
| Mail in | The recipient address, in each pairing's replicated recipients | Built per node. An address two pairings hold gets 451, as on an all-in-one node |
| Mail out | The item's pairing | Each pairing's mail is signed only for its own domains (E12's `auth::paired_domains`) |
| Chat webhooks | The bridge id within the request's host name | Ids come from separate databases and can collide, so the host name picks the pairing first |
| Calls | Rooms keyed by node and call (E12) | Recordings and captions go by the room's owner (E12). TURN gets a secret per pairing, with the pairing named in the TURN username |
| SIP | The number dialled, the device's realm, the trunk (E12) | Trunks merged from every pairing's copy; a number or realm two pairings claim is refused |
| XMPP | The XMPP domain: SNI on the direct TLS port, the stream's `to` on the STARTTLS port | One pair of ports for every domain. A pair of ports per node stays an option |
| SSH | A port per pairing | Rows the System app sets, in place of the two properties ([§4.10](#410-the-system-app)) |
| PASS | The site: a `public-site` host names a site, and a pairing's Configure names its sites | mr-pass asks for the authority by site. A PASS client id per pairing |
| Delegate | The owner scope | On a dedicated edge node too |

**Claims.** Each name, address, number, bridge, domain, realm and site has one owner. The first claim holds. A later
one is refused in Configure, with the generic "another node has this name", and a replicated record that claims it
is kept as a conflict that Edge Clients shows, as E12 does for the server's own items.

**As built (E16, the routing and claims part).**
- **The only paired node** is gone from these paths. `queue::enqueue` with no node takes the owner scope's
  (`queue::owner_node`), else the only paired node, and is refused and logged with several and no scope. A delegated
  call goes by the owner scope on a dedicated edge node as on an all-in-one node. A call with no owner (a delegated
  call outside a scope, a PASS call for a site no pairing names) goes to the only active pairing
  (`EdgeServer::only_pairing_link`), counted by the pairings and not by the links up, so with one pairing away it is
  refused ("needs its owner") rather than sent to the other.
- **Web access** needed no change; each pairing's host reaches only its node (tested).
- **Mail in.** An address two active pairings' copies both have at the same level gets 451, logged (`smtp_queue.rs`,
  `Lookup::Held`); the most specific match wins (one's `joe+x@` as it is over another's `joe@`).
  Replication keeps the later copy as a conflict, so this is a race or copies from before. **Mail out** was already
  signed per pairing (`auth::paired_domains`).
- **Chat webhooks.** The request's host name picks the pairing: at a pairing's own terminate host only its copy, at
  the edge node's own names the one pairing with the id (two: 401, logged). So that a pairing's host reaches the
  handler, the edge node answers `/channels/{id}/webhook` and `/channels/{id}/media/...` itself on a terminate host
  for the bridges that pairing's Configure lists (`chatBridgeIds`), queued while the node is away, and sends the rest
  of the host's paths down the link as before (`hosts.rs` `answers_here`; only when this node takes webhooks, the
  `channels` feature and `channels_webhooks_enabled`). A file is served at a pairing's host only if it is that
  pairing's. The offer's chat address is still the edge
  node's own: a pairing whose ids may collide sets `channels_public_url` to one of its terminate hosts.
- **PASS.** `edge:` goes to the pairing whose Configure names the call's site (`EdgeServer::pass_link`, from a site
  map the host table keeps and Configure, pairing, revoking and the host services load again); a site no pairing
  names goes to the only active pairing, and is refused with several. A site two pairings name (concurrent
  Configures) is logged and refused. The
  PASS client id per pairing is each local node's own `pass_edge_client`: each authority is its own database, so the
  ids never meet, and the offer still suggests `edge`.
- **Claims** (`edge/claims.rs`). `check_setup_claims` takes the pairing and refuses, on any node that takes links, an
  XMPP or SIP domain or a PASS site (a new `ClaimKind::PassSite`) another active pairing named first, with the generic
  "another node has this name" (the Configure text for a host name too, which was "another local node has this
  name"). Replica conflicts (`replica_conflicts.rs`, `Owners`) apply between pairings on a dedicated edge node with
  more than one: a mail address or number another pairing's copies hold. A chat bridge id is the exception: two
  pairings may share one, as the plan's chat row says. Mail domains are split by address between pairings, as with
  the server. The mail addresses of all copies are read once per batch (`claims::MailIndex`, kept by the batch, so a
  batch rolled back leaves nothing behind). Edge Clients'
  conflicts service shows them with the holder ("paired node local-a"), and the periodic recheck now runs on a
  dedicated edge node too.
- **Limits.** The queue's size cap is per pairing (it was node-wide): `queue-max-megabytes` for each, or
  `EdgeClient.queueMaxMegabytes` (above 0; 0 or less is unset). The MX delivery reads each active pairing's due rows
  apart (one that cannot be read is logged and waits) and gives each pairing with rows due an even share of its 8
  sessions, one held back while more than one pairing is active. A cap on call legs per pairing is not built (the calls' own part).
- **Tests.** Unit tests in `moqui-core` (claims, conflicts, owner, queue cap, chat host routing), `moqui-host`
  (`smtp_queue`, `hosts`) and `moqui-mail` (`mx`); `tests/it/edge_domains.rs` serves two local nodes and an edge
  node: each pairing's web, mail, webhook, PASS site and delegated call reach only it, and a name, site and address
  both claim stay with the first. Its `#[ignore]` acceptance test adds one node away and then overloaded while the
  other works.

**Isolation.** Reasonable, not multi-tenant. One operator runs the edge node, and the pairings belong to one
organization or to related ones, as the organizations inside one Mantle database do. The aim is that a mistake or an
overload in one domain does not break or leak into another. It is not a defense against a hostile pairing.
- **Kept apart:** every edge row is keyed by pairing, and no call from one pairing reads another's rows. Each pairing
  has its own limits: the queue's size cap (node-wide before E16, per pairing now), streams and tunnels on its links
  (built per node), MMS files (built per node), a share of the MX delivery threads, and a cap on call legs.
- **Shared on purpose:** the edge node's addresses and their mail reputation, the DKIM key and selector (one key,
  published in each domain's DNS), the media and TURN ports, web protection's counts, CPU and bandwidth.
- **Not provided:** an operator per pairing with its own view of the edge node, billing, or quotas enforced against
  a pairing that attacks.

**As built (E16 part B, the comms, Nov 13, 2026).**
- **Calls and TURN.** A TURN secret per pairing: `moqui_rtc::turn::pairing_secret` (HMAC-SHA1 of the pairing's id, its key's
  fingerprint and when it paired, with `turn_auth_secret`, so pairing again under an id gives a new secret), sent in that pairing's Hello answer. The local node names the id it paired as in each username
  (`<expiry>:<leg>@<pairing>`); the TURN server checks it with that pairing's secret and takes it only while the
  pairing is active (it reads them every second, `rtc/mod.rs` `pairings_loop`), so a revoke refuses its credentials
  and closes its relays with no new `turn_auth_secret`. A pairing's leg is never an XMPP user's own. The media driver
  revokes a paired leg by leg and pairing, and caps one pairing's legs at once (`edge_call_legs_per_pairing`, 200).
  Recordings and captions already went by the room's owner (E12): checked, unchanged.
- **SIP.** The engine loads every pairing's trunks (`sip/edge.rs` `engine_config`), each keyed by its node, with the
  first pairing's policy on a dedicated edge node. A number's owner is the pairing whose copies have it
  (`moqui_core::edge::sip::number_holders`): `PhoneNumber` now replicates in `voiceTrunks` (its trunk alone), so a
  number with no fallback row is found; two holders refuse the call (404, logged). A pairing's `sip_domain` is kept
  from its Configure on a dedicated edge node too; the registrar takes each pairing's domain as a device realm, and
  its Hello and offer name it (`moqui_core::edge::sip::pairing_realm`); a realm two pairings name is refused, and the
  edge node's own realm is the one pairing's only while one is paired. Fallbacks are read from the owner's copy, a
  voicemail sidecar names its pairing, and each offer lists its own trunks. The local side's "other local nodes"
  refusals for calls and phones are gone; the offer's `other_nodes` fields are no longer read.
- **XMPP.** `edge_xmpp_port` and `edge_xmpp_tls_port` read each connection's domain first (`edge_link/xmpp_domain.rs`:
  SNI, or the stream header's `to`, at most 16 KB in 10 s) and send it to the pairing whose Configure named it; with
  one pairing active what names no one's still goes to it (never to another pairing while one is away). An all-in-one node's own XMPP ports do the same for paired
  domains. A pair of ports per node is `EdgePort` rows.
- **Ports.** `moqui.edge.EdgePort` (port, pairedNodeId, tunnelKind `ssh`, `xmpp` or `xmpps`), in place of
  `edge_ssh_tunnel_node` and `edge_xmpp_node` (removed). `get#EdgePorts`, `add#EdgePort` and `remove#EdgePort` in
  `EdgeServices.xml` (`moqui_core::edge::ports`); the edge link opens and closes each row's listener within seconds
  (`tunnel.rs` `watch_rows`). The conf's SSH port goes to the one active pairing. A revoked pairing's rows stay and do not listen; a row may not take this node's own ports, set or default, nor the media ranges.
- **Claims** are read from the copies and setups here (`number_holders`, `realm_owner`, `tunnel::xmpp_route`); the
  claims between pairings that Configure and replication refuse are part A's.
- **Tests.** Regression: `moqui-rtc` `turn::tests` (a pairing's credential, its revoke), `moqui-core`
  `edge::sip` and `edge::ports`, `moqui-host` `sip::owners` (numbers and realms of two pairings, a number both claim,
  the same username in two domains), `edge_link::tunnel` and `xmpp_domain`, `rtc::turn_probe` (TCP); served,
  `it edge_domains_comms` (two pairings' XMPP domains on one pair of ports by SNI and by `to`, a port row to its
  pairing). Acceptance: `edge_domains_comms::two_local_nodes_comms_stay_apart` (TURN and XMPP of two pairings, one
  revoked).

### 4.2 Root screens by host name

Moqui Java lets one server answer different host names with different root screens: `<root-screen host="..."
location="..."/>` in the `webapp` conf, `webroot.xml` for every host by default. It serves larger deployments with
separate sites for particular purposes on sub-domains or related domains. mr-pass could be set up this way, with
`forum.foo.org`, `shop.foo.com` and so on, each a thin wrapper screen around one app in place of `webroot.xml`. The
default stays what it is: every app on a path under `webroot.xml`. moqui.org will not use it.

**What is there.** `root-screen` is read and used: the first host pattern other than `.*` that matches the whole
host wins, and `.*` is the fallback (`screen/info.rs` `root_screen_for_host`, read once per conf generation; a conf
reload is keyed by host). `ec.screen.makeRender().rootScreenFromHost()` works. One unit test covers the choice
(`the_host_header_selects_the_root_screen`). mr-pass binds a site to a host another way, `<public-site>`: the host,
the site id, its own root (`screen-location`) and its mounts, with a resolver service and a host check; the Shop
mounts at `/`.

**To verify, and to build where it fails.** Each is a check on a served instance with a second root screen, then a
test:
1. **The shell.** A wrapper root has none of `webroot.xml`'s transitions (`/shellData` and the rest) unless it
   includes them. The shell's providers come from the render's root when it has the apps root, else from the fallback
   root, asked for with an empty host (`screen/shell.rs` `declared_providers`), so a wrapper root shows
   `webroot.xml`'s apps.
2. **Static files.** A root screen serves the directory beside it (`webroot/` beside `webroot.xml`). A wrapper root
   needs the shared `/js`, `/css` and `/libs`, and may have files of its own.
3. **Framework routes.** `/Login`, `/rest`, `/rpc`, `/apps`, `/qapps`, the token routes and the servlets answer on
   every host today. A wrapper host for a public site should be able to refuse them, as a `public-site`'s
   `host-check-service` does.
4. **Error screens.** `error-screen` paths are relative to the root screen, so a wrapper root needs them or falls
   back to `webroot.xml`'s.
5. **Absolute URLs made outside a request.** `webapp_root_url("webroot")` has one host (`webapp_http_host`): mail
   links, SSO and OAuth callbacks, report links. A link made in a request should use the request's host; one made
   outside a request keeps the webapp's.
6. **Sessions and sign-in** (decided Nov 12, 2026). The session cookie stays tied to its one host, with no cookie
   domain. A deployment's own domains federate by SSO. mr-pass's Account does that for PASS sites, and that is enough
   for now. The framework's `sso` feature signs in at an outside provider; a server that is the provider for its own
   other hosts is not built, and is later.
7. **Per-origin features.** CORS origins, the service worker, the PWA manifest and Web Push subscriptions on each
   host.
8. **Tools.** What walks the root screens (`all_root_screen_locations`: the artifact index, screen documentation)
   includes each root.
9. **Behind an edge node.** Each such host is an `EdgeHost` of its pairing, with a certificate for the name.
10. **mr-pass.** A wrapper root per app (Forum, Shop, Account and the rest), and how it relates to `<public-site>`:
    one of the two is the host's root, and the site id stays bound by the operator, never by a request.

**Deliverable.** A working example, `conf/MoquiSitesConf.xml`, a section in Run-Deploy, and mr-pass's Deployment.md
for the per-app hosts.

**Built (E14, 2026-11-13).** `root-screen` has a `framework-routes` attribute (not in Java) for what a host gets beside
its root's own screens: not set as Java (the routes outside the screens answer on every host), `include` (the default
root's screens for a first path name the host's root does not have) or `refuse` (its root's screens and the shared
static files only). The checks, as found and as built:
1. **The shell.** A wrapper root had none of `webroot.xml`'s transitions or subscreens. Rather than copy them into each
   wrapper (`transition-include` by name, and the always-actions the framework's screens need), `include` serves a
   path the wrapper does not have under `webroot.xml` as the request's root, so `/qapps`, `/shellData`, `/Login` and the
   apps are `webroot.xml`'s with their own links. The providers outside an apps root come from the default root
   (`info::default_root`), not a root asked for with an empty host.
2. **Static files.** `/js`, `/css`, `/libs` and `/images` come from the default root when the wrapper's directory has no
   such file, in every mode; the wrapper's own files first.
3. **Framework routes.** `refuse` refuses the token routes and the servlets in `public_site::refuses_framework_route`
   (no service call), the host routes (sockets, `/dav`, `/health`, `/metrics`, the LLM and channel routes) by a
   `route_layer` in `http.rs`, and the HTTPS listener's gRPC services (`grpc_routes` sends the call to the router). Git
   at a public site's page URL still passes on a public site's host, by the `GitPageRequest` mark the rewrite sets; a
   request to `/git/` there does not. A conf with an invalid host pattern refuses (fails closed). `/Login`, `/apps`,
   `/rest` and the rest of `webroot.xml` are not in the wrapper's tree, so they are Not Found already.
4. **Error screens.** The error screen of the host's own root, else the default root's.
5. **Links.** `ec.web.getWebappRootUrl` in a request uses the request's host only when the whole header is a plain
   host name with at most a numeric port, a `root-screen` pattern other than `.*` names it and that root does not
   refuse the framework's routes (`info::link_host`, one normalized value checked and written), with the conf's scheme
   and port; outside a request it is the conf's (the earlier test keeps that meaning for any other host).
6. **Sessions.** There was no cookie domain; the served test checks the cookie stays host-only.
7. **Per origin.** CORS already took the request's own origin; the service worker, manifest and Web Push work per
   origin with `include` (the served test checks `sw.js`'s scope header and the manifest on a wrapper host).
8. **Tools.** `all_root_screen_locations` listed every root already; it now lists each once. The artifact index reads
   every component file, so a wrapper's screens are in it; an absolute link in them still resolves from `webroot.xml`.
9. **Behind an edge node.** Docs only (Run-Deploy).
10. **mr-pass.** No wrapper screen per app: `Pass.xml` is every PASS host's root, named by a `root-screen` with
    `refuse`, and the host's `public-site` binds the site and puts the app at `/`. Routing a site whose root is also the
    host's root needed a fix: a public site's path is now routed once, with no "already under the site's root" guard
    (which skipped the binding), and a request no site route takes has its `resolvedSiteId` parameter dropped, so the
    site is never the request's.

Tests: `screen::tests::hosts` and `webapp_root_url_keeps_a_root_screen_host_of_the_request` (moqui-core),
`root_hosts::a_wrapper_root_serves_its_host_by_its_framework_routes` (moqui-host, served); acceptance
`root_hosts::two_hosts_with_two_root_screens` and mr-pass's `client/SiteHostsHttp` (with `SiteHostsFixtures`).

### 4.3 The front option set

An edge node does two jobs today. This plan names them, so that the first can be deployed with the second or apart
from it (decided Nov 12, 2026).

| Option set | What it is |
|---|---|
| **Front** | What faces browsers and other clients. Links taken from the nodes behind it. Web access by host name, terminate or passthrough, with the client's address passed on. TCP ports sent to a pairing (SSH, XMPP, SMTP). Load balancing over a pool, with sticky sessions and health ([§4.4](#44-pools-load-balancing)). Web protection. TLS and certificates ([§4.8](#48-certificates-and-acme)). The offline page |
| **Edge** | The rest of an edge node: delegate, queue and replicate; the mail edge; chat webhooks and MMS files; the media node and TURN; SIP; mr-pass's sites |

**Roles** (decided Nov 12, 2026). `node_role` names the sets a node has:

| `node_role` | Sets | The node |
|---|---|---|
| `front` | Front | A front node ([§4.7](#47-a-front-node)). Its Hello offers only web access and ports, so the wizard on a node behind it shows only those |
| `front_edge` | Front and edge | An edge node as built, which needs no other node to balance a local cluster. This is what `node_role=edge` means today, so MoquiEdgeConf.xml, the guides and the tests change to `front_edge` in E17; nothing is released, so the old meaning gets no alias |
| `edge` | Edge | An edge node behind a front node. It takes its local nodes' links for the queue, delegate, replicate, comms and PASS, and its sites and ports are reached through the front. It forwards no web access itself |
| `all-in-one` | A full server with both | As built (E12) |
| empty | None | A normal node, a local node among them |

Every node with a role takes links. Which sets it has decides what its Hello offers.

**As built (E17 part A).** `moqui_core::edge::NodeRole` is `Normal`, `Front`, `FrontEdge`, `Edge` and `AllInOne`, with
`has_front` and `has_edge`; `edge_node` (no business data of its own) is true for the three dedicated roles. Today's
`edge` became `front_edge` everywhere: the default conf's comment, `conf/edge/MoquiEdgeConf.xml`, the docs and the
tests; `edge` alone parses as the edge set. The offer and the Hello honour the sets (`offer.rs` `for_role`): `front`
offers web access and the XMPP ports, with mail, chat, PASS, calls and SIP named not available; `edge` offers no web
access or ports, and its hook installs no web front, so a host name it holds gets nothing (E19 part A finished both: a
front starts and serves nothing of the edge set, and an `edge` node binds no tunnel port, [§4.7](#47-a-front-node), As
built). A screen
asks `get#LinkStatus`'s new `takesLinks` rather than the role's names (the Edge Node screens do; Edge Clients is 16C's).

**Web protection is the front's.** It is built: connection, request and body limits, overload shedding, rate limits,
bans, and client, path and user-agent rules (`<web-protection>` in the `webapp` conf), in memory per server. What a
front needs beyond it ([§8](#8-for-the-review-pass)):
- **For the first release, in E17:** rules per domain, so one pairing's site can be stricter than another's; bans
  shared between the members of an edge cluster, and between a front node and what is behind it (Run-Deploy lists
  shared bans as not built); a view in the System app of what is being limited, denied and banned.
- **Later:** the rest of what a front usually does: response compression, security headers per host, a cache for
  static files.

A node behind a front keeps its own web protection, counting by the client's address the link names, as a local node
does behind an edge node today.

**As built (E17 part C, web protection, 2026-11-13).**
- **Rules per host.** `rate-limit`, `client-rule`, `path-rule` and `user-agent-rule` take `host` (names, `*.example.com`
  for any name under one), matched against the request's `Host` without its port; such a rule applies to that host's
  requests on top of the rules without `host`. A connection's checks, before a request names a host, and the bans use
  only the rules without `host`. Bans keep no `host`: the events of every host count, and a host's stricter rate limit
  or rule feeds them. Each rule counts what it refused (`crate::guard` in the host).
- **Shared bans.** A ban started on a server is told on the new cluster topic `web-ban` (taken only from a live member
  of the cluster, by its row, read off the publishing thread) and up each edge link the server dialed (`Signal`,
  which now takes `web-ban` beside `rtc-signal`; the edge node's link stamps the pairing). Up is the useful way: a
  local node sees the failed sign-ins, and its edge node then refuses that client for that pairing's host names only
  (at the forward, 403, counted as `banned by a pairing`), so a pairing cannot shut a client out of another's sites;
  that pairing's lift lifts only those. Nothing goes down a link (decided in the review): an edge node's own bans may
  come from any pairing's visitors, so no pairing is told of another's, and a local node takes no ban from its edge
  node. A ban from elsewhere is never applied to a trusted proxy or an exempt client, lasts at most a week, is kept at
  most 10,000 per pairing, and is not told on (`crate::guard_share`, whose `decide` is the pure rule). Two clocks: a
  shared ban's length is sent in seconds and timed by each server's own clock, while member liveness and the sender
  lock are the database's. No new link message was needed.
- **The view.** System › Security › Web Protection (`System/Security/WebProtection.xml`) over
  `org.moqui.admin.OperationServices.get#WebProtection` (ADMIN_OPERATION_VIEW: the counts, each rule with its hosts and
  count, the bans with what banned them and for whom) and `lift#WebBan` (ADMIN_OPERATION_MANAGE: lifts every ban of an
  address on the server and tells the others). Web protection is not only an edge feature, so it is under Security
  rather than Edge Clients; no System screen showed any of it before (only `/metrics`).
- **Tests.** Host unit tests (`guard`: a host's stricter rules and their counts, shared bans told, applied, scoped and
  lifted; `guard_share`: whose bans are taken, each branch of `decide`, and up only; `edge_link::signal`: a ban crosses
  the link up only, marked); `it edge_pool_work`: a ban on one member holds on the other,
  the view lists it, a lift without ADMIN_OPERATION_MANAGE is refused and an administrator's ends it on both.

### 4.4 Pools: load balancing

**The rule** (decided Nov 12, 2026). When several nodes register for the same domain name, the front pools them and
balances evenly. A node joins a pool only when it is already in a cluster with the pool's members: local nodes join
their cluster first, then link. A node that is not in the cluster is refused.

**Members.** Today a link is matched by its key to one `EdgeClient`, and each new up connection replaces the older
generation. Cluster members share their `.env`, so they share the pairing's key.
- `EdgeClient` stays the pairing, and no member rows are added. A member's certificate is made from the shared key
  and names its instance id, as now. The pairing's one generation counter numbers every member's connections, and
  the live links are kept by pairing and member.
- Every member links, so each has its own route for call signaling. That removes "a call's socket on another member
  is refused" and the lock row for who holds the link.

**The cluster's member list, in memory** (decided Nov 12, 2026).
- Each linked member sends its cluster's `ClusterMember` rows up its link: with its Hello, then with every ping
  (15 s), and at once when the edge node asks. Each row is the instance id, its heartbeat's age and its version.
- The edge node keeps the latest list per pairing in memory only, never in its database. A restart loses it, and the
  first link brings it back.
- It is not one of the replicated sets: those are stored, and a heartbeat would send a change every few seconds.
- **The edge node reports on the cluster.** Clients shows each member: live in the cluster and linked; live but not
  linked, which is a fault to show; linked with a stale heartbeat; on another version. Run-Deploy lists a screen of
  the cluster's members as not built, and this is one.

**Admission.** A link whose key is a pairing's, from an instance id that is not linked:
- With a member linked, the edge node asks it for the list at that moment. The new node is admitted when the list
  has it as a live member. Else it is refused, with "join the cluster first", and tries again with the refusal
  backoff.
- With no member linked there is no list, and the key admits it. This is how a pool starts, how it returns after
  every member was down, and how a standby takes over ([§4.5](#45-failover-to-a-standby)).

A copy of a server on another database (a staging copy with production's `.env`) is then refused while production is
linked, where today it would take the link. A node with another key that asks for a name a pairing holds is refused
as now.

**As built (E17 part A, Nov 13, 2026).**
- **Members.** A member's Hello names its own instance id; the key names the pairing (`EdgeClient`, unchanged, no member
  rows). The registry (`accept.rs`) keeps a `Pool` per pairing: its members' links by instance id, the latest member
  list, and the nodes refused or held lately. The pairing's `linkGeneration` numbers every member's up connections, so a
  key and a generation name one member's down connection. A member that dials again replaces only its own older link.
  The link is registered by the Hello that admits the member, not at the role byte. The Hello answer names the pairing
  (`pairing_id`), which the member's TURN usernames carry. A local node keeps its queue rows and receipts by the pairing
  too: pairing writes `edge_node_pairing` (the instance id of the member that paired) into the `.env` every member
  shares, and `queue::local_node_id` reads it, so every member's outbox is one and a standby finds the receipts.
- **For 17B.** `EdgeServer::members_of(pairing)` gives each member linked with its generation, state and down
  connection's sender; `link_of(pairing)` picks a live member round robin, and `down_sender`, `only_pairing_link`,
  `pass_link`, `delegate` and `ping_local` go through it. Each member's down connection runs its own queue delivery
  (items are claimed by lease); a member that is not live answers its items "unavailable", not counted. Call signaling
  routes stay one per pairing (`signal::route`), the last member's down connection's: 17B's "the member whose socket
  bound the leg" (built in part B, below).
- **The member list.** `moqui_core::edge::pool::cluster_members` reads the `ClusterMember` rows (a new `binaryVersion`
  column, written by each member) with heartbeat age, liveness by the cluster's window and version, this member always
  in it. It goes up in `HelloRequest.status`, every `PingRequest.status` and `LocalLink.Members` (new). The edge node
  checks it (`check_member_list`: at most 64, ids as node ids, each once, short printable versions) and keeps the latest
  per pairing in memory. `get#EdgeMembers` reports each member (`pool::report`): `ok`, `notLinked` (live, not linked: a
  fault), `staleHeartbeat`, `otherVersion` (not the version most of the pool runs), `stopped`, `held`, `refused`.
- **Admission** (`pool::admission`). A member linked reconnects. With others linked, one with its down connection open
  is asked now (3 s); when none answers the latest list it sent with its Ping is used (the plan said refuse; a Ping is at
  most 15 s old, and a member that cannot answer for 3 s would otherwise lock its pool). Not live in it: refused
  "join the cluster first", kept among the refused for Clients; the dialer takes it as a refusal. With none linked the
  key admits.
- **State.** Each member's state rides on Hello and Ping (`MemberStatus.state`): `starting` until its server's startup
  report is logged (pinging every 500 ms meanwhile), `draining` while its own `InstanceDrain` row says so (the recovery
  action's drain, so `/health` answers 503 too), `stopping` in a last Ping as its listeners stop, else `live`. A change on
  the member is told at once (`EdgeLink::state_changed` wakes the Ping). `drain#EdgeMember` drains or resumes: on the
  edge node it asks the member down its link (`LocalLink.Drain`), on a local node it drains itself.


| Traffic | Balanced per | Sticky by |
|---|---|---|
| Web, terminate | Request | A cookie: `moqui.session` by default, the name configurable per host. The member is chosen by rendezvous hashing of the cookie's value over the live members, so there is no table to keep, a restart changes nothing, and every edge cluster member chooses the same one. A request with no cookie goes round robin |
| Web, passthrough | Connection | The client's address (IPv6 by its /64): the front sees no cookie |
| WebSockets | As their opening request | The same |
| PASS authority calls | Call | The session handle while a sign-in is in progress; else the member with the fewest calls in flight |
| Delegated calls | Call | The call's conversation: a phone call's or a call session's id, so one call's events reach one member |
| Queue delivery down | Item | Nothing more: an item's key already goes one at a time, and receipts are rows in the pool's database, so any member applies an item once |
| Call signaling | Message | The member whose socket bound the leg |
| XMPP and SSH tunnels | Connection | The client's address. To one member until checked: XMPP sessions are per member, and each member has its own SSH host key |
| Mail, webhooks, media | Not balanced: they end on the edge node, and their work goes down as queue items or delegated calls | |

**Health.**
- A member is live while its link is. A member that fails or times out several requests in a row is set aside for a
  time, then tried again.
- A member that is starting or stopping says so on its links, so it gets no new work. The operator can drain a
  member, from its own System app or from the edge node's, which asks it down the link. That makes a rolling upgrade
  of the pool possible. The state is the member's, told on each of its links, so nothing is stored on the edge node.
- A request that fails before any byte of an answer is sent again to another member, once, and only when it is a GET
  or HEAD with no body. Nothing else is sent twice.
- With no member live: the offline page, or a refused connection, as today.

**Sessions.** Sessions are rows in the pool's database, so any member can serve any request. Stickiness keeps a
client's requests on one member's caches and makes `flush="guarantee"` unnecessary. A member lost moves only its own
clients.

**As built (E17 part B, balancing, 2026-11-13).**
- **The choice.** `edge_link/balance.rs` chooses among the members `EdgeServer::link_of(pairing, affinity)` gives:
  linked with the down connection open and live. `Affinity::Key` is rendezvous hashing, the highest of SHA-256 over the
  member id, a zero byte and the key (the first eight bytes), so the list's order never matters and a build or an edge
  member gives the same; `Affinity::Address` hashes the client's address, an IPv4-mapped one as IPv4 and IPv6 by its
  /64; `Any` is the pool's round robin; `Fewest` the fewest in flight, in turn among equals. 17A's `members_of` and
  `down_sender` are gone: every path picks with an affinity and gets a `Picked` (pairing, member, generation, sender)
  whose `Flight` counts the work in flight until it drops.
- **By protocol, as the table.** Terminate: the host's cookie (`EdgeHost.stickyCookie`, new; else `<edge
  sticky-cookie>`, `moqui.session`, `SESSION_COOKIE_NAME`), read from every `Cookie` header, else round robin; the
  member is picked as the request is forwarded, and an answer's body counts in flight until it ends; a WebSocket is
  its opening request. Passthrough, SSH and XMPP tunnels and the `EdgePort` ports: `tunnel::open` by the client's
  address. PASS: `pass_link(site, session_handle)`, the handle while it is set, else the fewest in flight, the call in
  flight while it runs. Delegated calls: `balance::conversation`, the first of `callSessionId`, `sipCallId` and
  `callLegId` in the parameters, else the fewest in flight. Ping: any. A phone call's first event (`receive#PhoneCall`,
  `receive#DeviceCall`, by `sipCallId`) and its later ones (by the session) may reach two members; that is fine, as a
  local node keeps a call's state in rows, and the edge node's engines wait for each answer before the call's next
  event. A sticky key over 4 KiB (a client's oversized cookie) is not hashed: the request goes in turn.
- **Health.** Each member's `Health` is kept per pairing across its links (in-flight, requests, failures in a row,
  set-asides, set aside until; at most 128 kept, those neither linked nor busy let go first). A transport failure or a
  time-out counts (an HTTP status, a tunnel refusal, a gRPC refusal other than unavailable, cancelled, unknown or
  deadline exceeded are answers and clear the count); `set-aside-failures` in a row (3) set it aside for
  `set-aside-seconds` (30), logged and counted; a member tried again after its time and failing is set aside at once
  again. With every member set aside all are tried. Starting, draining and stopping members get nothing new (17A's
  state).
- **Once more.** `web::send_once_more`: a request whose member's connection was not ready gets it back unsent
  (`try_send_request`) and goes to another member whatever its method; one sent that failed or timed out before its
  answer's head goes to another member only when it is a GET or HEAD with no body and not a WebSocket, made anew from a
  copy of its head. One more try at most; then 502, 504 or the offline page as before.
- **Call signaling** (`signal.rs`). Each member's down connection adds its own route, `<member>@<pairing>` (`@` is in
  no node id), sending with the member's own id as the target; its guard removes only that member's route. A member's
  frame must name the member as its origin (17A refused every member but the one with the pairing's id), and the edge
  node takes the origin as the member's route, so its media driver's replies for a leg go to the member whose socket
  bound it.
- **Sessions.** A new session's cookie can hash to another member than the one that made it, which then had no row:
  eventual flush wrote sessions on the next scheduler pass. Now a pool member (`edge_node_url` set, read at most every
  10 s, and other live members in its cluster by the heartbeat's in-memory list: `user::session::pooled`) writes a
  session whose row is not written yet (new, or a new id after sign-in) before its response, at the end of the
  request or, for a streamed response, in its head hook before the cookie goes; a failure there is logged and the
  scheduled flush retries, the page is not failed. A written session's later changes stay on its member and are
  flushed as before. A session that comes back to a member after more than 5 s unused there (moved by a set-aside, a
  drain and resume, a member joining) is read again from its row, after what that copy had not written goes first. The
  limit: what a member had not flushed when the session moved away (up to one scheduler tick of changes) is not seen
  by the next member, and is written over it if the first member comes back to it later. `flush="guarantee"` stays
  available for those who want none of that.
- **Seen.** `get#EdgeBalancing` (`EdgeLink::balancing`): the defaults and each member's linked, state, takesWork,
  inFlight, requests, failuresInRow, setAsides and setAsideUntil, for the Balancing part (17D). Counters
  `moqui.edgelink.requests` (kind terminate, a tunnel kind, delegate, pass; counted for each member a piece of work is
  sent to, as it is sent), `retries` and `set_asides` carry
  `moqui.edgelink.pairing` and `moqui.edgelink.member`.
- **Tests.** Host unit tests: `balance` (a cookie on one member in any list order, a member gone moving only its
  cookies, an even spread, turns, a /64, the fewest in flight, set aside and tried again), `web` (a GET that fails
  before an answer goes once more, a POST never, an unsent POST to another member; the cookie from split headers),
  `signal` (the route per member, the origin, a gone member's replies refused, the other's kept), a key over 4 KiB in
  turn; core: `edge::conf` (the defaults), `edge::hosts` (the cookie kept, cleared, refused), `user::session` (a new
  session written first, a session read again on its return).
  `it edge_balance` (regression, about 2 s): two members of one pairing on one local app, by turn, sticky, drained
  with a WebSocket going on, back, a host's own cookie, a member gone, the offline page. Acceptance (run once each):
  `edge_balance::two_members_share_requests_and_a_session_moves_when_its_member_stops` and
  `edge_balance::a_drained_member_finishes_its_requests_and_gets_no_new_ones`, two real cluster members, the second in
  a child process whose own hook dials.

**Work from the pool to the edge node.**
- **The outbox.** Items are rows claimed by lease, so every linked member sends. Order per key holds: only the head
  of a key is claimed.
- **Replication.** Change notes are in memory in the member that wrote, and the cursor is one sequence per set. So
  one member sends, chosen by a lock row as scheduled jobs are, and the others pass their notes to it on a cluster
  topic. A new sender starts each set with a full send, as a restart does today. The sets are small.
- **The link's own rows** (`CallMediaNode`, `SipNode`) and the minute's `refresh#PhoneFallbacks` are written by the
  replication sender only.

**As built (E17 part C, work from the pool, 2026-11-13).**
- **The outbox.** A local node's queue rows, its outbox, its receipts and the content sent to it, are keyed by one id
  for the whole cluster (17A's `edge_node_pairing`, read by `queue::local_node_id`) where they were keyed by the member's instance id, so any
  linked member sends any item and an item is applied once whichever member it reaches (this also gives "Queue
  delivery down" its receipts). Only the head of a key is claimed, with a lease, as before, but two things were wrong
  for several members. The claim and the answer's write were an entity `update_all` with conditions, which reads the
  rows and then updates each by its key, so two members on Postgres both claimed one item (the race test found it);
  each now reads its row `FOR UPDATE` and checks it before writing, in one transaction. And a delivery starting on a new
  link let go of every lease on the node's items, another member's live one too; now only those its own earlier links
  left (`release_leases` by the owner's prefix). The queue's failing passes (`fail_pending`, the retention pass's
  `expire`) read each row `FOR UPDATE` too, so they never turn a delivered item failed; `ChatBridgeDelivery`'s claim,
  which the queue's was modeled on, had the same race and is fixed the same way.
- **Replication.** One member sends: the holder of the lock row `moqui.edge.EdgeSenderLock` `replicate`
  (`edge::replica::sender`), claimed by each member's link every 3 seconds while another holds it, renewed every 10, a
  30-second lease by the database's clock, taken over when it lapses, let go when the server stops or unpairs. A link
  that drops keeps the lock until its lease lapses, so a quick return goes on incrementally. Fencing (from the review):
  a new link sends nothing before its first claim; a holder whose claim does not come back within two thirds of the
  lease stops sending by its own clock; and each batch carries when its sender took the lock (the row's
  `claimedDate`, `ReplicateRequest.sender_token`), which the edge node keeps on the set's cursor and refuses an
  earlier one of, so a member that lost the lock cannot cross the new sender's batches. Every member sends each note
  it makes to the others on the new cluster topic `edge-replica-note` (gathered for 100 ms); the sender keeps its own
  and takes the others', a member that is not the sender keeps none. A new sender sends each set whole under a new
  epoch; its full send reads every record after it took the lock, and notes after that follow it, so nothing falls
  between. A member that is not the sender runs its `Sender` idle.
- **The link's own rows.** `keep_media_node` (the `CallMediaNode` and `SipNode` rows and the minute's
  `refresh#PhoneFallbacks`) runs only while its member is the sender, and writes at once when it becomes one.
- **Tests.** `edge::replica::sender` (one holder, a release and a lapsed lease taken over, a later sender's greater
  token; notes kept only by the sender, the others' taken, a new sender's full send), `edge::replica` (the edge node
  refuses an earlier sender's batch), the stall case in `it edge_pool_work` (the lock row held: the sender stops
  sending before its lease, and sends whole once a claim comes back), `edge::queue` (two members on stale heads, each item once and
  in order; a member releases only its own leases), `it edge_pool_work` (the race of two members of one Postgres database on
  one outbox; with another member holding the lock nothing is sent, a free lock is taken with a full send, the other
  member's change reaches the sender and goes up, a takeover stops the sending and taking it back sends whole under a
  new epoch). The link rows' gating has no test of its own: the in-process edge of the it tests has no media.
- **With 17A's links.** Every member now links with its own generation (part A), so each runs `keep_sender_lock` on
  its link, and the edge node's deliveries name the member in their lease owner (`edge/node/member/generation`), so a
  new link of one member lets go only of that member's leases there too.

**Not a general balancer.** A cluster behind a load balancer of your own still works as before. A front balances
only nodes that are paired with it.

### 4.5 Failover to a standby

Something simple (decided Nov 12, 2026). A local node registers with its edge node. When the local machine dies, a
hot standby is started and takes its place. Nothing starts the standby by itself: a person does, or automation the
operator adds, such as another moqui instance or any tool that runs a monitor. That automation is not part of this
plan.

**The standby** is a second machine with the same runtime and `.env` (the pairing's key, the edge node's address and
pin, the crypt pass), and the data: the same external database, a replica that is promoted, or the last backup
restored. It keeps an instance id of its own.

**The edge node's part.**
- **While no node is linked** it holds what it holds today: the queue for its retention period, mail taken for the
  replicated recipients, the voicemail fallback, the offline page.
- **Taking over.** The standby links with the pairing's key. No node of the pairing is linked, so it is admitted
  ([§4.4](#44-pools-load-balancing), Admission) and gets every name of the pairing. Replication starts with full
  sends, and the queue delivers.
- **The first machine coming back** while the standby is linked is refused: it is not in the standby's cluster.
  Clients shows it, so the two never trade the link back and forth with two copies of the data. Two machines on one
  database that both run are a cluster, and are pooled instead.
- **A notice** (decided Nov 12, 2026). A node with a new instance id that takes over a pairing is admitted, logged
  and shown on Clients, and raises a notification to the edge node's administrators. So a standby needs no step on
  the edge node. A pairing may be set to hold a new node for approval there instead.
- **After a restore from a backup.** The standby's database lacks what was delivered since the backup. The edge node
  keeps delivered items for the retention period, so Clients gets "Deliver again since" a time, for the items of a
  pairing. Receipts make an item already applied do nothing.

**As built (E17 part A).** `pool::admission` treats a node admitted with none linked as taking over when it is
neither `EdgeClient.lastMemberId` (written by each Hello; the node that paired before any) nor in the latest member list
kept in memory. A takeover is logged, noted on the row (`takeoverDate`, `takeoverFromId`, `takeoverToId`) and sent as a
notification, topic `EdgeMemberTakeover` (EdgeSetupData.xml), to the `ADMIN` group. `holdNewMember=Y`
(`hold#NewMembers`) refuses such a node "held for approval", with one notice, until `approve#EdgeMember` names it
(`approvedMemberId`, used once). The first machine coming back while the standby is linked is refused as any node not
in the linked member's cluster. `deliver#ItemsAgain` (pairing, `sinceDate`) sets the pairing's items delivered since
back to pending (`queue::deliver_again_since`); the receipts, by the pairing, make an applied item do nothing. After an
edge node restart the kept list is gone, so a pool member that is not the last linked one, coming first, counts as a
takeover (a notice, or a hold).

**Review fixes (E17 part A, Nov 14, 2026).** A member id is bound to a key of the member's own
(`runtime/db/edge_member_key`, `member_key.rs`): its Hello signs the TLS exporter value, and the edge node pins the key's
fingerprint per id (`EdgeClient.memberKeys`), so a pinned member is "returning" after an edge node restart too, with no
false takeover, and a known id with another key is a new node. A reconnect is refused while the old link answers a Ping.
The down connection names a random token from the Hello answer and is refused while one is open. The node a takeover
displaced is refused until `clear#EdgeTakeover`; `register` refuses (retryable) when the pairing's links changed since
the admission was decided. The fallback list is used only while at most 30 s old, and the linked members are asked at
once within 3 s. A member not live has its queue delivery paused, not answered "unavailable". Notices are capped at four
a pairing an hour. A local node keys its queue by the pairing its Hello answer names, and writes `edge_node_pairing`
again when it says another. Configure refuses the items of a set the role has not, and an `edge` node passes no
passthrough host.

**Second review (Nov 14, 2026).** A member's key is claimed for its id in one committed transaction before its link is
registered (`pool::claim_key`, compare and set; refused on another key unless a takeover was decided); past the cap of
64 the eviction spares the linked, the last and the kept list's members, forgotten markers going first; a linked id with
no key pinned is not replaced within 5 s. The last member with a new key takes over from itself: no `takeoverFromId`
is written, so it is not then refused as taken over, and the notice says it came back with a new key.
`forget#EdgeMember` reads for update and closes the member's link after commit. The key file is written to a part file
and hard-linked into place.

**The edge node can help a monitor.** It knows when a pairing's link dropped and for how long. The link-down alert
of monitoring over the link (Later) is what an operator's automation would watch.

### 4.6 A cluster of edge nodes

Several edge nodes share one external database and take links. This is the layout for a higher-volume shop or other
public site on mr-pass: the edge cluster's members serve the sites, and the local cluster is their authority.

**Membership.** The edge nodes are a cluster as any other ([Run-Deploy](../docs/Run-Deploy.md#cluster)): one
database, the same `.env`, a distinct instance id each. `edge_link_key` is in the shared `.env`, so every member has
the same link key and a local node's one pin fits them all. They need the Enterprise package, or Maximal with an
external database.

**A local node connects to each member** (decided Nov 12, 2026).
- The Hello answer lists the edge cluster's live members, each with its own link address (a new property per member,
  `edge_link_url`, as `cluster_base_url` is per member).
- The local node dials every one, up and down, and takes requests from each. It keeps the list, so a restart finds a
  member when the one it paired with is gone, and reads it again with each Hello.
- `edge_node_id` becomes the set of member ids, each learned on a link whose key is pinned.
- A local cluster of L members and an edge cluster of E members is L × E links. Each edge member gets the local
  cluster's member list from its own links.

**Shared already**, as rows: pairings, host names, queue items (claimed by lease), receipts, replica values and
cursors, MMS files, MX delivery rows (leased), PASS visitor sessions.

**In memory today**, and to make right in a cluster:
- the host table and the recipient maps, built again on a member when another member applies a change: a cluster
  notice in place of the poll;
- the SIP registrar's nonce counts and the voicemail fallback's forward counts, per member (as built the nonce's MAC
  key is per member too, see As built (E18 part B));
- the webhook replay cache: the queue's idempotency key already makes a repeat one item;
- pairing tries and web protection's counts, per member, as in any cluster; a ban is shared since E17 (§4.3, As built).

**By area.**
- **Web, mail in, webhooks, PASS pages:** any member. Sticky choices are the same on every member, since the hash is
  over the same list.
- **Mail out:** any member claims a delivery row.
- **Calls:** each member is a media node with its own public address. The local node keeps a `CallMediaNode` row for
  each from its link, and a call's legs are all on one member. Cascading between edge members is Later.
- **SIP:** each member runs an engine. One member registers a trunk at a time, by a lock row; a phone's binding names
  the member it registered with, as it names a node today.
- **Tunnels:** any member.

**Reaching the cluster.** Without a front node: an address record per member, an MX and SRV record per member. With
one: [§4.7](#47-a-front-node).

**A clustered all-in-one server** is the same: each member takes links.

**As built (E18 part A, a local node's link to each member, 2026-11-14).**
- **The list.** `edge_link_url` (a default property, per member as `cluster_base_url` is) is written by each member's
  heartbeat into its `ClusterMember` row (`edgeLinkUrl`, new). Every edge member's `HelloReply` and `PingReply` list the
  cluster's live members with it (`edge_members`, new `EdgeMember` message), in instance id order (`members::of_cluster`),
  the answering member only while its own row is live, so every member answers the same list; read at most every 2 s; empty when the rows cannot be read, which a local node takes as not told. The plan said Hello
  only; the Ping answer carries it too, so a member that joins or leaves is seen within a Ping on links that never
  drop. An address that is not `https` with a host goes as none.
- **The local node** (`dial.rs`). A `Dialer` is now a set of links, one per edge member, each with its own generation,
  status and down connection, and each taking the edge member's calls. `members::plan` decides them: a member with an
  address gets a link there; the URL the node paired with is dialed while no link is up and every link failed twice
  in a row, or when there is no list yet, and that link becomes its member's once its Hello names one (dialed at the
  member's own address from then); a member gone from the list is not dialed again (a link still up goes on until it
  ends). A member with no address is reached only at the paired URL, so a single edge node needs no `edge_link_url`
  and an edge cluster sets it on every member.
- **The list is kept** in `runtime/db/edge_members` (JSON: the paired URL and pin, the members, the ids learned), not in
  `runtime/.env`: each member of a local cluster learns it from its own links, the `.env` is what people copy between
  members, and a per-machine file is how `instance_id` and `edge_member_key` are kept. A file for another pairing is
  not used; pairing and unpairing remove it. A restart dials the kept members, so it links when the member it paired
  with is gone.
- **Identity.** All edge members present the one pinned link key (`edge_link_key` in the cluster's `.env`; the pin is
  the key's SHA-256 and the handshake's signature proves it). A link dialed for a listed member must reach that member,
  else the attempt fails ("its edge_link_url reaches another member") and the next list corrects the address, which
  wakes the link with its backoff reset; on the paired URL any id is taken that the local node has no use for: never its
  own, a member of its cluster (any `ClusterMember` row, live or not) or a media node of its own (a `CallMediaNode` row
  not `edgeLinked`), since signaling for an id with a link route goes over that link. `edge_node_id` becomes the set of ids learned on pinned links: `LocalConfig::on_pin` is told each
  new one, and the hook keeps the property the comma-separated set (the `.env` keeps the one pairing wrote), which
  `guard_share` already reads. The old "the edge node's instance id changed, so pair again" refusal is gone
  (`check_edge_member` replaces `check_edge_id`): every member is another id with the same key.
- **Work up once.** One link carries the node's work at a time (`Cluster::elect`, one step under the links' lock: the
  one that does while it is linked, else the first linked by member id): its outbox, its replicated sets and the sender lock, the edge node's
  `CallMediaNode` and `SipNode` rows and the TURN peer; it moves when that link ends. Call signaling has a route per
  link (per edge member). A web protection ban goes to every edge member (the ids `edge_node_id` names), as each edge
  member keeps its own bans in memory and does not tell a ban from a link on. Work down (requests, delegate, queue
  delivery, PASS) comes from each member, claimed by lease in the edge cluster's database, so each item once.
- **On the edge side nothing else changed.** Linked members are per edge member (`Pool`), admission finds the local
  member's key pinned in the shared `EdgeClient.memberKeys` (so a member linking to a second edge member is
  "returning", no false takeover), and the generation counter is one row `FOR UPDATE`. A clustered all-in-one server
  takes links on every member the same way; nothing in the hook or the acceptor assumes one member.
- **For 18C.** `get#LinkStatus` has `edgeMemberList` on a local node (each link: instanceId, url, state, generation,
  rttMs, lastConnectDate, lastError, reconnects, carriesWork, retired; `LinkStatus.members`, `EdgeMemberLink`), and
  `get#EdgeCluster` on a node that takes links lists its cluster's members (instanceId, linkUrl, live, heartbeatAgeMs,
  version, thisMember; `EdgeLink::edge_cluster`).
- **Left to 18B.** The edge members' in-memory state (the host table and recipient maps on a change, SIP trunk
  registration, phone bindings, the webhook replay cache), and a `CallMediaNode` per edge member: the local node keeps
  the work link's edge member as its one media node and TURN peer for now (`set_peer_turn`, `allow_row` are one slot).
  Built in part B, below.
- **Tests.** Host unit tests: `members` (the list's checks, the plan for a first link, adoption, a member gone, a
  member with no address, the paired URL only when every link fails, the file per pairing; the edge side's list),
  `check_edge_member`. `it edge_cluster` (regression, about 4 s each): two edge members on one database and a local
  node; the edge cluster's report, a misdirected address refused and corrected, a link to each member with both ids
  learned and the list kept, one link carrying the work, a request through either member, items down and up and the
  replicated set each once, a dialer with another pin linking to none; a restart with the paired member stopped links
  to the other from the kept list and stops dialing the one gone. Acceptance (run once each):
  `edge_cluster::acceptance_one_edge_member_stopped_leaves_web_and_the_queue_to_the_other` and
  `acceptance_a_clustered_all_in_one_server_takes_links_on_every_member` (the same with `node_role=all-in-one`). Mail
  in through the other member needs 18B's recipient maps and is not written yet (written in 18B, below).

**As built (E18 part B, the in-memory state and calls and SIP on several edge members, 2026-11-14).**
- **A cluster notice** (`moqui_core::edge::cluster_state`, the new topic `edge-state`, `{"from", "hosts", "sets",
  "links"}`). There was no poll to replace: each member loaded its host table only after its own changes, and built
  its recipients and trunks when its own process applied a batch, so another member's change waited for a restart.
  Now the member that changes the host names or pairings (`Hosts::changed`: add and remove of a host, pairing,
  Configure, revoke, unpair) or applies a batch (`replica::apply_batch`, the conflicts' recheck) tells the others after
  the commit. One that hears it from another member loads its host table (on a thread of its own; a burst loads at
  most twice) and counts each set as applied (`replica::note_applied`), so its mail recipients (`smtp_queue.rs`) and
  SIP trunks (`sip::watch_trunks`) are built again at their next use. A member whose cluster transport may have missed
  messages (`cluster::recover_missed`) does both for everything (`note_all_applied` moves every set's count).
- **Revocation and the links' state** (from the 18A review). A revoke, an Unpair, a pairing made again and a member's
  forgotten key go on the same notice (`LinkOp`), so every member closes and refuses the links it holds, as the one
  that took the change did.
- **Admission across the edge cluster** (from the 18A review). "Linked" meant linked to the edge member being dialed,
  so a node holding only the key, dialing an edge member with no link of the pairing (one just restarted), took the
  pairing over while a real member was linked to another edge member. Each edge member now keeps one row
  (`moqui.edge.EdgeMemberLinks`: the local members it holds or is admitting links of, by pairing), written inside
  `claim_key`'s transaction that locks the pairing's row and then by one serial writer every 5 s while another member of
  its cluster is live (by the heartbeat, else by the member rows), removed at stop, pruned after a day. Admission reads
  the rows each time and counts a member linked when another edge member's row naming it is under 20 s old
  (`linked_elsewhere`): one linked elsewhere is admitted as another of its links only with its pinned key (another key
  or a forgotten pin is refused while it is linked), and a new node needs a linked member's list as before. While a
  live edge member has no fresh row, no takeover is decided (retryable); two edge members admitting at once are ordered
  by the pairing's row lock, the second re-reading the first's row, so a node dialing both is one takeover. Rows over
  asking the other edge members: the cluster topics carry no answers, and one row per edge member is one write per
  beat. A row read that fails refuses the link (it dials again).
- **Missed notices.** The same beat reads the held pairings' rows and closes the links of a pairing no longer active
  and of a member whose pin is forgotten or another, and a member whose cluster transport reconnected runs it at once,
  so a revoke or forgotten key whose notice a member missed still closes its links within seconds.
- **SIP.** Every member runs an engine and loads every trunk; only the holder of `EdgeSenderLock` `sipTrunks`
  (`moqui_core::edge::sip::claim_trunk_lock`, the sender lock's claim and timing) registers them (`sip/trunk_lock.rs`,
  on any node that takes links), the others ping them; a change reloads the trunks, a holder whose claims fail stops
  after two thirds of its lease, and a stopping engine lets it go. A phone's binding already named the engine's own
  instance id (`SipRegistration.instanceId`), so with a `SipNode` row per member a call to the phone goes through the
  member it registered with. **Not as the plan said:** the registrar's nonce MAC key is random per process, not
  shared. Kept so: sharing it would let an answer captured at one member be replayed at another, whose nonce counts
  never saw it (digest does not cover the Contact); a phone that reaches another member is challenged again. A phone
  rings only for a call whose media is on its member, until cascading between edge members (Later), so SIP.md tells
  operators to point phones at one edge member for now.
- **Calls.** Each link now keeps its own edge member's `CallMediaNode` and `SipNode` rows and TURN secret
  (`dial.rs` `MediaRows`, beside its signal route), still written only by the local cluster's replication sender, so
  each edge member has one row; `calls::allow_row` and `media::set_peer_turn` keep a set (at most 64) cleared at
  unpairing, and a member gone from the edge cluster's list is pruned from both. The phone numbers' fallbacks stay on
  the work link (`keep_fallbacks`). Every member's TURN derives the same pairing secret (shared `turn_auth_secret` and
  pairing row).
- **Webhooks.** The replay cache stays per member; a repeat to another member is one queue item by the event's
  idempotency key (`queue::keyed_id`, an insert-if-missing), checked. Pairing tries and web protection's counts stay
  per member.
- **Tests.** Unit: `moqui-core` `edge::cluster_state` (another member's notice, recovery, a burst, link operations),
  `edge::sip` (one member holds the trunk lock, another takes over once it lapses), `edge::media` (a TURN secret per
  member, forgotten when the member leaves the list), `edge::calls` (the row gate per member), `cluster_state` (only
  another member's fresh row counts, stop and prune); `moqui-host` `sip::trunk_lock` (a trunk registers only on the
  holder; the two-thirds fence). `it edge_cluster_state` (regression): a host removed on one member is not answered by
  the other and one added there is, at once, and a repeated item is one; a key holder, and one naming a linked member's
  id with another key, refused on one edge member while that member is linked on another, a takeover once none is; a
  member linked to both edge members with no takeover, a batch counted on both, a revoke on one closing both links; a
  revoke with no notice closing the links at the next beat; a new mail recipient taken by both members' LMTP
  listeners. Acceptance (run once):
  `edge_cluster_state::mail::acceptance_mail_in_goes_on_through_the_other_edge_member`. The per-member call rows have
  no served test: the media driver and the SIP engine are process globals, so two edge members with media cannot run
  in one test process; the engine's lock and flag are unit tested and `edge_sip` and `edge_call` run the single case.

### 4.7 A front node

A node with the front option set and nothing else (`node_role=front`): no mail, media, SIP, queue or mr-pass. Its
database holds only its pairings, host names and certificates. It is deployed two ways.

**Before a local cluster: simple load balancing.** The cluster's members pair with the front node and dial it. They
register their host names, are pooled and balanced, sticky by the session cookie, behind the front's web protection.
None of the edge node's comms is there: the cluster does its own mail and the rest, or has none.

**Before an edge cluster: the full stack.** The edge cluster's members are the front's pool.
- Each edge member pairs with the front and dials it. So a node with the edge set must be able to dial as well as
  take links. Today a node with a role never dials.
- **The edge members' role.** `edge` when the front does all the fronting, or `front_edge` when the members also
  forward their local nodes' own host names (the apps, as `erp.example.com`). With `edge` members those names are
  the front's too, so a local node pairs with the front for them as well as with the edge cluster: a local node with
  two pairings, where today it has one `edge_node_url`. E19 builds `front_edge` members behind the front first, and
  the second pairing after ([§8](#8-for-the-review-pass)).
- **Web.** Terminate mode for the sites, so the front sees the cookie. Passthrough also works, sticky by address.
- **The local nodes' links.** Each edge member has a name of its own that is a passthrough host on the front, not
  pooled. A passthrough tunnel carries the TLS untouched, so the link's ALPN and client certificate reach the member.
  The front must pick a passthrough host by its server name before it looks at the link's ALPN.
- **Mail.** Port 25 as a TCP port on the front, passed to the pool with a PROXY header, as the SSH port is.
- **Not through the front:** media, TURN and SIP. They go to each edge member's own address, as media never goes
  over the link.

**Two front nodes** are two addresses for the names; each node behind them pairs with both. No more than that is
planned.

A load balancer of your own in place of a front node stays supported: before an edge cluster it must pass TLS
through with a PROXY header, since the link needs ALPN and client certificates end to end.

**As built (E19 part A, the front node and the nodes that dial it, 2026-11-14).**
- **The front node.** `node_role=front` starts none of the edge set: call media and TURN, SIP, the SMTP listener and MX
  delivery are not started even with their ports set (logged, and off in the startup report with "a front node
  (node_role front) has only the front set", `moqui_core::edge::front_refuses`), an XMPP server never is on a dedicated
  edge node, and it keeps no queue (no `queue::maintain`, no delivery down a link). On its links it refuses the edge
  set's calls: `Deliver`, `PutContent` and `Replicate` answer "this is a front node", `Signal` takes a web protection
  ban only, a delegated call and a PASS call fail at once (`EdgeServer` with `EdgeConfig::edge` false). Its Hello names
  its role (`Hello.node_role`, new, filled by every node that takes links) and, as E17 built, offers web access and ports
  only, refusing a Configure's mail, chat, PASS and SIP domain by name. A front dials no other node. Its database holds
  only what a node that takes links writes: pairings, host names, ports, certificates. An `edge` node binds no tunnel
  port either (its front takes them; 17A's review).
- **The front pairing.** A node's link to a front is a second pairing beside its edge pairing: `front_node_url`,
  `front_node_key`, `front_node_pin` and `front_node_id` (no pairing id: the queue's rows are the edge pairing's), its
  list kept in `runtime/db/front_members`, its own `Dialer` (`LocalConfig::front`). Pairing decides where it goes by the
  role the node's Hello names, so the System app's one Pair flow pairs either: a `front` is kept as the front pairing,
  anything else as the edge pairing. A link whose node names another role than its pairing expects is refused, pair
  again. Its links carry web access and ports only: no outbox, replication (not even enabled), call or SIP rows or TURN
  secret up, and the down connection takes no queued item, delegated call or PASS call; call signaling's route stays,
  for a web protection ban up (`guard_share` reads `front_node_id` as well as `edge_node_id`). A local node pooled by a
  front writes a new session before its response, as one pooled by an edge node (`session::pooled` reads both URLs).
- **What goes on which pairing.** With both, the wizard's host names and XMPP domain go to the front and its mail
  domains, chat bridges and PASS sites to the edge pairing (`edge_link::front::split_setup`; each pairing hears every
  Configure, as each keeps its part whole, so a cleared XMPP domain reaches the front), and the offer shown is the edge
  pairing's with the front's web access, ports and addresses (`merge_offer`). Both parts are sent at once; a part one
  pairing did not take is answered item by item with why (`failed_results`) and the other's answers stand, and an offer
  that one pairing did not give shows the other's. A Configure that sets the host names clears them on the edge pairing
  (decided in the review): a local node behind `front_edge` members that also pairs with the front gives its names to
  the front only, where the edge members would otherwise claim them a second time. A Configure to a front carries no SIP domain. With only a
  front pairing everything goes to it, and the front refuses what it has not. The queue, delegated calls, replication,
  comms and PASS are the edge pairing's alone. `get#LinkStatus` shows the front pairing (`frontState`, `frontUrl`,
  `frontName`, `frontFingerprint`, `frontLastError`, `frontLastConnectDate`) beside the edge one, and the peer's role
  (`edgeRole`); a node paired with a front alone shows it as its only link. `unpair#EdgeNode` with `front` unpairs the
  front pairing. System › Edge Node shows a Front node box with Unpair front, and on an edge node a Pair with a front
  node link (minimal; 19C or later polishes the screens).
- **An edge node that dials.** A node with the edge set (`front_edge`, `edge`, `all-in-one`) dials its front from
  `front_node_*` as well as taking links, and pairs with a front only (`PairConfig::only_front`: the role is read from
  the Probe answer, before the code is spent). Its down connection serves its own app with its web front
  (`down::install` with the edge server), so a front's request for a local node's terminate host goes on down that
  node's link with the client the front named, and a passthrough tunnel whose ClientHello names a passthrough host of a
  node linked here goes on down that node's link (`tunnel::relay_passthrough`, a new PROXY header with the same
  client); anything else is the node's own. It gives the front its names itself, by Configure on each new front link
  and on a change (`hosts_changed`, else every 5 s): its sites' hosts and its chat address's host as terminate hosts, so
  the front sees the cookie and balances them sticky by it, and with the front set every active pairing's names in their
  own modes (`front::names_for_front`); a name the front refused is given again after a minute. Its own
  `webapp_http_host` is not given: its System app and its link address stay its own (19B makes the edge members' link
  names passthrough hosts on the front); an all-in-one node's is, as a terminate host, as it is a full server's app.
  Every local node behind `front_edge` members is part of their one pairing on the front, so the front's bans, rules
  and limits per pairing hold for the edge cluster as a whole. The Pair screens show the node's role from
  `check#EdgeNode` (`edgeRole`) before pairing. A passthrough tunnel relayed on needs no certificate of the edge node's. The members of an edge cluster
  share a database and a `.env`, so they are one pairing on the front, admitted by the member list and balanced as any
  pool. `front_edge` members forward their local nodes' names (§8 item 3, built first); with `edge` members a local
  node pairs with the front for its names (the second pairing above).
- **Not through the front:** media, TURN and SIP go to each edge member's own address. Nothing tunnels them: the front
  starts none of them, takes no call signaling, and a front pairing carries no call rows.
- **The local nodes' links through the front** (each edge member's own name as a passthrough host, picked by SNI before
  the link ALPN), TCP ports on the front (SMTP 25 with a PROXY header) are 19B (below); `conf/front` and
  `conf/full-stack` and the guides are 19C ([§4.9](#49-example-confs-for-each-topology), As built).
- **Two front nodes** (19C's reading): with one `front_node_url` per node, two front nodes are a cluster of front nodes
  on one database and one `.env`, each with its own `edge_link_url`, and a node behind them links to every member as
  with an edge cluster: the front pairing's dialer keeps the member list (`runtime/db/front_members`) as the edge
  pairing's does. Run-Deploy says so; no test runs two front nodes yet.
- **Tests.** Host unit tests: `front` (the names an edge node gives its front, the split, the merged offer and
  answers, a part not taken), `signal` (a front takes a ban and no call signaling), `hook` (where a pairing goes by the
  other node's role, the properties each pairing writes), `dial` (a pairing links only to a node of its kind both ways,
  a front pairing's down connection takes no queued item or delegated call). `it edge_front` (regression, about 2 to 3 s each): a
  front starts none of the edge services, its Hello names its role, an edge pairing's link to it is refused, a queued
  item, a replication batch, a queued item's content, a delegated call and a PASS call are refused, its offer is web and
  ports, a Configure's mail, chat and PASS are refused; an edge node
  (`front_edge`) pairs with a front only, dials its front from its properties, gives it its local node's names in their
  modes, and a visitor reaches the local node through the front in terminate and passthrough mode; a local node with
  two pairings (an `edge` node and the front) sends its web name to the front and its mail domain to the edge node, is
  reached through the front, its queue goes both ways over the edge pairing, and unpairing the front leaves the edge
  pairing. Acceptance (run once):
  `edge_front::acceptance_a_front_node_balances_a_local_cluster_with_web_protection` (two members of a local cluster on
  one database linked to a front over the front pairing share a host's requests evenly, the front's web protection
  refuses a client before any member sees it, and a signed-in browser stays on one member and moves when it stops, still
  signed in). The full-stack scenario (an mr-pass site through a front over two edge members, with a local cluster as
  the authority) needed 19B's link names through the front: 19B's acceptance test, with a plain screen site in place of
  mr-pass (part B below).

**As built (E19 part B, ports on a front and the edge members' link names, 2026-11-14).**
- **SMTP on a front.** `EdgePort.tunnelKind` takes `smtp` (`moqui_core::edge::ports::SMTP`), on a front node only
  (`node_role=front`; a node with the edge set refuses one, as it takes mail on its own listener, and a row that is
  there anyway does not listen). On a front the usual ports of the edge set's listeners, which it never starts
  (`smtp_listen_port`, the MX relay, XMPP, SIP, TURN, media and their ranges), are free for a row, so its port 25 can
  be one; its web ports stay reserved. The row binds on `edge_smtp_tunnel_host` (every address when empty), takes at
  most 128 connections at once, and sends each to a live member of the pairing chosen by the client's address (17B's
  tunnels rule) as the tunnel kind `smtp`, a PROXY v2 header naming the client first. The member's down connection
  gives it to its own SMTP listener in process (`smtp::serve_tunneled`), which reads the header because the connection
  came over its own front link and checks the client against `smtp_allowed_addresses` and the per-address limits as a
  direct client's (a LOCAL header keeps the client the front named, checked the same way); the listener's own port reads
  no PROXY header unless `proxy-protocol` is on, and only a node with the edge set takes an SMTP tunnel, on its front
  link. The plan said "as the
  SSH port is": an SSH tunnel names its client in the tunnel request's header instead; SMTP uses the PROXY header the
  listener already reads, so the same session code checks it. The Ports tab offers SMTP on a front only.
- **The edge members' link names.** A node that takes links names its own `edge_link_url` in the Hello of its front
  link (`Hello.link_url`, new, from `LocalConfig::link_url`; an edge member's own code path, not 19A's
  `keep_front_names`). The front keeps the host as an `EdgeHost` row in the new mode `EdgeHostLinkName` with the
  member's id (`EdgeHost.memberId`, new; `moqui_core::edge::hosts::claim_link_name`, `edge_link/link_names.rs`): the
  first claim holds against other pairings, the front's own names (its own `edge_link_url` among them) are refused, a
  member back with another id takes its name over (two members sending one name are logged each time it moves) and a
  member's new name replaces its old one; a Configure neither removes nor takes one, and the System app cannot add one
  (Domains shows it as "Link name" with the member, and its Remove is the only cleanup of a name no member sends). Only
  a front node keeps one, and only from a node whose Hello names a role with the edge set (the front link's Hello now
  names the dialing node's role). A refused claim is logged on the front; the member is not told. The row stays while the member is away, so its
  name is refused then rather than taken by another pairing or member. In the host table a link name is a passthrough
  route with its member (`Route::member`), and its tunnel goes to that member's link alone (`EdgeServer::link_to_member`,
  linked with its down connection open, whatever its state: a link is not balanced work), else the connection is
  closed.
- **SNI before ALPN on a front.** On a front node (`front` set without the edge set) `Acceptor::accept` looks the server
  name up first: a link name goes down its tunnel whatever link ALPN the client offers, so a local node's link
  (`moqui-edge/1`, its client certificate) and its pairing (`moqui-edge-pair/1`) to an edge member pass through the
  front untouched; one offering no link or ACME ALPN is refused. A link ALPN with any other name, an ordinary
  passthrough host's too, is the front's own link (review: else a pairing claiming a name the front is dialed at would
  take every other pairing's links). Link-name tunnels have a budget of their own beside the pairing's. Every other node keeps the ALPN first, and E15's ACME answer still comes after the
  passthrough check on both. On the edge member, a passthrough tunnel from its front whose ClientHello offers a link
  ALPN is taken by its own link acceptor (`EdgeServer::take_link`: the link's TLS settings with the ClientHello read
  already, the client the front named as the remote), so `serve`, `serve_up`, `serve_down` and the pairing connection
  now take any stream (`LinkIo`).
- **Mail through a front.** The MX of an edge cluster's domains can point at the front; the members' own port 25 may
  stay open for a second MX record ([Mail-Inbound](../docs/interfaces/Mail-Inbound.md), the edge node's part).
- **Tests.** Unit: `moqui-core` `edge::ports` (an smtp row on a front only, the edge set's ports free on a front),
  `edge::hosts` (a link name's claims), `moqui-host` `smtp` (a front link's PROXY header names the client and is
  checked against the allow-list, a direct client's is not read), `edge_link::hosts` and `down` (an SMTP tunnel needs
  the node's SMTP listener). `EdgeScreens.spec.js`: an smtp port refused on a node that is not a front. `it
  edge_front_ports` (regression, about 2.5 s each): `a_front_passes_smtp_to_an_edge_member_with_the_clients_address`
  (an edge member dialing its front from its properties names its link address there; the member's allow-list sees the
  client the front names; a direct client's PROXY header is a command not known; an edge member refuses an smtp row)
  and `an_edge_members_link_name_passes_through_the_front_to_it_alone` (two edge members' link names each to its own
  member from one client address; a local node pairs and links through the front at a member's link name; a member
  whose front link stopped has its name refused, and kept). Acceptance (run once, about 4 s):
  `edge_front_ports::acceptance_the_full_stack_keeps_a_site_up_with_a_node_below_the_front_stopped` (a front before an
  edge cluster of two members before a local cluster of two: the local members link to one edge member through the
  front at its link name and to the other at its own address, visitors are balanced over both edge members, a
  signed-in browser stays on one edge member and stays signed in when that edge member stops, and then when a local
  member stops too). The site is the local cluster's own app on a terminate host, not an mr-pass site: loading mr-pass
  and Mantle in a host test is too slow, and what the scenario checks is the path through the tiers. One test process
  keeps one edge link hook (the last started server's), so the tests dial the edge members' front links with test
  dialers built as `LocalConfig::front_from_properties` builds them; the SMTP test's single member dials from its
  properties.

### 4.8 Certificates and ACME

An ACME client, used from a screen and never on its own (decided Nov 12, 2026).

**What does not change.** Certificates are files in `runtime/cert`, found by domain and read again when they change.
A name with no certificate gets no HTTPS: its handshake is refused, and with no certificate at all the HTTPS listener
does not start. Nothing asks a certificate authority for a certificate because a name has none.

**System › Certificates,** on any node with an HTTPS listener, of most use on a front:
- the names this node answers (its own host, each terminate host of a pairing, its `public-site` hosts, the SMTP
  host, the XMPP domain), and for each the certificate found: its issuer, its expiry and where it came from. A name
  with none says what that means;
- **Set up with ACME** for a name: the authority's directory URL, the contact, its terms to agree to. The URL is
  filled in with Let's Encrypt's (decided Nov 12, 2026): a default property, `acme_directory_url`,
  `https://acme-v02.api.letsencrypt.org/directory`, with its staging directory offered for a first try and any other
  authority's URL accepted. It checks first that the name's DNS points here and that the port answers, then requests
  the certificate;
- **Renew now**, **Stop renewing** and **Remove** for a name set up this way.

**Renewal** is the one automatic part: a name the operator set up here is renewed by a job before it runs out, and a
failure shows on the System home with the expiry. A name nobody set up is never requested.

**How.**
- TLS-ALPN-01 on the HTTPS listener, which needs no port 80. For a local node's passthrough name the challenge goes
  down the tunnel untouched, so the local node gets its own certificate with no outside tool. HTTP-01 is an option
  where port 80 is open. No DNS-01 at first, so no wildcard names.
- The certificate is written in certbot's layout under `runtime/cert`, which the listener already reads.
- In a cluster one member orders, by a lock row. The challenge and the certificate are rows, the key encrypted, so
  any member answers the challenge and every member writes its own files.
- The account key is a secret in `.env`, and a keyring's once [Sealed-Secrets](Sealed-Secrets.md) is built.
- A library to check against the license gate in `deny.toml`: `instant-acme`.

**As built (E15, Nov 13, 2026).** `crates/moqui-host/src/acme/`, `org.moqui.admin.CertificateServices`,
`tools/screen/System/Certificates.xml`, the `moqui.server.Acme*` entities. What differs from the text above or adds to
it:
- **The first certificate comes from a challenge-only listener** (decided with the user, Nov 13). A node with no
  certificate at all still does not bind its HTTPS listener; once a name is set up here it does, and until a
  certificate is there it answers `acme-tls/1` only for a name with a challenge open and refuses every other
  handshake. So TLS-ALPN-01 gets a first certificate with no port 80, and down an edge node's tunnel. An explicit
  `webapp_https_listen_port` with no certificate is no longer an error at start: the listener stands by the same way.
  It also binds for a name set up on another member, a certificate added to `runtime/cert`, and the length of a
  set-up check (closing after it when nothing else wants it). An edge node that started so takes no links until it
  restarts with a certificate.
- **A cluster** sees a challenge another member wrote, changed or closed within a second: the rows are read again
  when the last read is a second old, and an order (and the check) waits past that before the authority looks.
  `acme_account_key` is shared in the members' `.env`.
- **The edge node** checks a passthrough host before the challenge's ALPN, so a local node's challenge goes down its
  tunnel untouched and the local node answers it; the edge node answers only for names it ends TLS for.
- **The check before an order** opens a challenge of its own with a random digest and connects to the name on 443
  (`acme_challenge_port`) offering `acme-tls/1`: its digest coming back shows the DNS and the port reach this node.
  A router that cannot connect to its own public address fails it, so the screen offers "order without the check".
  It runs on the request's thread, for at most about 20 seconds. Calls to the authority have time limits (1 and 3
  minutes), under the order lock's 15-minute lease; an AcmeOrdering name with no live lock was cut off and counts
  as failed.
- **The account** is a P-256 key derived from `acme_account_key` (a generated secret); the same key is one account at
  each authority, created at the first order. `acme_directory_ca` names a root for a company's own authority.
- **Renewal** is the `renew_AcmeCertificates_daily` job, seeded paused and started by the first set-up (paused again
  with the last Remove): a name renews with a third of its certificate's life left, or 30 days with no issue date, not
  again within six hours of a failure. A failure shows on the screen and on the System home with the expiry.
- **Files** carry a `.moqui-acme` marker; a `<name>/` directory another tool wrote is refused at set-up and never
  written over. Each member writes its files from the rows within a minute.
- **Tests.** Regression (`tests/it/acme.rs`, `src/acme/tests.rs`): no listener and no request with nothing set up;
  the challenge-only listener; the passthrough challenge down the tunnel; an order against an authority of the test's
  own (`tests/it/acme_authority.rs`, which checks the challenge as a real one does), served by the listener and renewed
  by the job, then stopped and removed (rows, files, the job paused); two cluster members on one database answering a
  challenge as it is now; the refusals (badName, termsNotAgreed, badDirectory, badContact, otherTool, checkFailed,
  notSetUp); the job's choice of names; a file in `runtime/cert` on the screen; the failure on the System home. Not
  tested here: httpsOff, and a passing check (the acceptance test runs it, on loopback DNS).
  Acceptance: `acme::acceptance_certificates_for_an_edge_node_and_its_local_node`. Real issuance is a hand check
  ([§2.1](#21-checks-on-a-deployed-edge-node)).
- **Not built:** HTTP-01 and DNS-01.

**The screen says what this is and is not.**
- An ACME certificate proves control of the domain name and nothing about who runs it. It is free, issued in
  seconds, short-lived, and depends on renewal working and on the authority being reachable and within its rate
  limits. It comes with no support and no warranty.
- Another certificate is the better choice where more assurance is wanted: an organization-validated or
  extended-validation certificate for ecommerce, where the authority has checked the business; a certificate under a
  support contract for a high-volume or high-reliability site; a wildcard; a company's own authority for inside
  names; a certificate a partner pins. Those are files in `runtime/cert`, as now, and the screen shows them beside
  the others.

### 4.9 Example confs for each topology

One directory under `runtime/conf/` for each topology of [§3](#3-topologies), with a conf file for each kind of node
in it. The single node's confs stay in `conf/` itself.

**Names** (decided Nov 12, 2026). Short, for what sets the topology apart, leaving out the tiers that are single or
obvious: `edge`, `all-in-one`, `multi-domain`, `local-cluster`, `local-failover`, `edge-cluster`, `front`,
`full-stack`, `pass`, `dev-group`. A dash joins a tier and its form. An underscore joins two where a name needs
both, such as `local-cluster_edge-cluster`; not every combination gets a directory.

**In each directory:**
- a conf per kind of node: `MoquiLocalConf.xml`, `MoquiEdgeConf.xml`, `MoquiFrontConf.xml` and so on, each with the
  comments that explain its settings, as MoquiEdgeConf.xml has now;
- a short `README.md`: the diagram, which file each node runs, the `.env` names each node needs, and the commands.

**The files there now move** (decided Nov 12, 2026): `MoquiEdgeConf.xml` to `conf/edge/`, `MoquiAllInOneConf.xml` to
`conf/all-in-one/`. About a dozen files name them (Run-Deploy and the interface guides, MoquiDefaultConf.xml's
comments, `EdgeClients.xml`, `addons.xml`, `mx.rs`).

**Kept true.** One regression test loads every conf under `runtime/conf/` over the default conf and checks that it
parses, merges and passes the schema.

**Run-Deploy.** The topology table gets a column for the directory. Each topology's section keeps what it is, its
diagram, its steps and what is not built. The settings move into the conf files' comments. The edge node section is
230 lines today; the per-area detail goes to the conf comments and to the interface guides, which already have a
"behind an edge node" part each.

**As built (E13, 2026-11-13).** `conf/edge/` (`MoquiEdgeConf.xml`, `MoquiLocalConf.xml`), `conf/all-in-one/`
(`MoquiAllInOneConf.xml`; its local node runs `../edge/MoquiLocalConf.xml`, the same node), `conf/pass/`
(`MoquiPassConf.xml`, `MoquiAuthorityConf.xml`), `conf/dev-group/` (`MoquiGroupConf.xml`) and `conf/local-cluster/`
(`MoquiLocalConf.xml`, every member), each with a README; `runtime/conf/README.md` indexes them. The dev instances and
the shared dev-aide server of `dev-group` keep `conf/MoquiDevConf.xml` with a few `.env` properties: one runtime conf
is merged, so a conf of their own would repeat the whole dev conf. The production nodes that hold data set
`embedded_db_backup_scheduled=true`. The test is `moqui-devkit`'s `runtime_conf_tests` (core cannot depend on the
devkit's checker): every `runtime/conf/**/*.xml` with a `moqui-conf` root parses; the file alone has no element,
attribute or value the schema does not take; merged over `MoquiDefaultConf.xml` with `merge_conf` it passes the whole
schema, required attributes included (an overlay takes them from the default conf's element); and every directory has
a README. It found the XSD missing `request-pool-max` and `request-queue` on `<tools>`, now added. The mail detail of
Run-Deploy's edge node section moved to Mail-Inbound's "The edge node as the gateway"; the section is about 85 lines.
The test runtimes copy `conf/` with its subdirectories (`moqui test`'s scratch runtime), the devkit project cache
stamps the confs in them, and `./moq`'s completion lists them.

**As built (E19 part C, 2026-11-14).** `conf/front/` (`MoquiFrontConf.xml`, the front node, `node_role=front`, for
both of its uses; `MoquiLocalConf.xml`, each member of a local cluster behind it: `MoquiPooledConf.xml` with the front
pairing's `front_node_*` in place of `edge_node_*`, no queue, and the cluster's own mail, calls and phones, mail in by
an `smtp` port on the front) and `conf/full-stack/` (`MoquiFrontConf.xml`, the front before an edge cluster;
`MoquiEdgeConf.xml`, each edge member, `front_edge` by default with `edge` explained, the mr-pass sites' `root-screen`
and `public-site` bindings, `edge_link_url` as the member's link name at the front, the SMTP listener the front's port
25 reaches; `MoquiLocalConf.xml`, each local member, the edge pairing, the second pairing with `edge` members and the
PASS authority). A conf of its own for each kind of node, rather than pointing at `conf/front` or `conf/local-cluster`:
one runtime conf is merged, so a pointer cannot carry the settings, and each says only what differs from the conf its
comments name. The READMEs have the diagrams, the `.env` names per tier, the DNS (in the full stack, which names point
at the front and which at each edge member's own address) and the order to pair. `runtime/conf/README.md` indexes
both, Run-Deploy has "A front node" and "Full stack" and their topology rows, and the conf test loads the six new
files.

### 4.10 The System app

System › Edge Clients today lists the paired nodes, makes pairing codes, and has a box of host names per node, the
queue and sets per node, and the replica conflicts. On a node with the front set it grows into these parts:

| Part | Shows and sets |
|---|---|
| **Clients** | Each pairing with its members, from the links and the cluster's member list: live, linked, version, address, round trip, work in flight, draining. Drain and resume a member; ping; revoke. A new node that took over a pairing, to see or to approve. Deliver again since a time. Make a pairing code |
| **Domains** | For each pairing, what it claims: host names with their mode (passthrough or terminate) and the sticky cookie's name; mail domains, the XMPP domain, the SIP realm and numbers, chat bridges, PASS sites. Add and remove a host name. The conflicts |
| **Ports** | The TCP ports that go to a pairing: SSH, XMPP, and SMTP on a front before an edge cluster |
| **Balancing** | The defaults (the sticky cookie, the set-aside times) and, live, the requests per member and the members set aside |
| **Certificates** | [§4.8](#48-certificates-and-acme); on every node with HTTPS, not only a front |
| **Edge cluster** | This node's own cluster members and each one's link address; the front nodes it is paired with |

- **Rows and conf.** A pairing's and a host name's settings are rows, since they change while the node runs: new
  fields on `EdgeClient` and `EdgeHost`, and a new entity `EdgePort`. Members are not rows. The defaults are
  attributes of the `<edge>` conf element, which the example confs show.
- **The wizard.** A local node still sends its own part with Configure. What it sends shows under Domains.
- **On a local node.** System › Edge Node shows each edge cluster member it is linked to, and its own cluster.
- **Twins and tests.** Terminal twins, as the edge screens have now; `EdgeScreens.spec.js` grows with them.
- **Metrics.** `moqui.edgelink.*` gain the pairing and the member: requests, set-asides, takeovers.

**As built (E16, the screens, 2026-11-13).** System › Edge Clients keeps its URL and menu entry and has three tabs
(`System/EdgeClients/{Clients,Domains,Ports}.xml`):
- **Clients:** each pairing (one node each until E17) with its state, last connect, address, version, generation, key,
  queue and queue cap; ping, revoke, a pairing code, the link key fingerprint, and the replicated sets. The queue cap
  is `update#EdgeClient` (`queueMaxMegabytes` above 0, empty for the conf's).
- **Domains:** the host names with their mode, added and removed here; what each active pairing claims
  (`get#EdgeDomains`: from its kept setup the mail domains, XMPP domain, SIP realm, chat bridges and PASS sites, and
  the numbers of its `voiceFallbacks` and `voiceTrunks` copies); the replica conflicts, now on a dedicated edge node
  too. The sticky cookie is E17's.
- **Ports:** 16B's `EdgePort` rows (`get#EdgePorts`, `add#EdgePort`, `remove#EdgePort`), each with whether it
  listens and why not.
- **Rows without the link.** The host name rows and the pairings' rows moved to `moqui_core::edge::hosts` (the host's
  `edge_link/hosts.rs` and `store.rs` call it), so the services work on any node that takes links whether or not its
  HTTPS listener started the link, and the screens show the rows with a warning while it has not; a host name change
  tells the link (`EdgeLink::hosts_changed`, in place of `hosts`, `add_host` and `remove_host`). Each refuses on a node
  that takes no links, by `node_role` as it is now (the ports' services too). A host name another pairing has is
  refused with "another node has this name", as Configure does (it was "a host of local node X").
- **The menu.** `default-menu-include` is an expression of `node_role`, so other nodes do not list Edge Clients; the
  page itself says what the node is. The tabs read the role in the parent's `always-actions`, which a tab rendered
  alone (the qvt client's load, a transition) runs.
- **Twins.** None: the XML renders in the terminal as it is, its state and mode chips by `render-mode` tui texts.
- **Tests.** `framework/test/EdgeScreens.spec.js` (regression, about 0.4 s): the host name rules (first claim,
  generic refusal, revoked pairing, bad name and mode, remove), the queue cap, the port rules (another row's port,
  this node's own, a revoked pairing, a bad kind), the refusals on a node that takes no links, an administrator's
  add and remove through Domains and a user without System refused, and each tab once in qvt and once in tui;
  `moqui-core` `edge::hosts` unit tests. The it tests that rendered Edge Clients render its Domains tab now.

**As built (E17 part D, the screens and confs, Nov 14, 2026).**
- **Clients** (`System/EdgeClients/Clients.xml`): each pairing gains its members linked and what a new node taking it
  over gets (New nodes: `hold#NewMembers`, admitted with a notice or held for approval, with the approved id). Below
  it, **Members** (`get#EdgeMembers`): each member of each pairing with its condition as a chip (ok, not linked, stale
  heartbeat, other version, stopped, held, refused), state, version, address, heartbeat age, when last heard and a
  refusal's text; Ping on a member row (`ping#EdgeClient` takes `instanceId`: a Ping down that member's own down
  connection, `EdgeLink::ping_member`, whatever its state), Drain and Resume (`drain#EdgeMember`), Approve on a held
  node (`approve#EdgeMember`) and Forget key on one not linked (`forget#EdgeMember`). **Takeovers** shows each pairing
  with a takeover noted (when, to, from) and Let it link again for the displaced node (`clear#EdgeTakeover`). Deliver
  again is a dialog (`deliver#ItemsAgain`, a pairing and a time). `get#EdgeMembers` answers none on a node that takes
  links with no link running (as under `moqui test`), and refuses on any other node.
- **Balancing** (`System/EdgeClients/Balancing.xml`, a fourth tab): the defaults (`sticky-cookie`, `set-aside-failures`,
  `set-aside-seconds`) and, from memory, each member's work: taking new work, set aside, draining or not linked as a
  chip, in flight, sent, failures in a row, times set aside and until when (17B's `get#EdgeBalancing`, which now answers
  the conf's defaults and no members on a node that takes links with no link running, and refuses on any other node).
  Read only: a host's cookie is set on Domains and a member is drained on Clients. Clients' Members show each member's
  work in flight from it.
- **Domains** gains a Sticky by column (a terminate host's own cookie or the conf's, a passthrough host by the client's
  address), a Sticky cookie field in Add a host name, and a Sticky cookie dialog on each terminate host's row
  (`add#EdgeHost` with its `stickyCookie`; empty for the conf's).
- **On a local node.** System › Edge Node › Status has **This node's cluster** (`get#LocalCluster`, new): its instance
  id, its state on its links, the pairing id (`edge_node_pairing`) and its cluster's members as its links tell the
  edge node (`pool::cluster_members`), with Drain this node and Resume (`drain#EdgeMember` with no pairing). It shows
  while paired, and while `edge_node_url` is set with the link not running in the process. The edge cluster members
  it is linked to are E18's.
- **A fix.** `drain#EdgeMember` chose between the edge node's and the local node's part by the `<edge>` settings kept
  at their first read; it reads `node_role` as it is now, as the other services do.
- **Confs** ([§4.9](#49-example-confs-for-each-topology)): `conf/local-cluster/MoquiPooledConf.xml`, every member of a
  cluster pooled by an edge node (`front_edge`, `conf/edge/MoquiEdgeConf.xml`): pairing once and its `.env` lines
  copied, the member's own instance id and key, admission, balancing, no `flush="guarantee"` and no trusted proxies,
  draining for a rolling upgrade; `conf/local-failover/MoquiLocalConf.xml`, the first machine and the standby: the
  same `.env`, never `runtime/db`, the three ways to the data, and the edge node's part. The edge node of both runs
  `conf/edge/MoquiEdgeConf.xml`. Run-Deploy has a section for each, and a row each in the topology table.
- **Review fixes (Nov 14).** A link installed with its HTTPS listener off (`EdgeLink::serving` false) is
  `notStarted` to `get#LinkStatus`, and Clients, Domains and Balancing show the rows and the conf's defaults; members
  and balancing are read in probes of their own, so a failure there never hides the host names or Revoke. Drain on a
  pairing's last live member, and Drain this node on a cluster's only live member, say that the names go offline.
  Forget key keeps the id as forgotten (`pool::forget_key`), so its next link is a new node's (a takeover with its
  notice, or held), not the last member coming back; Forget key, the hold and Deliver again are logged, and Deliver
  again refuses an id that is not paired. Add a host name keeps a host's own cookie; its Sticky cookie dialog sets or
  clears it.
- **Tests.** `EdgeScreens.spec.js` (regression, about 0.6 s for its ten features; Balancing rendered once in qvt and
  tui, Domains' render shows a host's own cookie and a passthrough host's, `get#EdgeBalancing`'s defaults and its
  refusal): the pool's rows (hold, approve
  and a bad id, forget, clear with the takeover kept, Deliver again and its date required, no members without a link,
  a member's ping refused) with a stranger pairing and a node that takes no links refused; Let it link again through
  Clients by an administrator and not by a user without System; Clients' render with a takeover and a hold (qvt and
  tui); Edge Node's own cluster rendered draining (qvt and tui) with Drain and Resume. `edge_pool`'s members test
  pings a member by id.

**As built (E18 part C, the screens and conf, Nov 14, 2026).**
- **Edge cluster** (`System/EdgeClients/EdgeCluster.xml`, a fifth tab): this node's own cluster from `get#EdgeCluster`:
  each member live or stopped as a chip, its link address (`edge_link_url`; one that is not an `https` URL with a host
  is shown as not used, as the Hello answer sends none), its heartbeat's age and version, this node marked, and the
  member that registers the SIP trunks (`get#EdgeCluster`'s `registersTrunks`: the holder of 18B's lock row
  `sipTrunks` while its lease lasts by the database's clock, by the member id the SIP engine claims it with). A
  warning names each live member with no usable address while more than one is live; with one, a line says it needs
  none. Read only: an address is each member's own property. The front nodes it is paired with are E19's and not on
  the tab yet.
- **`get#EdgeCluster`** answers from the `ClusterMember` rows and this node's `edge_link_url` as it is now where no link
  serves (the HTTPS listener off, as under `moqui test`), as 17D made the members and balancing answer, and refuses on a
  node that takes no links; a member with no heartbeat written has an empty `heartbeatAgeMs` (it was `i64::MAX`), and
  `linkUrlUsable` says whether its address is one the Hello answer sends. The list and that check are one place,
  `moqui_core::edge::cluster_list` (`members`, `usable_link_url`), which the link's `edge_cluster` and the Hello
  answer's `members::of_cluster` call too.
- **On a local node** System › Edge Node › Status has **Edge cluster** when its edge node is a cluster of more than one
  (`get#LinkStatus` `edgeMemberList`): each edge member's id (Not known yet before its first Hello), the link's state,
  address, round trip, last connect, links lost and a note (which link carries this node's work up, a member gone from
  the list, the last error while not linked); the Link box says it is of the link that carries the work.
- **Conf** ([§4.9](#49-example-confs-for-each-topology)): `conf/edge-cluster/MoquiEdgeConf.xml`, every edge member
  (`front_edge` on one external database, no backup job, which covers an embedded database only): the shared `.env`
  with `edge_link_key`, each member's own `cluster_base_url`, `edge_link_url`, `smtp_hostname`, `rtc_public_address`,
  `turn_public_host` and `sip_public_address`, and what each area does in a cluster. A local node runs
  `conf/edge/MoquiLocalConf.xml` unchanged (or `conf/local-cluster/MoquiPooledConf.xml` per member), so the directory
  has no local conf. Its README has the diagram, the `.env` names, the DNS (an A, MX and SRV record per member) and the
  commands; Run-Deploy has a section and a topology row.
- **Tests.** `EdgeScreens.spec.js` (regression, its new feature about 0.03 s): the cluster from the rows with a stopped
  member's address, this node's address that is not https, the trunk lock's holder and a lapsed lease, the note for one member, Edge
  cluster rendered once in qvt and in tui, and the refusal on a node that takes no links. `moqui-core`
  `edge::cluster_list` (the address check) and `service::edge` (`edgeMemberList`'s shape: one link carrying the work, a
  retired one); `it edge_cluster`'s first test checks `get#EdgeCluster`'s `linkUrlUsable` with the link running. The
  Edge Node box is not rendered with a running link: its rows are the `edgeMemberList` the unit test shapes.

**As built (E19 part C, the screens, Nov 14, 2026).**
- **Edge cluster** gains **Front node**: the front this node is paired with, which every member dials with the shared
  `.env`, from `get#EdgeCluster`'s new `frontList` (this node's link to each member of the front's cluster, else the one
  link, with the front's name and pinned fingerprint; where no link serves, one row from `front_node_url` and
  `front_node_id`, not started here): each front member's link state as a chip, address, round trip, last connect and
  last error, and a link to Edge Node, where Unpair front is. On a node with the edge set and no front pairing a line
  says how to pair one; on a front node the tab lists its own cluster as front nodes and has no such box.
- **Edge Node › Status** for a front pairing: paired with a front alone (a member of a local cluster behind it,
  `conf/front`), the Link box is the front's, Setup says mail, chat, PASS, calls and phones are the node's own, and the
  Queue and replication box gives way to a note that a front pairing carries none; This node's cluster shows while
  paired by `front_node_url` too. The Front node box beside an edge pairing shows the front's last connect and
  fingerprint. Edge Clients' notice for a node that takes no links names `front`.
- **Tests.** `EdgeScreens.spec.js`'s Edge cluster feature (regression, about 0.06 s) checks `frontList` from
  `front_node_*` and its absence, and the Front node box in its one qvt and tui render; `moqui-core` `service::edge`
  `front_rows` (a row per front link, one from the properties, none without). A front-only pairing's Edge Node needs a
  running link, so that page is not rendered in a spec.
- **Review fixes (Nov 14).** `get#LinkStatus` answers `frontOnly` from what pairing wrote (`front_node_url` set,
  `edge_node_url` not), so Edge Node shows a front-only pairing as such before its link first comes up (the front's
  role in the status is empty until then); `frontList` falls back to the properties when a link serves but no front
  dialer runs, and shows `front_node_id`'s ids as a list; the hint to pair a front is one line in Members. The docs
  say how an edge member gets its first certificate (the edge cluster's own name at the members' addresses, as the
  link server needs one before any link name's ACME challenge can pass through the front), that the front is a single
  point for the links in the full stack unless `edge_link_url` names the member's own address, that a ban is not told
  on past the next node, and the mail records a front needs (SPF of the members' addresses, the MX name's certificate
  on every member, MTA-STS since the front relays the bytes).

### 4.11 Security

- **Admission** is the new trust decision. A pool member holds the pairing's key and is on the member list a linked
  member reports. A stolen `.env` alone no longer takes a pairing's traffic while a real member is linked. With none
  linked the key alone admits, which is what lets a standby in; the notice and the approval hold of
  [§4.5](#45-failover-to-a-standby) are the guard.
- **The member list** comes from a paired node and is input like any other: sizes and ids checked, used only for
  that pairing.
- **A front node** in terminate mode sees the sites' content. It holds certificates for their names and no data.
- **Several pairings.** The limits of [§4.1](#41-domains-on-one-edge-node), and a TURN secret per pairing.
- **ACME.** The account key and the certificates' keys are secrets. The challenge answers only for a name the
  operator set up.
- [Attack-Surface](../docs/security/Attack-Surface.md) gets each phase's part: the member list on the link, the
  list of edge members in Hello, an edge node that dials, TCP ports on a front, the ACME challenge.

## 5. Phases

| Phase | What | Acceptance |
|---|---|---|
| **E13** | Example confs for the topologies built today, in their directories, with MoquiEdgeConf.xml and MoquiAllInOneConf.xml moved; the conf test; Run-Deploy's sections cut down to them. Each later phase adds its directory. **Built 2026-11-13**: `edge`, `all-in-one`, `pass`, `dev-group` and `local-cluster`, each node started from its conf; the edge node's backup job runs by `embedded_db_backup_scheduled`; the test is `moqui-devkit`'s `runtime_conf_tests` ([§4.9](#49-example-confs-for-each-topology), As built) | Each built topology starts from its directory's files by its README. Run-Deploy names a directory for every topology. The test loads every conf |
| **E14** | Built 2026-11-13. Root screens by host name: the ten checks of [§4.2](#42-root-screens-by-host-name), what they find fixed, the example conf and the mr-pass per-app hosts (`Pass.xml` as each host's root) | A served instance answers two host names with two root screens: the shell, static files, sign-in, errors and links right on each, each host with its own session. mr-pass serves the Forum and the Shop each at the root of its own host |
| **E15** | Built Nov 13, 2026. Certificates: the screen and the ACME client ([§4.8](#48-certificates-and-acme)); acceptance test `acme::acceptance_certificates_for_an_edge_node_and_its_local_node` | A node with no certificate for a name serves no HTTPS for it and asks nobody. From the screen an edge node gets a certificate for a terminate host, and a local node one for a passthrough name through the tunnel. Both renew by the job. A file put in `runtime/cert` shows beside them |
| **E16** | Domains: owner routing on a dedicated edge node, each area by its key, claims between pairings, limits per pairing, a TURN secret per pairing, PASS by site, XMPP by domain on one port; the Clients, Domains and Ports screens. **Built 2026-11-13**, the screens and `conf/multi-domain` last ([§4.10](#410-the-system-app), As built). Routing part: web, mail, chat webhooks, PASS by site, delegate and the queue by owner, claims and conflicts between pairings, the queue cap and MX share per pairing ([§4.1](#41-domains-on-one-edge-node), As built); the acceptance test is `edge_domains::one_node_away_or_overloaded_leaves_the_other_working` (web, mail, chat, PASS, delegate; SIP and XMPP to add) | Two local nodes on separate databases pair with one edge node, each with its own host names, mail domain, bridge, numbers, XMPP domain and PASS site, all at once. Each one's work reaches only it. A name both claim is refused, naming it. One local node away or overloaded leaves the other working |
| **E17** | **Part D built 2026-11-14** ([§4.10](#410-the-system-app), As built): Clients' members, takeovers, hold and Deliver again, the Balancing tab and the sticky cookie on Domains, a local node's own cluster and drain, the `local-cluster` with an edge node and `local-failover` confs. **Part A built 2026-11-13** ([§4.4](#44-pools-load-balancing), [§4.5](#45-failover-to-a-standby), As built): the roles, every member linking, the member list and its report, admission, member state and drain, failover with its notice, the hold and Deliver again since; tests `edge_pool` (regression) and its three acceptance tests. **Part B built 2026-11-13** ([§4.4](#44-pools-load-balancing), As built): balancing by protocol, sticky by the session cookie (a host's own name on `EdgeHost`) or the client's address, health with set-aside, a GET or HEAD sent once more, call signaling per member, `get#EdgeBalancing`; tests `edge_balance` (regression) and its two acceptance tests. The front option set and the `node_role` values (`front`, `front_edge`, `edge`); pools: the member list in memory and its report, admission, balancing by protocol with sticky sessions, health and drain, the outbox and replication from a pool; failover to a standby with its notice and Deliver again since; web protection's rules per domain, shared bans and view; the Balancing screen. **Part C built 2026-11-13**: the outbox and replication from a pool, the link's own rows from the sender, web protection's rules per host, shared bans and System › Security › Web Protection ([§4.3](#43-the-front-option-set), [§4.4](#44-pools-load-balancing), As built); tests `edge_pool_work` | Two members of a cluster link and share a host name's requests evenly; a browser session stays on one member, and moves when it stops, still signed in. Clients shows a cluster member that is live but not linked. A node on another database with the same key is refused while a member is linked. With the local node stopped, a standby started by hand takes over its names and gets the queued items once; the first machine, started again, is refused. A drained member finishes its requests and gets no new ones |
| **E18** | The edge cluster: the list of edge members in Hello, a local node's link to each, the in-memory state made right, calls and SIP on several edge members; the Edge cluster screen. **Part A built 2026-11-14** ([§4.6](#46-a-cluster-of-edge-nodes), As built): `edge_link_url`, the list in Hello and Ping answers, a link to each member kept in `runtime/db/edge_members`, the work up one link, `get#EdgeCluster` and `edgeMemberList`; tests `edge_cluster` (regression) and its two acceptance tests. **Part B built 2026-11-14** ([§4.6](#46-a-cluster-of-edge-nodes), As built): the `edge-state` cluster notice (host table, replicated copies, revocation), admission by `EdgeMemberLinks` rows (and the missed-notice beat), the `sipTrunks` lock, a `CallMediaNode`, `SipNode` and TURN secret per edge member; tests `edge_cluster_state` (regression) and its mail acceptance test **Part C built 2026-11-14** ([§4.10](#410-the-system-app), As built): the Edge cluster tab, System › Edge Node's links to each edge member, `conf/edge-cluster`, Run-Deploy's section; Mobile-Desktop's question answered | A local node links to both members of an edge cluster. A request to either reaches it. With one edge member stopped, web, mail in and the queue go on through the other, and nothing is delivered twice. A clustered all-in-one server takes links on every member |
| **E19** | The front node: `node_role=front`, an edge node that dials (`front_edge` behind the front first, then `edge` with a local node's second pairing), TCP ports on the front, the local nodes' links through it; the `front` and `full-stack` confs. **Part A built 2026-11-14** ([§4.7](#47-a-front-node), As built): the front node with the front set only, the front pairing (`front_node_*`) and what goes on it, an edge node that dials its front and forwards its local nodes' names, a local node with two pairings; tests `edge_front` (regression) and its acceptance test. **Part B built 2026-11-14** ([§4.7](#47-a-front-node), As built): SMTP ports on a front with a PROXY header, the edge members' link names as member-pinned passthrough hosts, picked by SNI before the ALPN on a front; tests `edge_front_ports` (regression) and the full-stack acceptance test (a plain screen site in place of mr-pass). **Part C built 2026-11-14** ([§4.9](#49-example-confs-for-each-topology), [§4.10](#410-the-system-app), As built): `conf/front` and `conf/full-stack`, Run-Deploy's sections, the guides, mr-pass's Full stack, the front node on Edge cluster and Edge Node; tests `EdgeScreens.spec.js` and `service::edge` | A front node balances a local cluster with web protection and no edge node. Visitors reach an mr-pass site through a front node, balanced over two edge cluster members, sticky by session, with a local cluster as their authority. Stopping any one node below the front leaves the site up. The acceptance test (`edge_front_ports`) serves a plain screen site; the mr-pass site is a [§2.1](#21-checks-on-a-deployed-edge-node) hand check |
| **E11** | Filtering in moqui, as in the archived plan; after the first release | To be set when it is designed |
| **Later** | A PASS read cache on the edge node; monitoring over the link, with the link-down alert a standby's automation would watch; sealing queued items; a local media node cascading to the edge node, and regional media nodes; cascading between edge cluster members; the voicemail fallback following rules and schedules; the wizard's undo; QUIC; DANE; typed gRPC messages and mutual TLS for direct PASS | |

**Before 0.1.0:** E13 to E19, in this order, and the checks of [§2.1](#21-checks-on-a-deployed-edge-node), which
need none of them. E13, E14 and E15 stand alone and are small. E16 to E19 build on each other; if something must
wait, it is from the end. **After:** E11 and Later.

## 6. Documents to change

- **Run-Deploy:** the topology table and each section, as E13 says; then a section for each new topology, and
  Certificates with E15.
- **Guides:** Attack-Surface with each phase; the interface guides' edge node parts where a pool or an edge cluster
  changes them (WebRTC, SIP, TURN, XMPP, Mail-Inbound, Chat-Bridges, SMS).
- **mr-pass:** `docs/Deployment.md`, the per-app hosts and the full stack.
- **Plans:** Mobile-Desktop's open question on a clustered all-in-one server (E18); Dogfood F10 (E15);
  [Remaining-Work](Remaining-Work.md#edge-node-and-deployment-topologies) lists the phases; Future-Plans has E11 and
  Later.

## 7. Decisions

**Decided 2026-11-12**, from the user's notes and then answers. The archived plan's decisions 1 to 8 stand; E16
builds the rest of decision 1.

1. **Several domains on one edge node,** routed by domain name to the local node that owns each. Reasonable
   isolation, not multi-tenant: more like the organizations inside one database.
2. **Root screens by host name** are supported as in Moqui Java, for larger deployments. Not the default, and not
   used for moqui.org.
3. **Pooling is the default.** Several nodes that register for one domain name are pooled and balanced evenly. A node
   must already be in the cluster: local nodes join the cluster, then register with the edge node. One that is not
   is refused.
4. **Sticky sessions** for web traffic by a configurable cookie, our session cookie's name by default. Other traffic
   is balanced as each protocol allows, sticky wherever it has something like a session.
5. **An edge cluster** shares a database, and a local node that connects to one member connects to each and takes
   requests from each.
6. **Example confs** for each topology in a directory under `runtime/conf/`, the single node in `conf/` itself;
   Run-Deploy refers to them and leaves the settings to them. The System app on the edge node sets up domains and
   load balancing.
7. **Directory names** are short, for the distinguishing feature, with a dash inside a tier and an underscore
   between tiers: `local-cluster`, `edge-cluster`, `full-stack`. Not two-letter codes. The single and obvious tiers
   are left out.
8. **Failover is simple.** A hot standby is started when the local machine dies, and takes its place at the edge
   node. Not automatic, unless the operator builds the automation.
9. **Front is an option set of its own.** It is deployed with the edge set, to balance a local cluster, or apart from
   it as a front node. A front is more than a balancer: it has the web protection and the rest of what a front does.
10. **The local cluster's member list** goes to the edge node, kept in memory and not on disk. Admission reads it,
    and the edge node reports the cluster members' status from it.
11. **The newer conf files move** into their topology directories.
12. **ACME by the screen only.** It does not run by default. A name with no certificate gets no HTTPS and no
    certificate is requested for it. An operator sets a name up in the System app, which says what an ACME
    certificate is and is not, and when another kind is the better choice.
13. **The session cookie stays tied to its host.** A deployment's own domains federate by SSO.
14. **The bulk of this plan comes before 0.1.0.** It sets MR Moqui apart, saves administration, and opens these
    topologies to small businesses and homelabs.
15. **"Local node"** stays the word for the node behind an edge node.
16. **The roles are combinations:** `node_role` is `front`, `front_edge` or `edge`, with `all-in-one` as built.
17. **A new node taking over a pairing** is admitted and the administrators are told. Holding it for approval is a
    setting.
18. **SSO between a server's own hosts** is PASS's Account for now. The framework as a sign-in provider is later.
19. **Set up with ACME suggests Let's Encrypt,** with its endpoint as the default setting.

## 8. For the review pass

Every question is answered. These are the points this plan chose on its own, for the user's pass through it, each
with what it became (Nov 14, 2026):

1. **What a front does beyond web protection as built** ([§4.3](#43-the-front-option-set)): rules per domain, shared
   bans and the view for the first release; compression, security headers per host and a static cache later. The
   user's answer of Nov 12: good enough for now, to review. **Became:** rules per host, shared bans (between members
   and up each link, to a front too) and System › Security › Web Protection, built in E17 part C; the rest is Later
   ([§5](#5-phases)).
2. **Renewal.** A name the operator set up with ACME is renewed by a job, with no further step
   ([§4.8](#48-certificates-and-acme)). It is the one thing there that runs on its own. **Became:** built in E15,
   `renew_AcmeCertificates_daily`, running only while a name is set up, a failure shown on the System home.
3. **`edge` members in a full stack** need a local node to hold two pairings, one with the front and one with the
   edge cluster ([§4.7](#47-a-front-node)). E19 builds `front_edge` members first. **Became:** both built in E19 part
   A: `front_edge` members forward their local nodes' names to the front, and with `edge` members a local node holds a
   front pairing beside its edge pairing (`front_node_*`), the wizard's parts split between them. `conf/full-stack`
   runs `front_edge` by default (one pairing per local node) and explains `edge`.
4. **Additions to the simple failover** ([§4.5](#45-failover-to-a-standby)): the first machine refused while the
   standby is linked, the notice, and Deliver again since. **Became:** built in E17 parts A and D, with the hold for
   approval, and the displaced node refused until an administrator lets it link again.
5. **`node_role=edge` changes its meaning** to the edge set alone; today's edge node becomes `front_edge`
   ([§4.3](#43-the-front-option-set)). **Became:** built in E17 part A, with no alias for the old meaning; the confs,
   guides and tests say `front_edge`.
