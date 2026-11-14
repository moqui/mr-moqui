# A local cluster

Several members share one external database, for more capacity or a member that can fail. Any member serves any
request: sessions are rows in the shared database. In front of them is a load balancer of your own, or an edge node,
which pools the members of a pairing and balances over them. Requirements, what is shared and what is not:
[Run-Deploy](../../../docs/Run-Deploy.md#cluster); with an edge node:
[Run-Deploy](../../../docs/Run-Deploy.md#a-local-cluster-with-an-edge-node).

```
        behind your own load balancer                 with an edge node
                   clients                    visitors, mail servers, providers
                      | https                             |  443, 25, ...
               load balancer                  edge node      ../edge/MoquiEdgeConf.xml
           +----------+----------+                ^  each member dials 443 and keeps its own link
       member 1   member 2   member 3         member 1   member 2   member 3     MoquiPooledConf.xml
           +----------+----------+                +----------+----------+
              shared Postgres                        shared Postgres
```

| Node | Conf | Package and runtime |
|---|---|---|
| Each member, behind your own load balancer | `MoquiLocalConf.xml` | Enterprise (`--enterprise`), the same runtime on each |
| Each member, with an edge node | `MoquiPooledConf.xml` | Enterprise (`--enterprise`), the same runtime on each |
| The edge node | [`../edge/MoquiEdgeConf.xml`](../edge/README.md) | Maximal (or Enterprise with an external database) |

## `.env`

- **Every member, the same**: `entity_ds_host`, `entity_ds_port`, `entity_ds_database`, `entity_ds_user`,
  `entity_ds_password`; the secrets the first member generates, `session_cookie_key`, `cluster_tls_cert`,
  `cluster_tls_key` and `entity_ds_crypt_pass`. Behind your own load balancer: `webapp_trusted_proxies`,
  `webapp_client_ip_header`. With an edge node: what pairing writes on the first member, `edge_node_url`,
  `edge_node_key`, `edge_node_pin`, `edge_node_id` and `edge_node_pairing`.
- **Each member, its own**: `cluster_base_url=https://<this member's private address>:8444`. Never copy `runtime/db`:
  each member keeps its own instance id and edge member key there.

## Commands

```sh
./moq archive --enterprise --no-data --conf conf/local-cluster/MoquiLocalConf.xml    # or MoquiPooledConf.xml
```

1. Start the first member once, so it generates its secrets into `.env`.
2. With an edge node: on the edge node, System › Edge Clients › Clients, make a pairing code; on the first member,
   System › Edge Node, pair and run the wizard.
3. Copy that `.env` to each other member, add its own `cluster_base_url`, and start it. With an edge node each member
   links once its cluster lists it as live.
4. Open 8444 between the members only.
5. Check: `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database. With an edge
   node, Edge Clients › Clients there lists each member, and System › Edge Node on a member lists its cluster.
