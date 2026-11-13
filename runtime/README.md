# runtime

The runtime directory holds everything that varies per instance. The binary finds it through `MOQUI_RUNTIME`
(default `runtime` in the working directory when it has `components/base/`, else the working directory itself when it
has one, else a new `runtime` in the working directory; never from where the binary is, so one binary runs several
instances). `./moq archive` zips this directory, without `log/`, `tmp/`, `save/` and
`install/`, with the binary and `MoquiConf.xml` in it.

| Path | In git | Purpose |
|---|---|---|
| `components/` | partly | The component sets, each a directory of components; `components/components.xml` lists the sets that load, in order, and only those load ([plans/archive/Components-Dir.md](../plans/archive/Components-Dir.md)) |
| `components/base/` | yes | The framework and base components that ship with Moqui |
| `components/addon/` | no | Add-on components, normally checked out by `./moq component-get` or deployed from System > Components ([components/addon/README.md](components/addon/README.md)) |
| `components/dev/` | partly | Components that load only when `instance_purpose` is `dev`: the framework's (`dev-aide`, `example`), the demo lock-down (off by default, loaded on a demo server by `conf/MoquiDemoConf.xml`) and the component template in git, a developer's own beside them ([components/dev/README.md](components/dev/README.md)) |
| `components/test/` | no | Test-only add-on components, loaded on a dev or test instance, never on production ([components/test/README.md](components/test/README.md)) |
| `conf/` | yes | Runtime conf files (`MoquiDevConf.xml`, `MoquiProductionConf.xml`, ...) merged over `MoquiDefaultConf.xml` |
| `cert/` | no | HTTPS certificates by domain (nginx-proxy, certbot or mkcert layouts; `docs/Run-Deploy.md`). Never commit keys. |
| `db/` | no | Embedded database cluster and related files |
| `log/` | no | Log files |
| `opensearch/` | no | Managed OpenSearch install and data |
| `chromium/` | no | Chromium from `./moq chromium-download`, for PDF when `moqui_chromium_path` is not set |
| `tmp/` | no | Scratch files |
| `txlog/` | no | Transaction logs |
| `.env` | no | Generated secrets and local overrides |
| `install/` | no | The embedded Postgres, unpacked from the binary on the first start (`MOQUI_INSTALL` moves it) |
| `MoquiConf.xml` | no | The instance conf, merged over `MoquiDefaultConf.xml` when `MOQUI_CONF` is not set |

Directories marked "no" are listed in `.gitignore` and are created when first needed.

The binary carries a copy of `components/base/framework`, `components/base/tools` and a webroot stub
(`crates/moqui-embedded`). At startup it logs each file of those here that differs from that copy, and uses the files
here. With no runtime it writes the copy as a skeleton runtime, which boots, answers `/rest`, `/Login`, `/status` and
the html `/apps`, and runs `moqui tui` ([Run-Deploy](../docs/Run-Deploy.md#built-in-framework-files)).
