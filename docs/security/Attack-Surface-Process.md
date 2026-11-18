# Attack surface: processes and build

Processes the server starts, the startup port check, and the build features that decide what is compiled in. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Child processes

Each row is a program the server starts. A program named without a path (`git`, `gcc`, `ldconfig`, `lsof`, `su`, `id`, `kill`, `pkill`) is found through the server's `PATH`. Unless a row says so, the child gets the server's whole environment, with any secret that was given to the server as an environment variable.

| Process | Code | Program and arguments | Environment and input | Limits |
|---|---|---|---|---|
| Headless Chromium for PDF | `crates/moqui-host/src/pdf.rs` `ChromiumProcess::spawn`; `crates/moqui-host/src/chromium.rs` `launch_args` | The binary of `screen-facade/pdf` `chromium-path`, or one that is found. Arguments: `--proxy-server=127.0.0.1:9`, `--proxy-bypass-list=<-loopback>`, `--force-webrtc-ip-handling-policy=disable_non_proxied_udp`, `--headless=new`, `--remote-debugging-pipe` (on Unix, started through `/bin/sh -c 'exec ...'` that gives it the server's pipes as its descriptors 3 and 4; elsewhere `--remote-debugging-address=127.0.0.1` and `--remote-debugging-port=0`), `--disable-extensions`, `--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--user-data-dir=<runtime>/tmp/chromium-<pid>-<n>`, `about:blank`. No server crate passes `--no-sandbox` | A named environment, never the server's: `moqui_lib::env::limit_child_env` passes only `CHILD_ENV_NAMES` (`PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR` and the Windows start-up names) and `pdf.rs` `CHROMIUM_ENV` (XDG, fontconfig, display, D-Bus) | Its own process group; the group is killed and the profile directory removed when the browser is dropped. Page and time limits come from `screen-facade/pdf` `page-limit` and `timeout` |
| OpenSearch | `crates/moqui-host/src/search.rs` `opensearch_command`, `switchable_owner`, `stop_child` | `./bin/opensearch` with its arguments. When the install directory is owned by the user `opensearch`, the server runs as another user, and `/bin/su -c /bin/true opensearch` succeeds, the start is `su -c "./bin/opensearch '<arg>' ..." opensearch`, each argument in single quotes. `id -u opensearch` and `id -u` are run for that test | The named environment of `limit_child_env`, with `JAVA_HOME`, `OPENSEARCH_*` and `DISABLE_*` | A stop sends SIGTERM to the process group, and after 30 seconds runs `pkill -KILL -P <pid>` and sends SIGKILL to the group |
| Embedded Postgres tools | `crates/moqui-drivers/src/embedded.rs` `stop_postmaster`, `run_tool`; `crates/moqui-drivers/src/backup.rs` `run_bounded` | `pg_ctl stop -m <mode> -D <data directory>`, and `pg_dump`, `pg_restore` and other tools from the install's `bin` | The database password is in the child's environment as `PGPASSWORD`, not in its arguments | `run_bounded` has a time limit and kills the process group |
| libxml2 shim build | `crates/moqui-drivers/src/xmlshim.rs` | `ldconfig -p`; `gcc -shared -fPIC -o <shim dir>/libxml2.so.2 shim.s -Wl,-soname,libxml2.so.2 -Wl,--version-script=<map> <host library>`. Linux x86_64 only | The assembly and version script are written by the server | |
| External agent bridge | `crates/moqui-core/src/service/external_agent.rs` `request`, `invoke` | The argument list in the environment variable `MOQUI_EXTERNAL_AGENT_COMMAND` (a JSON array). No shell | The service's parameters go to the child as JSON on its standard input | 90 seconds; request and answer sizes are capped (8 MiB each). The child gets the named environment of `limit_child_env` and the names `MOQUI_EXTERNAL_AGENT_ENV` lists (comma separated), never the server's (plans/Security-Review.md SEC-AI-12, SEC-OUT-09, fixed), and the server's working directory. With the variable unset the service answers a failure. No service definition in the tree names this service, so nothing calls it until a component defines one |
| Registry token command | `crates/moqui-core/src/service/registry_token.rs` `run_first_line`, run only by `org.moqui.dev.InstanceServices.get#RegistryToken` (sealed `INSTANCE_ADMIN`, unless authorization is off) | The `instance-hosting` `token-command` of the runtime conf whose `name` the image's `InstanceImage.authTokenCmd` holds, split on whitespace, with no shell; a value that names no entry is refused, so a row's text is never run. No script can start a process | The named environment, stdin closed; the first line of output is the registry password | 30 seconds |
| Terminal host session | `crates/moqui-host/src/term/exec.rs` | This binary as `moqui term-exec`, which becomes a session leader on a pty and then `exec`s the session's program | The environment is cleared and set to the session's own list; the program's argument list goes in one variable as JSON | |
| Terminal session holder | `crates/moqui-host/src/term/held.rs` `ensure` | This binary as `moqui term-hold`, in its own process group so it outlives the server | The server's environment plus the holder's directory | |
| Component deploy `git` | `crates/moqui-devkit/src/deploy/git.rs` `git_command` | `git <arguments> [target]`; the deploy token reaches `git` as `http.extraHeader=Authorization: Basic <base64 of user:token>` in `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_0`/`GIT_CONFIG_VALUE_0`, never in the argument list (`git_process`; git 2.31 or newer). The URL is passed after `--` | `GIT_TERMINAL_PROMPT=0` | The URL is `https` or `ssh` only ([Outbound](Attack-Surface-Outbound.md#requests-the-server-makes)) |
| CI step shell | `crates/moqui-git/src/ci/runner/mod.rs` `run_step` | `<shell> -c <step command>` in the job's workspace; the shell is `sh` by default | `MOQUI_CI_TOKEN` is removed and the job's variables are added; the rest of the runner's environment is passed | A deadline for each step; its own process group |
| Restart and detach | `crates/moqui-host/src/lib.rs` `run_again`; `crates/moqui-host/src/detach.rs` `spawn_child` | This binary with the arguments the server was started with. `run_again` replaces the process on Unix. `spawn_child` starts it in a session of its own | The server's environment, plus a marker variable for the detached child | |
| Port holder lookup | `crates/moqui-host/src/port_check.rs` | `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fpc` (or `-iUDP:<port>`) | | |

## Other local processes

The `pdf` feature, which every server package includes, starts headless Chromium when a binary is found, with DevTools on a pipe (`--remote-debugging-pipe`) on Unix, so there is no socket another local process could open (SEC-SCR-10); on Windows on `--remote-debugging-port=0` at `127.0.0.1`, with no authentication of its own. The test client's browser (`moqui test-client`) keeps a loopback port. If no Chromium binary is found, the server still starts and PDF is unavailable.

An OpenSearch child starts only when `search-facade@backend` is opensearch, `search-facade/cluster@url` is loopback (the property default is `http://127.0.0.1:9200`), nothing is already answering, `runtime/opensearch/bin` exists, `search-facade/cluster@run` is not `false` (default `true`), and there is no pid file. The default `search-facade@backend` is `postgres`, so the child stays off. User and password properties default to empty.

## Local sockets and secret files

| Thing | Where | Mode or binding | Code |
|---|---|---|---|
| Chromium DevTools | On Unix the PDF browser's pipe (its descriptors 3 and 4), no socket; on Windows, and for the test client's browser, loopback TCP, a port the browser picks, written to `DevToolsActivePort` in the profile directory under `<runtime>/tmp` | The pipe is the server's alone; a port is `127.0.0.1` with no authentication of its own (see [Other local processes](#other-local-processes)). The browser sends every request, loopback included, to a proxy at `127.0.0.1:9` | `crates/moqui-host/src/chromium.rs`, `crates/moqui-host/src/pdf.rs` |
| Embedded Postgres socket | On Unix a socket in the data directory, with `listen_addresses` empty, so there is no TCP listener. On Windows `listen_addresses` is `127.0.0.1` | When the data directory's path has a character outside letters, digits and `/._-+@%:,`, the socket directory is `moqui-pg-<hash of the data directory path>` under the temp directory (or under `/tmp`). The server creates it with mode 0700 when it is missing, and refuses to start on one that is not a directory of this user with mode 0700 (`embedded::socket_dir_private`; a symlink is refused) | `crates/moqui-drivers/src/embedded.rs` `socket_dir_for`, `start` |
| Embedded Postgres password file | `pgpass` in the embedded database directory | Written 0600, or made 0600 at a start when it is there (`embedded::private_password_file`) | `crates/moqui-drivers/src/embedded.rs` |
| Terminal holder socket | A socket in the holder's directory | The directory is set to 0700 at each start and the socket to 0600 | `crates/moqui-host/src/term/hold.rs` `make_dir`, `serve` |
| The env file | The file `moqui_lib::env` writes | Written through a partial file with mode 0600 | `crates/moqui-lib/src/env.rs` |
| TLS private keys | The files `write_private` writes | 0600 | `crates/moqui-host/src/tls.rs` `write_private` |
| Edge member key | The edge link's member key file | 0600 | `crates/moqui-host/src/edge_link/member_key.rs` |
| Database backups | Dump files and directories | Files 0600 (a dump's partial file before `pg_dump` writes it), directories 0700 | `crates/moqui-drivers/src/backup.rs` |
| Upload temp files | Multipart bodies spooled to disk | 0600 | `crates/moqui-core/src/web/multipart.rs` |

## Startup port check

`/metrics` is on the public HTTP listener. It is not its own port. `port_check` probes HTTP, HTTPS, cluster, and SSH before the database opens. It does not probe SMTP.

## Build features

Default Cargo features are `maximal` and `devtools` (`crates/moqui-host/Cargo.toml`). `maximal` includes `enterprise`, which includes `git`, `llm`, `tui`, and `ssh`. `lite` includes `llm` and `tui`. `git` and `ssh` are on the `enterprise` and `maximal` sets. A default `cargo build` therefore compiles `/git`, `/termws`, and the LLM routes. `inbound-smtp` is on Enterprise and Maximal. `embedded-postgres` is on Lite and Maximal. Enterprise leaves that feature out and uses an external database.

The sets in `crates/moqui-host/Cargo.toml` are these. No `dev` set exists; its line is commented out.

| Set or feature | Holds | Not in it |
|---|---|---|
| `lite` | `axum` (the HTTP and HTTPS listeners, ACME), `embedded-postgres`, `pdf`, `smtp` (outbound mail), `tui`, `llm`, `test-client`, `telemetry`, `web-push`, `backup-s3`, `grpc` | `mysql`, `mail` and `imap`, `inbound-smtp`, `git`, `ssh`, `sso`, `print`, `image`, `xlsx`, `rtc`, `sip`, `channels`, `xmpp`, `edge` |
| `enterprise` | `axum`, `postgres`, `mysql`, `pdf`, `mail`, `git`, `image`, `xlsx`, `ssh`, `sso`, `print`, `inbound-smtp`, `tui`, `llm`, `test-client`, `telemetry`, `grpc`, `web-push`, `backup-s3`, `rtc`, `sip`, `channels`, `xmpp`, `edge` | `embedded-postgres`, `devtools` |
| `maximal` | `enterprise` and `embedded-postgres` | `devtools` |
| `devtools` | The file watcher of `moqui-devkit` and the LSP crates; in `crates/moqui` also the `dev-aide` and `example` crates | |
| `embed-production`, `embed-all`, `embed-opensearch`, `embed-chromium`, `speech-local`, `ic-stats` | In no set. `speech-local` is empty | |

`./moq build` makes one of four packages, each with `--no-default-features`. Maximal, the default, is `--profile release -F maximal,devtools` and gives `target/release/moqui`. Debug is `--profile dev -F maximal,devtools` and gives `target/debug/moqui`. Lite is `--profile lite -F lite` and gives `target/lite/moqui`. Enterprise is `--profile enterprise -F enterprise` and gives `target/enterprise/moqui`. The default release build therefore has `devtools`; Lite and Enterprise do not. `--embed production|all`, `--embed-opensearch` and `--embed-chromium` add the `embed-*` features.

## Confs under `runtime/conf`

The table gives the purpose, listener, address, role and switch properties each file sets as `default-property` values, and for the top-level files and `dev-group` their other elements. The other elements of the directory examples were not read.

| Conf | For | Opens beyond the default |
|---|---|---|
| `MoquiDevConf.xml` | Development | `instance_purpose=dev`. `moqui_conf__webapp_list__webapp__WEBROOT__allow_origins=localhost,127.0.0.1` (this machine on any port). `moqui_conf__tools__empty_db_load=all`. `moqui_conf__ssh_server__port=2222` turns the SSH server on. `moqui_conf__llm_facade__endpoint__DEFAULT__url=http://127.0.0.1:1234`. `<receiver grpc-enabled="true"/>` under telemetry. The tarpit is off for screens and transitions. Example datasource passwords (`moquiroot`, `moqui`) are in comments |
| `MoquiDemoConf.xml` | A demo | `moqui_conf__tools__empty_db_load=all`. Adds the component `components/dev/demo` |
| `MoquiLoadTestConf.xml` | Load tests | The tarpit is off for screens, transitions and services. `<login history-store="false"/>`. `moqui_conf__ssh_server__port=2222`. SSH and terminal session limits are 1000 |
| `MoquiProductionConf.xml` | Production | Sets the entity facade's `crypt-pass` from `moqui_conf__entity_facade__crypt_pass`. No listener property |
| `MoquiSitesConf.xml` | Example public sites | `root-screen` and `public-site` elements by host for `mr-pass` and an example site; `framework-routes` is `refuse` on two hosts and `include` on one |
| `dev-group/MoquiGroupConf.xml` | A development group's server | `instance_purpose=production`. `moqui_conf__ssh_server__port=2222` with `moqui_conf__ssh_server__host=0.0.0.0`. `moqui_conf__llm_facade__gateway__enabled=true`, with `<gateway ... require-client="true">`. `moqui_conf__llm_facade__mcp__server_enabled=false`. `embedded_db_backup_scheduled=true` |
| `all-in-one/MoquiAllInOneConf.xml` | One node in every role (`node_role=all-in-one`) | `moqui_conf__mail_delivery__mta=moqui`. `channels_webhooks_enabled=true`. `rtc_media_host=0.0.0.0` on port 10000. `turn_port=3478`. `sip_host=0.0.0.0` on ports 5060 and 5061 |
| `edge/MoquiEdgeConf.xml` | An edge node (`node_role=front_edge`) | `pass_backend_url=edge:`. `edge_xmpp_port=5222`, `edge_xmpp_tls_port=5223`. `moqui_conf__mail_delivery__mta=moqui` with a relay at `127.0.0.1:10025`. `channels_webhooks_enabled=true`. `rtc_media_host=0.0.0.0` on port 10000. `turn_port=3478`. `sip_host=0.0.0.0` on ports 5060 and 5061 |
| `edge-cluster/MoquiEdgeConf.xml` | An edge node of a cluster | As the edge conf without the SIP properties, plus `cluster_transport`, `cluster_protocol`, `edge_xmpp_host` and `edge_ssh_tunnel_port` |
| `multi-domain/MoquiEdgeConf.xml`, `full-stack/MoquiEdgeConf.xml` | Edge nodes of those examples | As the edge conf without the SIP properties; `full-stack` also sets `cluster_transport` and `cluster_protocol` and no XMPP ports |
| `front/MoquiFrontConf.xml`, `full-stack/MoquiFrontConf.xml` | A front node (`node_role=front`) | `front` sets `edge_xmpp_port=5222` and `edge_xmpp_tls_port=5223` |
| `edge/MoquiLocalConf.xml`, `multi-domain/MoquiLocalConf.xml`, `local-failover/MoquiLocalConf.xml` | The node behind the edge | `instance_purpose=production` only, among the properties looked for |
| `front/MoquiLocalConf.xml`, `full-stack/MoquiLocalConf.xml`, `local-cluster/MoquiLocalConf.xml`, `local-cluster/MoquiPooledConf.xml` | Cluster nodes | `cluster_transport` and `cluster_protocol` |
| `pass/MoquiAuthorityConf.xml` | A PASS authority | `pass_authority_grpc_enabled=true`. `pass_client_forum_sites` and `pass_client_forum_callback` name an example client |
| `pass/MoquiPassConf.xml` | A PASS site | `pass_backend_url=https://boss.example.com`, `pass_client_id=forum`, `pass_backend_transport`, `pass_backend_grpc_url` |

## Packaging

`podman/Containerfile` builds on `debian:13-slim`. It makes the user `moqui` (uid 1000, shell `nologin`), copies the binary, the confs and the components to `/opt/moqui`, and gives the directory `/opt/moqui` itself to `moqui`; the binary is mode 755. It declares the volume `/var/lib/moqui` and exposes 8080 and 8443. It has no `USER` line. `podman/entrypoint.sh`, when it starts as root, makes the data directories under `/var/lib/moqui`, creates `/var/lib/moqui/env` with mode 600 and links it as `/opt/moqui/.env`, and then runs the server with `setpriv --reuid=moqui --regid=moqui --init-groups --inh-caps=-all`. When it starts as another user it runs the server directly. The health check is `curl http://127.0.0.1:8080/health`.

`podman/quadlet/moqui.container` publishes 8080 and 8443 on every host address and sets `instance_purpose=production`. The four compose files publish the same two ports and set `instance_purpose: production`. The three with a database give the server and the database container `${MOQUI_DB_PASSWORD:?set MOQUI_DB_PASSWORD in podman/.env}` (and the MySQL one `MOQUI_DB_ROOT_PASSWORD` the same way), so there is no default password and a start without one fails; `podman/env.example` leaves both empty. They set `moqui_conf__entity_facade__datasource__TRANSACTIONAL__sslmode: disable` (the compose network only) and do not publish the database port; a commented line publishes it on `127.0.0.1` of the host (plans/Security-Review.md SEC-OUT-07, fixed). The two with OpenSearch run it as `user: "0:0"` with `DISABLE_SECURITY_PLUGIN: "true"`.

`systemd/moqui.service` runs `/opt/moqui/moqui` as user and group `moqui` with `Type=notify`, `NotifyAccess=main`, `EnvironmentFile=-/etc/moqui/environment`, `KillMode=mixed`, and a reload that sends SIGHUP. Its sandbox settings are `NoNewPrivileges=true`, `PrivateTmp=true`, `ProtectSystem=strict`, `ProtectHome=true`, `ReadWritePaths=/opt/moqui /opt/moqui/components` and `ReadOnlyPaths=/opt/moqui/moqui -/opt/moqui/MoquiConf.xml -/opt/moqui/conf`: the runtime's data and components are writable, the binary and the confs are not, whoever owns them (SEC-OUT-13), and the install steps (docs/Run-Deploy.md) make them root's. The image (`podman/Containerfile`) has the binary and the confs root's and not writable to uid 1000, `components/` uid 1000's, and `/opt/moqui` root's, group `moqui` and sticky, which the entrypoint no longer hands to the moqui user. It does not set `PrivateDevices`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectControlGroups`, `RestrictAddressFamilies`, `RestrictNamespaces`, `CapabilityBoundingSet`, `SystemCallFilter`, `LockPersonality` or `UMask`. The drop-ins `postgres.conf` and `mysql.conf` only order the unit after the database service. `systemd/moqui.sysusers` makes the user `moqui` with home `/opt/moqui` and shell `nologin`.

The `moq` script downloads OpenSearch with `curl` and checks it against the SHA-512 file published beside it at the same address. It downloads a Chromium snapshot and an add-on's zip with `curl -fL`, and clones an add-on with `git clone -- <url>`; `verify_published_sha512` is called only for OpenSearch.

## Test-only hooks

- `HttpFixtures` (`crates/moqui-core/src/service/http_fixtures.rs`) answers every request of `outbound::send` when it is on. `set_enabled(true)` fails unless `http_fixtures::allow()` ran in the process, and only the `moqui test` runner calls it (`crates/moqui-host/src/test_cmd/mod.rs`). A server of `moqui run` cannot turn it on.
- The `X-Moqui-Test-Peer` header (`crates/moqui-host/src/http/test_peer.rs`) is read only after `test_peer::allow()`, which only `moqui test --serve` calls (`crates/moqui-host/src/test_cmd/serve.rs`). It is taken only when the TCP peer and the client address are both loopback, and only for an IPv4 address in `127.0.0.0/8`.
- Mail capture (`crates/moqui-core/src/service/mail_capture.rs`) keeps a send in memory in place of sending it, up to 500 messages for a factory. It is on when the property `email-client@capture-sends` is `true` or a script turns the `MailCapture` tool on. Nothing in `is_enabled` or `set_enabled` checks for a test run or the instance's purpose, so any instance can have it on.
- `chat_link_preview_service` names a service asked in place of the network for a chat link preview, and `mail-delivery@mx-override` sends a recipient domain's mail to a named `host:port`; both are read with no check of the instance's purpose in the lines read (`crates/moqui-core/src/service/chat_link.rs`, `crates/moqui-mail/src/mx.rs` `override_for`).

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- What starts the CI runner (`moqui ci-run`) on the server's host, as which user, and with which environment was not read; nor was the built-in pipeline (`crates/moqui-git/src/ci/builtin.rs`) or the `git` child of `crates/moqui-git/src/ci/runner/http.rs`.
- When the PDF browser is started (at startup or on the first PDF) and whether the profile directory under `<runtime>/tmp` is readable by other local users was not read.
- The mode of `runtime/tmp` was not read; nor what the logs can hold.
- The desktop app's child processes (`crates/moqui-desktop`: the sidecar, `xdg-mime`, the opener, `hostname`) and its single-instance socket were not read.
- The dev tooling children (`ffmpeg`, the CDP Chromium of `crates/moqui-host/src/test_client/cdp.rs`, the test workers) were not read.
- The systemd notify socket, signal handling, and what a start with a bad conf does were not read.
- Whether port 9200 of the OpenSearch containers is published, and which package `podman/build.sh` puts in the image, was not read.
- The Chromium snapshot and add-on zip download lines of `moq` were found by `grep` and not read in full.
