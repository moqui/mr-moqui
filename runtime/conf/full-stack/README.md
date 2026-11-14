# The full stack

A higher-volume shop or other public site on mr-pass: a front node that faces visitors and mail servers, an edge
cluster behind it that serves the sites and the comms, and a local cluster behind that with the apps, the data and the
PASS authority. The desktop and mobile apps are clients of the local cluster's host names. Every tier can lose a node:
the front balances the edge members, each local member links to every edge member, and the edge members balance the
local members. The front is a single point in this layout, for the local nodes' links to the edge members as well as
the web: while it is down their queue, delegated calls and replication stop too, unless each edge member's
`edge_link_url` names its own address. Steps and limits: [Run-Deploy](../../../docs/Run-Deploy.md#full-stack); the sites:
mr-pass's `docs/Deployment.md`, Full stack.

```
 visitors, mail servers, chat providers                 calls, TURN, SIP trunks and phones
    |  443, 25                                               |  each edge member's own address
 front node     front.example.com     MoquiFrontConf.xml     |  pairings, names, certificates; no data
    ^  each edge member dials the front: one pairing, pooled  |
 edge 1   edge1.example.com   edge 2   edge2.example.com   <-+  MoquiEdgeConf.xml; 8444 between members only
    +----------+----------+   shared Postgres: the queue, pairings, host names, copies
    ^  each local member dials every edge member, at its link name through the front
 local 1   local 2   local 3      MoquiLocalConf.xml; 8444 between members only
    +----------+----------+   shared Postgres: the apps, the data, the PASS authority
```

| Node | Conf | Package and runtime |
|---|---|---|
| Front node | `MoquiFrontConf.xml` (as [`../front/MoquiFrontConf.xml`](../front/README.md)) | Maximal; base components only |
| Each edge member | `MoquiEdgeConf.xml` | Maximal with an external database, or Enterprise; base components and mr-pass (`./moq component-get edge`), the same on each |
| Each local member | `MoquiLocalConf.xml` | Enterprise (`--enterprise`) with the apps (`./moq component-get mr-boss`), the same on each |

The edge members run `node_role=front_edge` (the default here): they forward their local nodes' names, and a local node
has one pairing. With `node_role=edge` they forward no web access and bind no tunnel port, and each local node pairs
with the front as well, for its names, XMPP domain and ports.

## `.env`

- **Front node**: `webapp_http_host` (`front.example.com`), `edge_public_addresses` (its addresses), and the secrets
  of its first start (`edge_link_key` among them). A certificate in `cert/` for its own name and the sites' hosts, or
  System › Certificates with ACME.
- **Every edge member, the same**: as in [`../edge-cluster/`](../edge-cluster/README.md) (the database, the secrets
  with `edge_link_key`, `turn_auth_secret`, `mail_dkim_key` and `acme_account_key`, `edge_mail_bounce_domain`,
  `sip_port`, `sip_tls_port`, `pass_backend_web_url`), with `edge_public_addresses` set to the front's addresses, and
  `smtp_listen_port` (`smtp_listen_host=127.0.0.1`) for the mail the front passes down. Once paired with the front:
  `front_node_url`, `front_node_key`, `front_node_pin` and `front_node_id`.
- **Each edge member, its own**: `cluster_base_url`, `edge_link_url=https://edge1.example.com` (its link name, at the
  front), `smtp_hostname` (its own address's reverse DNS name), `rtc_public_address`, `turn_public_host` and
  `sip_public_address` (its own address). Never copy `runtime/db`.
- **Every local member, the same**: the database and secrets, and once paired: `edge_node_url`, `edge_node_key`,
  `edge_node_pin`, `edge_node_id`, `edge_node_pairing`; with `edge` members also `front_node_*`; the PASS client
  lines the wizard writes (`pass_edge_client`, `pass_client_edge_sites`, `pass_client_edge_callback`).
- **Each local member, its own**: `cluster_base_url`. Never copy `runtime/db`.

## DNS

- **At the front's addresses**: `front.example.com`; the sites (`forum.example.org`, `shop.example.com`); the local
  cluster's own names (`erp.example.com`); each edge member's link name (`edge1.example.com`, `edge2.example.com`),
  which the front passes through to that member alone; the MX of each mail domain.
- **At each edge member's own address**: the edge cluster's own name (`edge.example.com`, its `webapp_http_host`, an
  address record per member), its `turn_public_host`, its `smtp_hostname` (with reverse DNS for the address), and the
  SIP SRV records (`_sip._udp`, `_sip._tcp`, `_sips._tcp`) naming every member; with `front_edge` members the XMPP SRV
  records too (with `edge` members they name the front, whose ports take each local node's domain).
- **Mail.** SPF for each mail domain lists every edge member's address (`ip4:`): mail out leaves from them, and the
  wizard's `v=spf1 mx` would name the front. The MX name's certificate goes on every edge member, where STARTTLS ends.
  The wizard checks MX against the member's `smtp_hostname`, so set these records by hand. MTA-STS for the domains
  keeps senders on TLS through the front.

## Commands

```sh
./moq archive --release --no-data --conf conf/full-stack/MoquiFrontConf.xml       # the front node
./moq component-get edge
./moq archive --release --no-data --conf conf/full-stack/MoquiEdgeConf.xml        # each edge member; or --enterprise
./moq component-get mr-boss
./moq archive --enterprise --no-data --conf conf/full-stack/MoquiLocalConf.xml    # each local member
```

The order to pair, each tier with the one before it:

1. **The front.** Start it once, so it generates its secrets, and give it a certificate for its own name.
   An edge member's first certificate cannot come through the front (with none it starts no link server), so in
   step 2 give the edge cluster's own name a certificate first: System › Certificates with ACME at a member, the name
   at the members' own addresses with 443 open, or a copied one in `cert/`. The rows are shared, so every member
   writes it.
2. **The edge cluster with the front.** Start the first edge member once with the shared database settings, copy its
   `.env` to each other member, add each member's own lines, and start them (as in `../edge-cluster/`). On the front,
   System › Edge Clients › Clients, make a pairing code; on one edge member, System › Edge Node, Pair with a front node.
   Copy the `front_node_*` lines it wrote to every other edge member's `.env`, and restart them. The front's Clients
   shows each edge member linked; Domains shows each one's link name. On the front's Ports add port 25, kind SMTP, for
   the edge cluster's pairing.
3. **The local cluster with the edge cluster.** Start the first local member once. On any edge member, System › Edge
   Clients › Clients, make a pairing code; on the first local member, System › Edge Node, pair at the edge cluster's
   own name (`https://edge.example.com`), or at an edge member's link name once it has a certificate, and run the
   wizard: host names, mail, chat, calls, phones, and PASS with the
   sites the edge members serve. Copy its `.env` to each other local member, add its own `cluster_base_url`, and start
   it.
4. **With `edge` members only, the local cluster with the front.** On the front make another pairing code; on the
   first local member pair with the front, copy the `front_node_*` lines to every local member, and run the wizard
   again: the names go to the front.
5. **The sites.** On the authority (a local member) create each site and bind its hosts and mounts, the same hosts as
   the edge members' `public-site` bindings. Their hosts reach the front from the edge members, as terminate hosts.
6. **Check.** The front's Edge Clients: the edge cluster's pairing with every member linked; Domains: the sites, the
   local cluster's names and the link names. An edge member's Edge Clients › Edge cluster: every member live, with its
   link address, and the front it is paired with. A local member's System › Edge Node: a link to each edge member.
