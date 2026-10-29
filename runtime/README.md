# runtime

The runtime directory holds everything that varies per instance. The binary finds it through `MOQUI_RUNTIME`
(default `runtime` in the working directory, else `runtime` beside the binary, else the binary's own directory: the
first with `base-component/` in it). `./moq archive` zips this directory, without `log/`, `tmp/`, `save/` and
`install/`, with the binary and `MoquiConf.xml` in it.

| Path | In git | Purpose |
|---|---|---|
| `base-component/` | yes | The framework and base components that ship with Moqui |
| `component/` | no | Additional components, normally checked out by `./moq` |
| `dev-component/` | partly | Components that load only when `instance_purpose` is `dev`: the framework's (`dev-aide`, `example`) and the component template in git, a developer's own beside them ([dev-component/README.md](dev-component/README.md)) |
| `conf/` | yes | Runtime conf files (`MoquiDevConf.xml`, `MoquiProductionConf.xml`, ...) merged over `MoquiDefaultConf.xml` |
| `cert/` | no | HTTPS certificates by domain (nginx-proxy, certbot or mkcert layouts; `docs/Run-Deploy.md`). Never commit keys. |
| `db/` | no | Embedded database cluster and related files |
| `log/` | no | Log files |
| `opensearch/` | no | Managed OpenSearch install and data |
| `chromium/` | no | Chromium from `./moq chromium-download`, for PDF when `pdf_chromium_path` is not set |
| `tmp/` | no | Scratch files |
| `txlog/` | no | Transaction logs |
| `.env` | no | Generated secrets and local overrides |
| `install/` | no | The embedded Postgres, unpacked from the binary on the first start (`MOQUI_INSTALL` moves it) |
| `MoquiConf.xml` | no | The instance conf, merged over `MoquiDefaultConf.xml` when `MOQUI_CONF` is not set |

Directories marked "no" are listed in `.gitignore` and are created when first needed.
