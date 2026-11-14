# PASS servers with a BOSS authority

Public machines serve mr-pass's sites (Forum, Account, Members, Sites) and keep no business data: each call goes to
the authority, a BOSS server or cluster with Mantle and the business database, over REST or gRPC. Steps, scaling and
limits: [Run-Deploy](../../../docs/Run-Deploy.md#pass-servers-with-a-boss-server-or-cluster); mail, media, workers and
imports: mr-pass's `docs/Deployment.md`.

```
 visitors --https--> PASS server forum.example.com --+
                     MoquiPassConf.xml                |  REST /rest/s1/pass/v1/sites/*, or gRPC
 visitors --https--> PASS server shop.example.com ---+--------> authority boss.example.com
                     MoquiPassConf.xml                          MoquiAuthorityConf.xml: mr-boss and mr-mantle
                                                                one server, or a cluster (../local-cluster/)
```

| Node | Conf | Package and runtime |
|---|---|---|
| PASS server | `MoquiPassConf.xml` | Any server package, Lite is the smallest; base components and mr-pass, no Mantle |
| Authority | `MoquiAuthorityConf.xml` | Maximal or Enterprise; mr-boss, which brings mr-mantle |

Change the example names in both confs (the host, site id, client id and callback) to yours. A PASS server served by
an edge node is the edge node itself, with `pass_backend_url=edge:` ([../edge/README.md](../edge/README.md)).

## `.env`

- **PASS server**: `pass_client_secret`; `pass_backend_url` and `pass_client_id` when they differ from the conf's.
  The certificate for its public host goes in `cert/`.
- **Authority**: `pass_client_<id>_secret` for each client, the same value as that PASS server's `pass_client_secret`.

## Commands

```sh
# the authority
./moq component-get mr-boss
./moq run-production --release -c conf/pass/MoquiAuthorityConf.xml

# a PASS server, from a checkout without Mantle (a conf cannot remove a component in runtime/components/addon)
git clone https://github.com/moqui/mr-moqui.git pass && cd pass
./moq component-get mr-pass
./moq archive --lite --no-data --conf conf/pass/MoquiPassConf.xml
```

On the authority, create each site and bind its host and mounts (`mantle.public.SiteServices.create#Site`,
`store#Mounts`), then check `https://boss.example.com/rest/s1/pass/v1/sites/<siteId>/readiness` signed in as an
operator.
