# An edge cluster

Several edge nodes on one external database, for an edge node that can fail or more capacity at the edge. Each local
node pairs once, with any member, then links to every live member at its own address and takes work from each; the
queue, the pairings and the host names are rows the members share. A clustered all-in-one server is the same, with
`../all-in-one/MoquiAllInOneConf.xml` and an external database on each member. Steps and limits:
[Run-Deploy](../../../docs/Run-Deploy.md#an-edge-cluster).

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, tunnel ports       an A, MX and SRV record for each member
 edge 1   edge1.example.com   edge 2   edge2.example.com      MoquiEdgeConf.xml; 8444 between members only
    +-------------+-------------+
          shared Postgres                                     the queue, pairings, host names, copies
    ^  each local node dials every member and keeps a link to each
 local node, or each member of a local cluster                ../edge/MoquiLocalConf.xml or ../local-cluster/MoquiPooledConf.xml
```

| Node | Conf | Package and runtime |
|---|---|---|
| Each edge member | `MoquiEdgeConf.xml` | Maximal with an external database, or Enterprise; base components and mr-pass (`./moq component-get edge`), the same on each |
| A local node | [`../edge/MoquiLocalConf.xml`](../edge/README.md): nothing differs, it learns the members from its links | Maximal or Enterprise, with its apps |
| Each member of a local cluster | [`../local-cluster/MoquiPooledConf.xml`](../local-cluster/README.md) | Enterprise (`--enterprise`) |

## `.env`

- **Every edge member, the same** (the first member's, copied before each other one's first start):
  `entity_ds_host`, `entity_ds_port`, `entity_ds_database`, `entity_ds_user`, `entity_ds_password`; the secrets the first
  start generates, `session_cookie_key`, `cluster_tls_cert`, `cluster_tls_key`, `entity_ds_crypt_pass`,
  `edge_link_key` (the one link key local nodes pin), `turn_auth_secret`, `acme_account_key` and, once made,
  `mail_dkim_key`; `webapp_http_host`, `edge_public_addresses` (every member's addresses), `edge_mail_bounce_domain`,
  `smtp_listen_port=25`, `smtp_listen_host`, `sip_port`, `sip_tls_port`, `sip_host`, `sip_registrar_enabled`,
  `edge_xmpp_port`, `edge_xmpp_tls_port`, `edge_xmpp_host`, `edge_ssh_tunnel_port`, `pass_backend_web_url`. A WebPKI
  certificate in `cert/` for the names local nodes pair at; the links present the pinned link key, not it.
- **Each edge member, its own**: `cluster_base_url=https://<private address>:8444`, `edge_link_url=https://<its own
  public name>`, `smtp_hostname` (its MX name), `rtc_public_address`, `turn_public_host`, `sip_public_address`. Never
  copy `runtime/db`: each member keeps its own instance id there.
- **A local node**: as with one edge node. Pairing writes `edge_node_url`, `edge_node_key`, `edge_node_pin`,
  `edge_node_id` and `edge_node_pairing`; the list of edge members is kept in `runtime/db/edge_members`.

## DNS

With no front node before the cluster, for each member: an A (and AAAA) record under each public name a local node
uses (the cluster's `webapp_http_host` and each host name), an A record for its own `edge_link_url` name, an MX record
for each mail domain (the same preference on each) with reverse DNS naming its `smtp_hostname`, and SRV records for
SIP (`_sip._udp`, `_sip._tcp`, `_sips._tcp`) and XMPP (`_xmpp-client._tcp`, `_xmpps-client._tcp`).

## Commands

```sh
./moq component-get edge
./moq archive --release --no-data --conf conf/edge-cluster/MoquiEdgeConf.xml   # Maximal; or --enterprise
```

1. Start the first member once with the shared database settings in `.env`, so it generates its secrets.
2. Copy that `.env` to each other member, add its own lines, set `edge_link_url` on the first one too, and start
   them. Open 8444 between the members only.
3. Check System › Edge Clients › Edge cluster on any member: every member live, each with its link address.
4. Pair each local node as with one edge node (System › Edge Clients › Clients for the code, System › Edge Node on the
   local node). Its System › Edge Node then lists a link to each edge member, and which one carries its work up.
