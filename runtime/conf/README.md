# runtime/conf

Runtime confs, each merged over `components/base/framework/conf/MoquiDefaultConf.xml` and the components'
`MoquiConf.xml`. One is used at a time: the file `MOQUI_CONF` names (`./moq run -c FILE`, `moqui --conf FILE`),
relative to the runtime directory, else `runtime/MoquiConf.xml`, else `conf/MoquiDevConf.xml`
([Run-Deploy](../../docs/Run-Deploy.md#runtime-directory-and-conf)). Per-host settings go in `runtime/.env`.

The confs of one node are here:

| File | For |
|---|---|
| `MoquiDevConf.xml` | Development (`./moq run`) |
| `MoquiProductionConf.xml` | One production server (`./moq run-production`, and an archive's `MoquiConf.xml`) |
| `MoquiDemoConf.xml` | A public demo server |
| `MoquiLoadTestConf.xml` | Load tests |

Each other topology has a directory, with a conf for each kind of node and a README with the diagram, the `.env`
names and the commands ([Run-Deploy, Choose a topology](../../docs/Run-Deploy.md#choose-a-topology)):

| Directory | Topology |
|---|---|
| [`pass/`](pass/README.md) | PASS servers with a BOSS authority |
| [`dev-group/`](dev-group/README.md) | Dev instances with a dev group server |
| [`edge/`](edge/README.md) | A local node with an edge node |
| [`all-in-one/`](all-in-one/README.md) | A local node with an all-in-one node |
| [`multi-domain/`](multi-domain/README.md) | Several local nodes on one edge node |
| [`local-cluster/`](local-cluster/README.md) | A local cluster, behind your own load balancer or an edge node |
| [`local-failover/`](local-failover/README.md) | A local node with a standby |

`crates/moqui-devkit/src/runtime_conf_tests.rs` checks that every conf here, in the directories too, parses, merges
over the default conf and passes the schema.
