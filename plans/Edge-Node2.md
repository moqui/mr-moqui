# Edge Node 2: domains, pools, edge clusters and the front

Status 2026-11-12: plan; nothing here is built. It follows [Edge-Node](archive/Edge-Node.md), archived the same day
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

### 2.2 Not built

| Item | What is missing | Goes to |
|---|---|---|
| Several local nodes on one edge node (decision 1) | The edge node's rows are keyed by node, but several paths still take "the only paired node" ([§4.1](#41-domains-on-one-edge-node)) | E16 |
| A clustered local node | One member was to hold the link through a lock row; the lock row is not built. The link is matched by key, so members that share a `.env` would replace each other's link (to verify). A call's socket on a member without the link is refused | E17: every member links |
| A second edge node | A secondary MX, a second webhook URL | E18 |
| A clustered all-in-one server | Which member holds the edge role | E18: every member takes links |
| PASS sites for a paired node on an all-in-one node | `pass_backend_url=edge:` means one authority for the whole node | E16: the authority by site |
| XMPP for two domains on one port | SNI for direct TLS, the stream's `to` for STARTTLS; today a pair of tunnel ports per node | E16 |
| All-in-one gaps | A voicemail sidecar requeued after a restart goes to the only paired node; a paired number with no fallback row is found only by its trunk | E16 |
| A TURN secret per pairing | Revoking a node does not end its TURN access: the edge node's one `turn_auth_secret` must be rotated | E16 |
| An ACME client | Certificates for an edge node's terminate hosts, and through the tunnel for a local node's passthrough names. Also [Dogfood](Dogfood.md) F10 | E15: by a screen, never on its own ([§4.8](#48-certificates-and-acme)) |
| The System menu | It cannot leave a screen out by a condition, so Edge Node and Edge Clients show on every node | Framework, with E16's screens |
| The edge node's backup job | A conf cannot unpause a seed job, so the operator unpauses `backup_EmbeddedDatabase_daily` by hand | E13, if a conf property can do it |
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

- The `_edge_` refusals of `SipDevices.js` while phones are off: the spec engine cannot set a process property.
- Mantle's own `handle#Call` through the host: loading mr-mantle in a host test is too slow for a fast test.
- The TURN check over TCP or TLS only; the same SIP username in both domains and the 409 on the media path of an
  all-in-one node.
- `edge_link::pairing_links_both_ways_and_unpairing_cuts_the_link` failed once on a port already in use
  ([Remaining-Work](Remaining-Work.md#tests)).

## 3. Topologies

The topologies this plan ends with. Each gets a directory of example confs under `runtime/conf/`, named for what
sets it apart ([§4.9](#49-example-confs-for-each-topology)).

| Topology | Use it for | Conf directory | State |
|---|---|---|---|
| One node | Development, one production server, the demo | `conf/` itself | Built |
| One node, several sites by host name | Separate sites on sub-domains or related domains, one server | `conf/` (`MoquiSitesConf.xml`) | E14 |
| PASS servers with a BOSS authority | Public sites with no business data on the public machines, direct REST or gRPC | `conf/pass/` | Built |
| Dev instances with a group server | A team | `conf/dev-group/` | Built |
| A local node with an edge node | A server in the office or at home with a public presence; the desktop and mobile apps as its clients | `conf/edge/` | Built; the apps' exit checks are open |
| A local node with an all-in-one node | The same, with a full public server as the edge node | `conf/all-in-one/` | Built |
| Several local nodes on one edge node | Branch offices, or separate organizations of one group, each with its own domains | `conf/multi-domain/` | E16 |
| A local cluster | More capacity, or a member that can fail: pooled and balanced by an edge node, or behind a load balancer of your own | `conf/local-cluster/` | Built behind your own balancer; E17 with an edge node |
| A local node with a standby | A second machine started when the first one dies | `conf/local-failover/` | E17 |
| An edge cluster | An edge node that can fail, or more capacity at the edge | `conf/edge-cluster/` | E18 |
| A front node before a local cluster | Load balancing and web protection, with none of the edge node's comms | `conf/front/` | E19 |
| Full stack | A higher-volume shop or other public site on mr-pass: a local cluster, an edge cluster, a front node, the desktop and mobile apps | `conf/full-stack/` | E19 |

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
  restart;
- the SSH tunnel (`edge_ssh_tunnel_node`) and the XMPP tunnels (`edge_xmpp_node`): one node per port;
- a chat bridge id that two nodes have, which gets 401;
- the offer's `other_nodes` warnings for calls and phones.

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

**Isolation.** Reasonable, not multi-tenant. One operator runs the edge node, and the pairings belong to one
organization or to related ones, as the organizations inside one Mantle database do. The aim is that a mistake or an
overload in one domain does not break or leak into another. It is not a defense against a hostile pairing.
- **Kept apart:** every edge row is keyed by pairing, and no call from one pairing reads another's rows. Each pairing
  has its own limits: the queue's size cap (to check: it may be node-wide today), streams and tunnels on its links
  (built per node), MMS files (built per node), a share of the MX delivery threads, and a cap on call legs.
- **Shared on purpose:** the edge node's addresses and their mail reputation, the DKIM key and selector (one key,
  published in each domain's DNS), the media and TURN ports, web protection's counts, CPU and bandwidth.
- **Not provided:** an operator per pairing with its own view of the edge node, billing, or quotas enforced against
  a pairing that attacks.

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

**What is balanced, by protocol.** Sticky wherever the protocol has something like a session.

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

**Work from the pool to the edge node.**
- **The outbox.** Items are rows claimed by lease, so every linked member sends. Order per key holds: only the head
  of a key is claimed.
- **Replication.** Change notes are in memory in the member that wrote, and the cursor is one sequence per set. So
  one member sends, chosen by a lock row as scheduled jobs are, and the others pass their notes to it on a cluster
  topic. A new sender starts each set with a full send, as a restart does today. The sets are small.
- **The link's own rows** (`CallMediaNode`, `SipNode`) and the minute's `refresh#PhoneFallbacks` are written by the
  replication sender only.

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
- the SIP registrar's nonce counts and the voicemail fallback's forward counts, per member (the nonce's MAC key is
  shared, so any member checks a nonce);
- the webhook replay cache: the queue's idempotency key already makes a repeat one item;
- pairing tries and web protection's counts, per member, as in any cluster.

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
| **E13** | Example confs for the topologies built today, in their directories, with MoquiEdgeConf.xml and MoquiAllInOneConf.xml moved; the conf test; Run-Deploy's sections cut down to them. Each later phase adds its directory | Each built topology starts from its directory's files by its README. Run-Deploy names a directory for every topology. The test loads every conf |
| **E14** | Root screens by host name: the ten checks of [§4.2](#42-root-screens-by-host-name), what they find fixed, the example conf and the mr-pass wrapper roots | A served instance answers two host names with two root screens: the shell, static files, sign-in, errors and links right on each, each host with its own session. mr-pass serves the Forum and the Shop each at the root of its own host |
| **E15** | Certificates: the screen and the ACME client ([§4.8](#48-certificates-and-acme)) | A node with no certificate for a name serves no HTTPS for it and asks nobody. From the screen an edge node gets a certificate for a terminate host, and a local node one for a passthrough name through the tunnel. Both renew by the job. A file put in `runtime/cert` shows beside them |
| **E16** | Domains: owner routing on a dedicated edge node, each area by its key, claims between pairings, limits per pairing, a TURN secret per pairing, PASS by site, XMPP by domain on one port; the Clients, Domains and Ports screens | Two local nodes on separate databases pair with one edge node, each with its own host names, mail domain, bridge, numbers, XMPP domain and PASS site, all at once. Each one's work reaches only it. A name both claim is refused, naming it. One local node away or overloaded leaves the other working |
| **E17** | The front option set and the `node_role` values (`front`, `front_edge`, `edge`); pools: the member list in memory and its report, admission, balancing by protocol with sticky sessions, health and drain, the outbox and replication from a pool; failover to a standby with its notice and Deliver again since; web protection's rules per domain, shared bans and view; the Balancing screen | Two members of a cluster link and share a host name's requests evenly; a browser session stays on one member, and moves when it stops, still signed in. Clients shows a cluster member that is live but not linked. A node on another database with the same key is refused while a member is linked. With the local node stopped, a standby started by hand takes over its names and gets the queued items once; the first machine, started again, is refused. A drained member finishes its requests and gets no new ones |
| **E18** | The edge cluster: the list of edge members in Hello, a local node's link to each, the in-memory state made right, calls and SIP on several edge members; the Edge cluster screen | A local node links to both members of an edge cluster. A request to either reaches it. With one edge member stopped, web, mail in and the queue go on through the other, and nothing is delivered twice. A clustered all-in-one server takes links on every member |
| **E19** | The front node: `node_role=front`, an edge node that dials (`front_edge` behind the front first, then `edge` with a local node's second pairing), TCP ports on the front, the local nodes' links through it; the `front` and `full-stack` confs | A front node balances a local cluster with web protection and no edge node. Visitors reach an mr-pass site through a front node, balanced over two edge cluster members, sticky by session, with a local cluster as their authority. Stopping any one node below the front leaves the site up |
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

Every question is answered. These are the points this plan chose on its own, for the user's pass through it:

1. **What a front does beyond web protection as built** ([§4.3](#43-the-front-option-set)): rules per domain, shared
   bans and the view for the first release; compression, security headers per host and a static cache later. The
   user's answer of Nov 12: good enough for now, to review.
2. **Renewal.** A name the operator set up with ACME is renewed by a job, with no further step
   ([§4.8](#48-certificates-and-acme)). It is the one thing there that runs on its own.
3. **`edge` members in a full stack** need a local node to hold two pairings, one with the front and one with the
   edge cluster ([§4.7](#47-a-front-node)). E19 builds `front_edge` members first.
4. **Additions to the simple failover** ([§4.5](#45-failover-to-a-standby)): the first machine refused while the
   standby is linked, the notice, and Deliver again since.
5. **`node_role=edge` changes its meaning** to the edge set alone; today's edge node becomes `front_edge`
   ([§4.3](#43-the-front-option-set)).
