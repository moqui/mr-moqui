# A local node with a standby

A local node paired with an edge node, and a second machine that is started when the first one dies. The standby has
the same runtime and `.env`, and the data: the same external database, a replica that is promoted, or the last backup
restored. It links with the pairing's key and takes over every name of the pairing; the edge node tells its
administrators. Nothing starts the standby by itself: a person does, or automation the operator adds. Steps and
limits: [Run-Deploy](../../../docs/Run-Deploy.md#a-local-node-with-a-standby).

```
 visitors, mail servers, providers
    |  443, 25, ...
 edge node        ../edge/MoquiEdgeConf.xml   holds the queue and mail while no machine is linked
    ^  one machine at a time dials 443 and keeps the link
 first machine    MoquiLocalConf.xml          the apps and the data
 standby          MoquiLocalConf.xml          stopped; started when the first machine dies
```

| Node | Conf | Package and runtime |
|---|---|---|
| The first machine and the standby | `MoquiLocalConf.xml` | Maximal or Enterprise, the same runtime on each |
| The edge node | [`../edge/MoquiEdgeConf.xml`](../edge/README.md) | Maximal (or Enterprise with an external database) |

## `.env`

- **Both machines, the same**: the first machine's whole `.env`, pairing's lines (`edge_node_url`, `edge_node_key`,
  `edge_node_pin`, `edge_node_id`, `edge_node_pairing`) and `entity_ds_crypt_pass` among them. Copy it again after the
  wizard or a setting changes.
- **Each machine, its own**: `runtime/db/instance_id` and `runtime/db/edge_member_key`. Never copy `runtime/db`.

## Commands

```sh
./moq archive --release --no-data --conf conf/local-failover/MoquiLocalConf.xml   # one archive for both machines
```

1. Pair the first machine with its edge node and run the wizard (System › Edge Node).
2. Copy its `.env` to the standby. Keep the data ready: the database's address, a replica, or the mirrored backups.
3. When the first machine dies: promote the replica or restore the last backup (`moqui load-save`), then start the
   standby. On the edge node, System › Edge Clients › Clients shows the takeover. After a restore, use **Deliver
   again** there, since the backup's time.
4. Keep the first machine stopped: started again, it is refused until the operator lets it link again on Takeovers.
