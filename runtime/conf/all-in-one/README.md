# A local node with an all-in-one node

A full public server, with its own users, apps, data, mail domains, chat bridges, calls and XMPP domain, that is also
the edge node of one local node, such as a desktop or office node. No second cloud instance is needed. The local node
pairs and runs the wizard as with a dedicated edge node, and sees no difference. Each item has one owner; a name,
address, number or bridge both sides claim is refused. Steps and limits:
[Run-Deploy](../../../docs/Run-Deploy.md#an-all-in-one-node).

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223 (its own XMPP), the local node's XMPP tunnel ports
 all-in-one node   www.example.com   MoquiAllInOneConf.xml       its own apps and data; the local node's queue and copies
    ^  the local node dials 443 and keeps one link open
 local node        office or home    ../edge/MoquiLocalConf.xml  the apps and the data
```

| Node | Conf | Package and runtime |
|---|---|---|
| All-in-one node | `MoquiAllInOneConf.xml` | Maximal; the server's full runtime |
| Local node | `../edge/MoquiLocalConf.xml`, as with a dedicated edge node (the desktop app has its own) | Maximal or Enterprise, with its apps |

## `.env`

- **All-in-one node**: what an edge node needs ([../edge/README.md](../edge/README.md)): `webapp_http_host`,
  `edge_public_addresses`, the mail, calls and SIP settings, and a WebPKI certificate in `cert/`. Tunnel ports of the
  local node's XMPP domain that differ from the server's own `xmpp_port` and `xmpp_tls_port`, such as
  `edge_xmpp_port=5242` and `edge_xmpp_tls_port=5243`.
- **Local node**: as with an edge node; pairing and the wizard write the rest.

## Commands

```sh
./moq run-production --release -c conf/all-in-one/MoquiAllInOneConf.xml
./moq archive --release --no-data --conf conf/all-in-one/MoquiAllInOneConf.xml   # or an archive for the host
```

Then on the all-in-one node, System › Edge Clients: make a pairing code. On the local node, System › Edge Node:
**Pair with an edge node**.
