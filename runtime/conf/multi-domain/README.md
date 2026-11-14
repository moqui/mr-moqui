# Several local nodes on one edge node

One edge node in front of several local nodes, such as branch offices or the separate organizations of one group,
each on its own database with its own domains: host names, mail domains, XMPP domain, SIP realm and numbers, chat
bridges and PASS sites. Each local node pairs as it would alone, and the edge node routes each name to the node that
claimed it first. Isolation is reasonable, not multi-tenant: one operator, related organizations. Steps and limits:
[Run-Deploy](../../../docs/Run-Deploy.md#several-local-nodes-on-one-edge-node).

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, a tunnel port per node for SSH
 edge node      edge.example.com    MoquiEdgeConf.xml    the links' queues and copies, one pairing per local node
    ^  each local node dials 443 and keeps its own link open
 local node A   branch-a            MoquiLocalConf.xml   its apps and data; erp.branch-a.example.com, branch-a.example.com
 local node B   branch-b            MoquiLocalConf.xml   its apps and data; erp.branch-b.example.com, branch-b.example.com
```

| Node | Conf | Package and runtime |
|---|---|---|
| Edge node | `MoquiEdgeConf.xml` | Maximal (or Enterprise with an external database); base components and mr-pass (`./moq component-get edge`) |
| Each local node | `MoquiLocalConf.xml` (any production conf works) | Maximal or Enterprise, with its apps |

## `.env`

- **Edge node**: as for one local node ([../edge/README.md](../edge/README.md)): `webapp_http_host`,
  `edge_public_addresses`, the mail, calls, SIP and XMPP settings, and a WebPKI certificate in `cert/` for its own
  name and each terminate host. SSH ports per node are rows (System › Edge Clients › Ports), not `.env`.
- **Each local node**: `webapp_http_host`, `xmpp_domain` and `sip_domain` of its own, and a certificate in `cert/` for
  each passthrough name. Pairing and the wizard write the rest.

Each conf's comments say what each setting does.

## Commands

```sh
# the edge node, from a checkout with only the edge set's components
./moq component-get edge
./moq run-production --release -c conf/multi-domain/MoquiEdgeConf.xml

# each local node
./moq run-production --release -c conf/multi-domain/MoquiLocalConf.xml
```

Then on the edge node, System › Edge Clients › Clients: make a pairing code for each local node. On each local node,
System › Edge Node: **Pair with an edge node**, and follow the wizard. Edge Clients › Domains shows what each one
claims, and any name two of them want.
