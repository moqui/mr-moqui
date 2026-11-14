# A local node with an edge node

A server in the office or at home holds the data and dials a small cloud instance, the edge node, which gives it a
public presence: web access, SSH, mail, chat webhooks, mr-pass's sites, calls with TURN, SIP trunks and outside phones,
and XMPP clients. The local node needs no open port, port forward or fixed address. The desktop and mobile apps are
clients of the local node. Steps and limits: [Run-Deploy](../../../docs/Run-Deploy.md#a-local-server-with-an-edge-node).

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, edge_ssh_tunnel_port
 edge node      edge.example.com    MoquiEdgeConf.xml    base components and mr-pass; the link's queue and copies
    ^  the local node dials 443 and keeps one link open
 local node     office or home      MoquiLocalConf.xml   the apps and the data
```

| Node | Conf | Package and runtime |
|---|---|---|
| Edge node | `MoquiEdgeConf.xml` | Maximal (or Enterprise with an external database); base components and mr-pass (`./moq component-get edge`) |
| Local node | `MoquiLocalConf.xml` (any production conf works) | Maximal or Enterprise, with its apps |

## `.env`

- **Edge node**: `webapp_http_host`, `edge_public_addresses`; for mail `smtp_listen_port=25`, `smtp_listen_host`,
  `smtp_hostname`, `edge_mail_bounce_domain`; for calls `rtc_public_address`; for phones `sip_port`, `sip_tls_port`,
  `sip_host`, `sip_public_address`, `sip_registrar_enabled`; for XMPP `edge_xmpp_port`, `edge_xmpp_tls_port`; for SSH
  `edge_ssh_tunnel_port`; for PASS `pass_backend_web_url`. Generated at the first start: `edge_link_key`, and
  `mail_dkim_key` on first use. A WebPKI certificate goes in `cert/`.
- **Local node**: `webapp_http_host`, and a certificate in `cert/` for each passthrough name. Pairing writes
  `edge_node_url`, `edge_node_key`, `edge_node_pin`, `edge_node_id` and `edge_node_pairing` (copy this `.env` to each member of a local cluster, which then links too); the wizard writes the rest.

Each conf's comments say what each setting does.

## Commands

```sh
# the edge node, from a checkout with only the edge set's components
./moq component-get edge
./moq run-production --release -c conf/edge/MoquiEdgeConf.xml
./moq archive --release --no-data --conf conf/edge/MoquiEdgeConf.xml   # or an archive for the host

# the local node
./moq run-production --release -c conf/edge/MoquiLocalConf.xml
```

Then on the edge node, System › Edge Clients: make a pairing code. On the local node, System › Edge Node: **Pair with
an edge node**, and follow the wizard.
