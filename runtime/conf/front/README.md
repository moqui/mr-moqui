# A front node before a local cluster

A front node balances a local cluster and protects it, with none of an edge node's comms: web access by host name,
load balancing with sticky sessions and health, web protection, TLS and certificates, and TCP ports. The cluster's
members pair with it and dial it, so they need no load balancer of your own and no open port. The cluster keeps its
own mail, chat, calls and phones, or has none. The same front node before an edge cluster is the full stack
([../full-stack/README.md](../full-stack/README.md)). Steps and limits:
[Run-Deploy](../../../docs/Run-Deploy.md#a-front-node).

```
 visitors (and mail servers, with an smtp port)
    |  443 (25, 5222/5223, an SSH port)
 front node   front.example.com   MoquiFrontConf.xml    pairings, host names, ports, certificates; no data
    ^  each member dials 443 and keeps its own link open
 member 1   member 2   member 3                         MoquiLocalConf.xml; 8444 between members only
    +----------+----------+
       shared Postgres                                   the apps and the data
 calls and phones: each member's own address, never through the front
```

| Node | Conf | Package and runtime |
|---|---|---|
| Front node | `MoquiFrontConf.xml` | Maximal (embedded Postgres is enough), or Enterprise with an external database; base components only |
| Each member of the local cluster | `MoquiLocalConf.xml` | Enterprise (`--enterprise`), the same runtime on each, with its apps |
| One local node, not a cluster | [`../edge/MoquiLocalConf.xml`](../edge/README.md) or any production conf: pairing with a front writes `front_node_*` | Maximal or Enterprise |

## `.env`

- **Front node**: `webapp_http_host` (its own name, where nodes pair), `edge_public_addresses` (its public addresses),
  and, made at its first start, `session_cookie_key`, `entity_ds_crypt_pass`, `edge_link_key` (the key the nodes
  behind it pin) and `acme_account_key`. A WebPKI certificate in `cert/` for its own name and each terminate host, or
  System › Certificates with ACME. Ports: `edge_ssh_tunnel_port`, `edge_xmpp_port` and `edge_xmpp_tls_port` when used.
- **Every member, the same**: `entity_ds_host`, `entity_ds_port`, `entity_ds_database`, `entity_ds_user`,
  `entity_ds_password`; the secrets the first member generates, `session_cookie_key`, `cluster_tls_cert`,
  `cluster_tls_key` and `entity_ds_crypt_pass`; and what pairing writes on the first member: `front_node_url`,
  `front_node_key`, `front_node_pin` and `front_node_id`. A certificate in `cert/` for each passthrough name.
- **Each member, its own**: `cluster_base_url=https://<this member's private address>:8444`. Never copy `runtime/db`:
  each member keeps its own instance id, member key and the front's member list there.

## DNS

The front's own name, and each of the cluster's host names, with an A (and AAAA) record at the front's addresses. With
port 25 on the front for the cluster's mail: an MX record at the front's name, SPF for wherever the cluster's mail
out leaves from, and the MX name's certificate on every member, where STARTTLS ends. With XMPP through the front: the
XMPP SRV records (`_xmpp-client._tcp`, `_xmpps-client._tcp`) at the front's name and ports. Calls and phones need records at the
members' own addresses (TURN, SIP SRV), not the front's.

## Commands

```sh
./moq archive --release --no-data --conf conf/front/MoquiFrontConf.xml          # the front node: Maximal
./moq archive --enterprise --no-data --conf conf/front/MoquiLocalConf.xml       # the members
```

1. Start the front node once, so it generates its secrets, and give it a certificate for its own name.
2. Start the first member once, so it generates its secrets.
3. On the front, System › Edge Clients › Clients, make a pairing code; on the first member, System › Edge Node, pair
   with the front's address (the Pair screen shows its role, front) and run the wizard: host names, and XMPP if used.
4. Copy the first member's whole `.env` to each other member, add its own `cluster_base_url`, and start it. Each links
   once its cluster lists it as live.
5. For mail in through the front: on the front, System › Edge Clients › Ports, add port 25 of kind SMTP for the
   cluster's pairing, and run the SMTP listener on every member in internet mode. The front passes it as it does to an
   edge cluster's members, but this path to a local cluster has no test yet: check a message end to end before
   relying on it.
6. Check Edge Clients › Clients on the front: each member linked and live; Domains: the host names. On a member,
   System › Edge Node shows the front's link and its cluster.
