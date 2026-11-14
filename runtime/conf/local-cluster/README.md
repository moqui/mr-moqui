# A local cluster behind your own load balancer

Several members share one external database and run behind a load balancer, for more capacity or a member that can
fail. Any member serves any request: sessions are rows in the shared database. Requirements, what is shared and what
is not: [Run-Deploy](../../../docs/Run-Deploy.md#cluster). A cluster pooled and balanced by an edge node is not built
yet ([plans/Edge-Node2.md](../../../plans/Edge-Node2.md), E17).

```
                   clients
                      | https
               load balancer             webapp_trusted_proxies on each member
           +----------+----------+
       member 1   member 2   member 3    MoquiLocalConf.xml; 8444 between members only (cluster_base_url)
           +----------+----------+
              shared Postgres            entity_ds_db_conf=postgres
```

| Node | Conf | Package and runtime |
|---|---|---|
| Each member | `MoquiLocalConf.xml` | Enterprise (`--enterprise`), the same runtime on each |

## `.env`

- **Every member, the same**: `entity_ds_host`, `entity_ds_port`, `entity_ds_database`, `entity_ds_user`,
  `entity_ds_password`, `webapp_trusted_proxies`, `webapp_client_ip_header`; and the secrets the first member
  generates, `session_cookie_key`, `cluster_tls_cert`, `cluster_tls_key` and `entity_ds_crypt_pass`.
- **Each member, its own**: `cluster_base_url=https://<this member's private address>:8444`.

## Commands

```sh
./moq archive --enterprise --no-data --conf conf/local-cluster/MoquiLocalConf.xml
```

1. Start the first member once, so it generates its secrets into `.env`.
2. Copy that `.env` to each other member, add its own `cluster_base_url`, and start it.
3. Open 8444 between the members only.
4. Check: `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database.
