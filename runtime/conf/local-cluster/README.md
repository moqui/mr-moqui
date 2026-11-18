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

- **Every member, the same**: the transactional datasource's `host`, `port`, `database`, `user`
  and `password` (`moqui_conf__entity_facade__datasource__TRANSACTIONAL__host` and the rest); the secrets the first member generates, `webapp-list/webapp[webroot]/session-config@cookie-key`, `cluster-member@tls-cert`,
  `cluster-member@tls-key`, `moqui_conf__entity_facade__crypt_pass`, and for git hosting `ssh-server@host-key` and `git-server@lfs-token-key`: a client's
  SSH connections and LFS requests reach any member, and another one after a failover (a member whose SSH host key
  differs warns at startup). Copying the first member's whole `.env` gives each the same. Behind your own load balancer: `webapp-list/webapp[webroot]@trusted-proxies`,
  `webapp-list/webapp[webroot]@client-ip-header`. With an edge node: what pairing writes on the first member, `edge/node@url`,
  `edge/node@key`, `edge/node@pin`, `edge/node@id` and `edge/node@pairing`.
- **Each member, its own**: `moqui_conf__cluster_member__base_url=https://<this member's private address>:8444`. Never copy `runtime/db`:
  each member keeps its own instance id and edge member key there.

## Commands

```sh
./moq archive --enterprise --no-data --conf conf/local-cluster/MoquiLocalConf.xml    # or MoquiPooledConf.xml
```

1. Start the first member once, so it generates its secrets into `.env`.
2. With an edge node: on the edge node, System › Edge Clients › Clients, make a pairing code; on the first member,
   System › Edge Node, pair and run the wizard.
3. Copy that `.env` to each other member, add its own `cluster-member@base-url`, and start it. With an edge node each member
   links once its cluster lists it as live.
4. Open 8444 between the members only.
5. Check: `SELECT instance_id, cluster-member@base-url, heartbeat FROM cluster_member;` on the shared database. With an edge
   node, Edge Clients › Clients there lists each member, and System › Edge Node on a member lists its cluster.
