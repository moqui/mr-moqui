# Run and Deploy

This guide covers running MR Moqui on your machine and deploying it on a server, in a container or as a cluster. It
also covers connecting instances to each other: PASS servers to a BOSS server, and developers' instances to a team's
group server. The first sections give the steps for the common cases; the sections after them have the detail. Building
and the build packages are in [Build-Test](Build-Test.md).

- [Quick start](#quick-start): develop on your machine, run one production server, run in a container
- [Everyday commands](#everyday-commands)
- [Choose a topology](#choose-a-topology)
- [Production checklist](#production-checklist)
- Detail:
  - setup: [Runtime directory and conf](#runtime-directory-and-conf), [Command line](#command-line),
    [Data and the first start](#data-and-the-first-start), [Database and search](#database-and-search),
    [HTTPS, ports and proxies](#https-ports-and-proxies);
  - deployments: [One production server](#one-production-server), [Cluster](#cluster),
    [PASS servers with a BOSS server or cluster](#pass-servers-with-a-boss-server-or-cluster),
    [Dev instances with a dev group server](#dev-instances-with-a-dev-group-server),
    [A local server with an edge node](#a-local-server-with-an-edge-node), [An all-in-one node](#an-all-in-one-node);
  - reference: [Components](#components), [Moving from Moqui Java](#moving-from-moqui-java),
    [Directory layout](#directory-layout).

## Quick start

### Develop on your machine

You need Rust 1.99 or newer (`rustc --version`; quickjs-oxide declares 1.99, and `rust-toolchain.toml` pins 1.99.0),
git and bash, on Linux, macOS, or Windows. On Windows, install the MSVC toolchain (`x86_64-pc-windows-msvc`), Visual
Studio Build Tools with the "Desktop development with C++" workload, and Git for Windows, then use `moq.cmd` instead
of `./moq` (the binary is `target\release\moqui.exe`). The first build that needs it downloads NASM 3.02 to
`%USERPROFILE%\.moqui\nasm`; set `MOQUI_NASM` to an existing directory or `nasm.exe` to skip that download. A
`cargo build` that does not go through `moq` still needs NASM on `PATH`, and for a dev build or `cargo test`,
`CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed`. NASM may no longer be needed at all: [Windows](Build-Test.md#windows).

```sh
git clone https://github.com/moqui/mr-moqui.git
cd mr-moqui
./moq component-get apps   # optional: Marble ERP (mr-marble) and IronStone BOSS (mr-boss), with Mantle (mr-mantle)
./moq load                 # builds the Maximal package the first time, then loads every data type, demo included
./moq run                  # serves http://localhost:8080; ctrl-c stops it
```

Open `http://localhost:8080` and log in as `john.doe` with the password `moqui`.

- **The first build** downloads the embedded Postgres archive once (`scripts/pg-payload.sh strip`).
- **The dev conf** (`runtime/conf/MoquiDevConf.xml`) loads every data type into an empty database on the first start,
  so a plain `./moq run` gets the same data. `./moq load` is for loading again later.
- **To update**, `./moq git-pull` pulls every repository, and `./moq run` builds before it starts.

### Run one production server

On a build machine with the same OS and CPU as the server, build the Maximal package into an archive, leaving out the
local database and secrets:

```sh
./moq archive --release --no-data   # mr-moqui-YYYYMMDD.zip, which expands to mr-moqui-YYYYMMDD/
```

On the server:

1. Make the `moqui` user and unpack the archive at `/opt/moqui`. The archive directory is a runtime directory with the
   binary in it. The `systemd/` files here and in step 4 come from a checkout.

   ```sh
   sudo systemd-sysusers systemd/moqui.sysusers   # makes the moqui user
   unzip mr-moqui-20261029.zip
   sudo install -d -o moqui -g moqui /opt/moqui
   sudo cp -a mr-moqui-20261029/. /opt/moqui/ && sudo chown -R moqui:moqui /opt/moqui
   ```

2. Put the certificate for your host in `/opt/moqui/cert/`, for example certbot's `cert/erp.example.com/fullchain.pem`
   and `privkey.pem` ([certificates](#certificates)).
3. Write the host's settings to `/opt/moqui/.env`. The first start adds the generated secrets to the same file.
   `webapp_https_port` is the port clients use; leave it out when they reach the server on 443.

   ```sh
   sudo -u moqui tee /opt/moqui/.env > /dev/null <<'EOF'
   webapp_http_host=erp.example.com
   webapp_https_enabled=true
   webapp_https_port=8443
   EOF
   sudo chmod 600 /opt/moqui/.env
   ```

4. Install the unit and start it:

   ```sh
   sudo cp systemd/moqui.service /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl enable --now moqui
   ```

5. The first start creates the tables and loads the seed, seed-initial and install data. Open
   `https://erp.example.com:8443/Login` and create the first administrator with **Create Initial Admin Account**. The
   form is there only while nobody has an account.
6. Back up `/opt/moqui/.env` with the database. It holds `entity_ds_crypt_pass`, the key of the encrypted fields.

Upgrades, backups, binding port 443 and the rest are in [One production server](#one-production-server).

### Run in a container

```sh
podman/build.sh                                    # ./moq build --release, then the image localhost/moqui:latest
cp podman/env.example podman/.env                  # then change the passwords
systemctl --user start podman.socket
podman compose -f podman/moqui-compose.yml up -d   # one container, embedded Postgres
podman exec moqui-moqui-1 curl -fsS http://127.0.0.1:8080/health
```

The image runs the production conf, so the first start loads the seed, seed-initial and install data. Three more
stacks (Postgres; Postgres and OpenSearch; MySQL and OpenSearch), the data volume and Quadlet are in
[podman/README.md](../podman/README.md).

## Everyday commands

`./moq` runs the tasks that were Gradle tasks in Moqui Java. `./moq help <task>` describes one, and a task name matches
by the prefix of each part (`./moq r-p` is `run-production`).

| Task | `./moq` | Moqui Java |
|---|---|---|
| Run the dev server | `./moq run` | `./gradlew run` |
| Run on another port | `./moq run -p 8081` | `java -jar moqui.war port=8081` |
| Run with the production conf | `./moq run-production` | `./gradlew runProduction` |
| Set a property for one run | `./moq run -D entity_ds_db_conf=postgres` | `-Dentity_ds_db_conf=postgres` |
| Load every data type | `./moq load` | `./gradlew load` |
| Load some types | `./moq load seed seed-initial install` | `./gradlew load -Ptypes=seed,seed-initial,install` |
| Load one component's data | `./moq load seed components=mr-mantle` | `java -jar moqui.war load components=...` |
| Load one file | `./moq load -l runtime/components/addon/mr-mantle/data/SomeData.xml` | `java -jar moqui.war load location=...` |
| Start over with an empty database | `./moq clean-db && ./moq load` | `./gradlew cleanDb load` |
| Load once, then save a snapshot | `./moq load-save` | `./gradlew loadSave` |
| Save a snapshot / restore it | `./moq save-db before-upgrade`, `./moq reload-save before-upgrade` | `./gradlew reloadSave` |
| Pull every repository | `./moq git-pull` | `./gradlew gitPullAll` |
| Status of every repository | `./moq git-status` | `./gradlew gitStatusAll` |
| Get a component and its dependencies | `./moq component-get mr-boss` | `./gradlew getComponent -Pcomponent=...` |
| Run a component's specs | `./moq test mr-mantle` | `./gradlew runtime:component:mantle-usl:test` |
| Run client specs against a server | `./moq test-client framework` | |
| Run a load script against a server | `./moq test-load mr-marble --script OrderToCash` | |
| Archive the runtime with the binary | `./moq archive --release` | `./gradlew addRuntime` |
| Remove the database, logs and old build output | `./moq clean-all` | `./gradlew cleanAll` |
| Show the binary's package and features | `target/release/moqui version` (`target\release\moqui.exe version` on Windows) | |

## Choose a topology

| Topology | Use it for | Conf | Package | Database | Section |
|---|---|---|---|---|---|
| One dev instance | Developing on your machine | `conf/` (`MoquiDevConf.xml`) | Maximal with the dev tools (the default) | Embedded Postgres | [Quick start](#develop-on-your-machine) |
| One production server | One business or one site | `conf/` (`MoquiProductionConf.xml`) | Maximal (`--release`) | Embedded Postgres, or an external server | [One production server](#one-production-server) |
| One server, several sites by host name | Separate sites on sub-domains or related domains, each host with its own root screen | `conf/` (`MoquiSitesConf.xml`) | Maximal (`--release`) | As one production server | [Several sites by host name](#several-sites-by-host-name) |
| Cluster | More capacity, or a member that can fail | [`conf/local-cluster/`](../runtime/conf/local-cluster/README.md) | Enterprise (`--enterprise`) | One external Postgres for all members | [Cluster](#cluster) |
| PASS servers with a BOSS server or cluster | Public sites (forum, members) with no business data on the public machines | [`conf/pass/`](../runtime/conf/pass/README.md) | PASS: any server package, Lite is the smallest. BOSS: Maximal or Enterprise | Each PASS server its own; BOSS its own | [PASS](#pass-servers-with-a-boss-server-or-cluster) |
| Dev instances with a dev group server | A team: shared repositories, tasks and models | [`conf/dev-group/`](../runtime/conf/dev-group/README.md) | Laptops: Dev. Group server: Maximal or Enterprise | Each its own | [Dev group](#dev-instances-with-a-dev-group-server) |
| Public demo server | The apps for anyone to try with the `john.doe` login | `conf/` (`MoquiDemoConf.xml`) | Maximal (`--release`) | Embedded Postgres, restored on a schedule | [Public demo server](#public-demo-server) |
| A local server with an edge node | A server in the office, behind a firewall or a changing address, with a public presence: web access, mail, chat webhooks, public sites | [`conf/edge/`](../runtime/conf/edge/README.md) | Local: Maximal or Enterprise. Edge node: Maximal | Each its own; the edge node keeps only the link's queue and copies | [A local server with an edge node](#a-local-server-with-an-edge-node) |
| An all-in-one node | A public server, with its own users, apps and data, that is also the edge node of one local node (a desktop or office node), so no second cloud instance is needed | [`conf/all-in-one/`](../runtime/conf/all-in-one/README.md) | Server: Maximal. Local: Maximal or Enterprise | Each its own; the server also keeps the local node's queue and copies | [An all-in-one node](#an-all-in-one-node) |
| Several local nodes on one edge node | Branch offices, or the separate organizations of one group, each with its own data and domains behind one edge node | [`conf/multi-domain/`](../runtime/conf/multi-domain/README.md) | Local: Maximal or Enterprise. Edge node: Maximal | Each local node its own; the edge node keeps each one's queue and copies | [Several local nodes on one edge node](#several-local-nodes-on-one-edge-node) |
| A local cluster with an edge node | More capacity, or a member that can fail, with a public presence and no load balancer of your own: the edge node pools and balances the members | [`conf/local-cluster/`](../runtime/conf/local-cluster/README.md) (`MoquiPooledConf.xml`) | Members: Enterprise (`--enterprise`). Edge node: Maximal | One external Postgres for all members; the edge node keeps the pairing's queue and copies | [A local cluster with an edge node](#a-local-cluster-with-an-edge-node) |
| A local node with a standby | A second machine, started by hand or by your own automation when the first one dies, that takes over at the edge node | [`conf/local-failover/`](../runtime/conf/local-failover/README.md) | Both: Maximal or Enterprise. Edge node: Maximal | The same external database, a promoted replica, or the last backup restored | [A local node with a standby](#a-local-node-with-a-standby) |
| An edge cluster | An edge node that can fail, or more capacity at the edge: several edge nodes on one database, each local node linked to every one | [`conf/edge-cluster/`](../runtime/conf/edge-cluster/README.md) | Edge members: Maximal with an external database, or Enterprise. Local: Maximal or Enterprise | One external Postgres for the edge members; each local node its own | [An edge cluster](#an-edge-cluster) |
| The desktop app | A node on a person's own computer, in a window with a tray, paired with an edge or all-in-one node | In the app; its edge or all-in-one node as above | Maximal, in the app | Embedded Postgres in the app data directory | [moqui-desktop](../crates/moqui-desktop/README.md) |

The conf is the runtime conf each node runs, under `runtime/`. A topology's directory has a conf for each kind of node,
whose comments say what each setting does, and a README with its diagram, the `.env` names each node needs and the
commands ([runtime/conf](../runtime/conf/README.md)). The sections below keep what each topology is, its steps and
what is not built. The topologies still to come (a front node, the full stack) get their directories as they are built
([Edge-Node2 §3](../plans/Edge-Node2.md#3-topologies)).

## Production checklist

- **Package**: Maximal for embedded Postgres, or Enterprise for an external database. Dev and Debug carry the dev tools
  and are not for deployment ([Packaging](Build-Test.md#packages)).
- **Purpose**: `instance_purpose` is `production` everywhere except in the dev conf. An archive's `MoquiConf.xml` is
  the production conf.
- **Secrets**: the first start writes generated secrets to `runtime/.env` with mode 0600. Keep the file private and back
  it up. Without `entity_ds_crypt_pass` the encrypted fields cannot be read ([conf](#runtime-directory-and-conf)).
- **Data**: no demo data, so no `john.doe` with password `moqui` ([first start](#data-and-the-first-start)).
- **HTTPS**: a certificate in `runtime/cert`, or a proxy in front with `webapp_trusted_proxies` set
  ([HTTPS](#https-ports-and-proxies)).
- **Ports**: firewall 8080. `webapp_http_bind` sets the address of both HTTP and HTTPS, so `127.0.0.1` suits only a
  server behind a proxy on the same machine. The cluster listener (8444) starts on every server, on every address, so
  firewall it unless cluster members use it ([ports](#ports)).
- **Backups**: run the embedded database's backup job, or back up the external database
  ([backups](#backups)).
- **Disk and memory**: the thresholds at which the System home warns are `resource-watch`'s; copy the disk, memory
  and CPU alert rules so someone is told ([resource watch](#resource-watch)).
- **Listeners**: SSH, inbound SMTP, A2A, MCP and the LLM gateway are off by default. Turn on only the ones that are used.
- **CORS**: leave `webapp_allow_origins` empty. The dev conf sets `*`.
- **Lock-down**: the full list is in [Attack-Surface.md, Heavy lock-down](security/Attack-Surface.md#heavy-lock-down).

---

## Runtime directory and conf

The runtime directory holds everything that varies per instance: components, conf, certificates, the database, logs
and secrets ([runtime/README.md](../runtime/README.md)). The binary takes the first of:

1. `MOQUI_RUNTIME`;
2. `runtime` in the working directory, when it has a `components/base/` directory;
3. the working directory itself, when it has a `components/base/` directory: an unpacked archive, or `/opt/moqui`,
   started in its own directory (`cd /opt/moqui && ./moqui`; the systemd unit and the container set that directory);
4. a new `runtime` in the working directory.

The runtime is never found from where the binary is, so one binary runs several instances, each started in its own
directory. Run a command such as `moqui load` from the instance's directory, or with `MOQUI_RUNTIME`; from anywhere
else it finds no runtime and makes a new one there. The server and `moqui load` fill a runtime that has no framework
component with a skeleton runtime written from the files built into the binary (below), so a lone binary starts.

### Built-in framework files

The binary carries a copy of the framework and tools components (less their `test/`, which holds both suites) and of a
webroot stub, listed in `crates/moqui-embedded/webroot-files.txt`: the REST, login, error and html `/apps` screens, and
the html render mode's style sheet and scripts with the two libraries it loads (Tom Select, SortableJS). Fonts, icons,
`/qapps` and the web client's JavaScript libraries are not in it. It uses the copy two ways:

- **Startup comparison.** After the factory boots, the server and `moqui load` compare the framework, tools and
  webroot components on disk with the copies and log `N framework, tools and webroot files differ from the ones built
  into this binary`, then one line per file: changed (with the first line that differs), missing on disk, or added on
  disk (framework and tools; the webroot copy is a subset). The files on disk are used; the lines say why a changed
  file may no longer fit the binary. A file the binary has compiled in (`template/XmlActions.js` and three others) is
  marked as ignored, since its disk copy is never read. `moqui embedded diff [<component>/<path>]` prints the diffs
  and `moqui embedded list` the built-in files.
- **Skeleton runtime.** A runtime with no `components/base/framework/component.xml`, and no `MOQUI_FRAMEWORK`, gets
  the framework, and tools and the webroot stub when `components/base/tools` and `components/base/webroot` are not
  there, with a placeholder page at `/` in place of the web client, and the sets' manifest `components/components.xml`
  when it has none (only the sets it lists load). It boots, loads the seed data, answers `/rest`
  (with the tools `moqui` API), `/Login`, `/status` and the html `/apps` (System, Tools, Dashboards), and runs
  `moqui tui`; links to fonts, icons and the web client are broken until the full webroot is installed, and the
  application components are not there. `moqui embedded extract <dir>` writes the same files into a directory. Files
  already there are never replaced.

Keep the runtime path short. The embedded Postgres socket, `<runtime>/db/embedded/.s.PGSQL.5432`, must be under 108
bytes, or the start fails with "postgres socket path is N bytes".

The conf is merged in this order:

1. `components/base/framework/conf/MoquiDefaultConf.xml`;
2. each component's `MoquiConf.xml`, in component order;
3. the runtime conf, on top.

The runtime conf is the first of these that is found:

1. the file `MOQUI_CONF` names, relative to the runtime directory or absolute (`./moq run -c FILE` and `moqui --conf
   FILE` set it);
2. `<runtime>/MoquiConf.xml`;
3. `<runtime>/conf/MoquiDevConf.xml`.

`./moq run` uses the dev conf and `./moq run-production` uses `conf/MoquiProductionConf.xml`. The confs of the other
topologies are in a directory each under `conf/`, as `-c conf/edge/MoquiEdgeConf.xml`
([Choose a topology](#choose-a-topology)). A running server takes
a changed conf file without a restart where what reads the setting allows it (System › Conf, `systemctl reload moqui`
or SIGHUP, and the file watcher on a dev instance), and lists the rest as waiting for a restart
([Component-Deploy](tools/Component-Deploy.md)). Conf merges add components but never remove one. To leave a component out, keep it out of `runtime/components/addon`. The example dev
component is the exception: `example_disabled=true` leaves it out. The demo dev component is off until
`demo_disabled=false` or a conf names it ([Public demo server](#public-demo-server)).

### Properties

A `default-property` in a conf is a default only. A property is looked up in this order:

1. a `-D name=value` on the command line (`./moq -D` sets it as an environment variable);
2. the process environment;
3. `runtime/.env`;
4. the merged conf's `default-property`.

Use the underscore names (`entity_ds_db_conf`), not dotted ones. An empty value counts as unset, so it cannot clear a
conf's default; to clear one, override the `default-property` in the runtime conf.

`runtime/.env` holds `KEY=value` lines. Blank lines and `#` comments are ignored, a matching pair of quotes is
stripped, and `$` is not expanded. It is read after the conf merge, so it cannot select `MOQUI_CONF`. An archive made
with `--no-data` leaves it out, so it is the place for per-host settings that should survive an upgrade.

When a secret property still ends in `CHANGEME` at startup, a value is generated and written to `runtime/.env`. This
covers `entity_ds_crypt_pass`, `session_cookie_key`, `ssh_host_key`, `git_lfs_token_key`, `cluster_tls_cert`,
`cluster_tls_key` and `edge_link_key`. A new file gets mode 0600, and a `.env` that cannot be written fails the start.
`mail_dkim_key` (empty by default) is made there too, the first time a server needs it.

The properties most deployments set or should know. Their defaults are in `MoquiDefaultConf.xml`.

| Property | Default | What it does |
|---|---|---|
| `instance_purpose` | `production` (`dev` in the dev conf) | `production`, `dev` or `test`. Only `dev` loads the dev components (Dev AIDE, the example component); the Desktop app's tray item Developer tools sets it for its node |
| `default_locale`, `default_time_zone`, `database_time_zone` | `en_US`, `UTC`, `UTC` | |
| `webapp_http_host` | empty: the request's host | The host in generated URLs |
| `webapp_http_bind` | empty: every address | The address the HTTP and HTTPS listeners bind; `127.0.0.1` keeps them on the machine |
| `webapp_http_port` | empty: 8080 | The HTTP listen port (`-p`) |
| `webapp_https_listen_port` | empty: 8443 | The HTTPS listen port (`--https-port`), or `off` |
| `webapp_https_port` | empty: 443 | The HTTPS port in generated URLs and redirects |
| `webapp_https_enabled` | `false` | `true` makes generated URLs https |
| `webapp_allow_origins` | empty (`*` in the dev conf) | Other origins allowed for CORS |
| `webapp_trusted_proxies` | empty | Address ranges of the proxies in front ([proxies](#behind-a-proxy-or-load-balancer)) |
| `webapp_client_ip_header` | empty | The header those proxies put the client address in |
| `webapp_status_ips` | `127.0.0.1` | Addresses, besides loopback, that `/health` answers |
| `scheduled_job_check_time` | `60` | Seconds between job checks; `0` runs no scheduled jobs |
| `terminal_command_line`, `terminal_host_sessions` | `dev` | The browser terminal's `moqui>` command line, and programs on the server's operating system: `dev` on a dev instance, `false` off, empty on for any purpose ([Terminal](ui/Terminal.md#conf)) |
| `terminal_remote` | `false` | `true` lets those be reached from other machines, over HTTPS with a recent sign-in; `terminal_remote=true` in `runtime/.env` is the lightest way ([Terminal](ui/Terminal.md#who-may-open-what-and-from-where)) |
| `moqui_chromium_path` | empty | The Chromium for PDF and the browser tests; empty looks in `runtime/chromium`, then for an installed Chromium, Google Chrome or Microsoft Edge (the PATH and the OS's install locations) |
| `load_runner_targets`, `load_runner_max_runs`, `load_runner_max_users` | empty, `1`, `1000` | Load runs from Tools: the URL prefixes a run may reach besides this server and loopback (any URL on a dev instance; plain http to another machine only when listed), the loads at once, and the users a run |
| `ssh_port`, `ssh_host` | empty, `127.0.0.1` | The SSH server (SFTP, git, the apps in a terminal); off while `ssh_port` is empty (`2222` in the dev conf) |
| `rtc_media_port`, `rtc_media_host`, `rtc_public_address` | empty, `127.0.0.1`, empty | Call media ([WebRTC](interfaces/WebRTC.md)); off while `rtc_media_port` is empty; the public address is what browsers are told, for a server behind 1:1 NAT |
| `call_ring_seconds` | `45` | How long a call rings before it is missed |
| `chat_server_name`, `chat_to_device_retention_days` | `localhost`, `30` | The server name in the Matrix user IDs of browsers' crypto devices (set it before the first device: a change makes every device new), and how long a to-device message waits for a device that does not take it ([Chat keys](interfaces/Chat-Keys.md)) |
| `chat_store_key_secret` | generated | The platform secret each crypto device's browser-store key is derived under (HMAC of user, device and revision), so no table holds the keys. A new value makes every browser's store unreadable: each makes a new device and reads older messages from the key backup ([Chat keys](interfaces/Chat-Keys.md)) |
| `rtc_cascade_enabled` | `false` | Cascading media nodes ([WebRTC](interfaces/WebRTC.md#cascading-media-nodes)): a leg's media goes to the media node of its own socket, which relays to the call's node over both nodes' `rtc_media_port`; set it on every node |
| `call_captions_enabled`, `call_captions_provider`, `call_captions_store` | `false`, `llm`, `false` | Live captions in calls ([WebRTC, Captions](interfaces/WebRTC.md#captions)): off unless `true`; the provider is the model bound to the `transcribe` feature (`local`, speech-local, is not built yet); `true` keeps each caption as a `CallTranscript` |
| `call_captions_chunk_seconds`, `call_captions_queue`, `call_captions_workers`, `call_captions_timeout_seconds`, `call_captions_language` | `5`, `6`, `2`, `20`, empty | The longest chunk of speech sent, the chunks that wait (past this the oldest is dropped), how many of a call's go at once (all calls on a node together use at most half the service pool), how long one may take, and the language (empty lets the model tell) |
| `turn_port`, `turn_tls_port`, `turn_tls443_host`, `turn_public_host`, `turn_auth_secret`, `turn_external_urls`, ... | empty; the secret generated | The embedded TURN server, or coturn instead ([TURN](interfaces/TURN.md)) |
| `call_recording_enabled`, `call_recording_ack_required`, `call_recording_policy_revision` | `false`, `false`, `1` | Call recording by a call's host, with a notice to every leg, and whether each leg must acknowledge it ([Call recording](interfaces/Call-Recording.md)) |
| `call_recording_retention_days` | `90` | Recordings are deleted this long after they start; `0` keeps them |
| `call_recording_queue_frames`, `call_recording_segment_mb`, `call_recording_segment_minutes`, `call_recording_max_mb` | `4096`, `64`, `30`, `2048` | The recorder's queue, when a file is cut into the next segment, and where a recording stops |
| `call_recording_prompt_audio`, `call_captions_prompt_audio` | empty | WAV files a phone leg hears after the recording and captions tones ([SIP, Notices](interfaces/SIP.md#notices)) |
| `sip_port`, `sip_host`, `sip_tls_port`, `sip_tls_host`, `sip_rtp_ports`, `sip_rtp_host`, `sip_public_address`, `sip_tls_ca_file` | empty, `127.0.0.1`, empty, `sip_host`, `20000-20999`, `sip_host`, empty, the system bundle | Phone calls over SIP trunks ([SIP](interfaces/SIP.md)); off while both ports are empty, and only beside call media; the public address is what trunks are told in Contact and SDP |
| `channels_webhooks_enabled`, `channels_webhook_max_bytes` | `false`, `262144` | The chat bridges' routes a provider calls: `POST /channels/{chatBridgeId}/webhook` (its body at most this many bytes) and `GET /channels/{chatBridgeId}/media/{token}` ([SMS](interfaces/SMS.md)) |
| `channels_public_url`, `channels_media_key`, `channels_media_minutes` | empty, generated, `60` | The https base a provider reaches this server at, for the files of an MMS that goes out (none without it), the key their addresses are signed with, and how long one works |
| `sip_request_rate`, `sip_request_burst`, `sip_max_transactions`, `sip_max_connections`, `sip_max_connections_per_source` | `20`, `100`, `2000`, `500`, `100` | The SIP listener's flood caps: new requests a second per source and at once, the transactions past which new requests are dropped, TCP and TLS connections at once, and from one source ([SIP, Settings](interfaces/SIP.md#settings)) |
| `sip_registrar_enabled`, `sip_domain` | `false`, empty | The registrar for desk and soft phones, off even with the SIP listeners on; the realm and domain phones register to (`sip_public_address`, else `moqui`, when empty) ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `xmpp_domain`, `xmpp_rooms_domain`, `xmpp_upload_domain` | empty, `rooms.<domain>`, `upload.<domain>` | XMPP for local users ([XMPP](interfaces/XMPP.md)): off while `xmpp_domain` is empty; users are `user@<domain>`, rooms `room@rooms.<domain>` |
| `xmpp_port`, `xmpp_host`, `xmpp_tls_port`, `xmpp_tls_host`, `xmpp_websocket_enabled` | empty, `0.0.0.0`, empty, `xmpp_host`, `false` | The XMPP client listeners: STARTTLS (5222 by convention) and direct TLS (5223), each off while empty; `/xmpp-websocket` on the HTTP listener when `true` |
| `xmpp_upload_url`, `xmpp_upload_max_bytes`, `xmpp_upload_secret` | `https://<domain>` (with `webapp_https_port`), `20971520`, generated | The HTTPS base of upload URLs (`/xmpp-upload/`), the largest upload, and the secret the URLs of chat files sent to XMPP clients are derived with |
| `xmpp_resume_seconds`, `xmpp_fast_token_days` | `300`, `14` | How long a dropped session waits for stream resumption, and how long a FAST token lasts |
| `xmpp_upload_seconds` | `300` | The longest a PUT to an upload slot may take |
| `xmpp_turn_enabled`, `xmpp_turn_per_hour` | `false`, `30` | External services (XEP-0215): TURN credentials for XMPP apps' calls, never to an external user, at most so many a user an hour |
| `xmpp_scram_decoy_key` | generated | Derives the SCRAM salts of names that are no user's, so they stay the same across restarts and nodes |
| `xmpp_s2s_port` | empty | Server to server (5269, federation, C3b): not built, nothing listens on it yet |
| `call_forward_max_hops`, `call_forward_ring_seconds` | `5`, `30` | The most forwards one phone call takes (a user's rules, a ring group's fallback; 1 to 10), and how long a forward to an outside number rings before the call goes on (10 to 120) ([SIP, Call handling](interfaces/SIP.md#call-handling)) |
| `call_queue_tick_seconds`, `call_transfer_ring_seconds` | `5`, `30` | How often the SIP engine checks each call waiting in a queue (offers, the wait limit, the caller's place; 1 to 30), and how long a blind transfer's target rings before the caller goes to their voicemail (10 to 120) ([SIP, Queues](interfaces/SIP.md#queues), [Transfer and merge](interfaces/SIP.md#transfer-and-merge)) |
| `call_hold_music_location`, `call_queue_prompt_location` | empty | Hold music for calls on hold and in queues without their own (WAV or Ogg Opus, looped; empty: a tone pattern; use only audio you may use), and a folder of `0.wav` to `9.wav` that speaks a queue caller's place (empty: pips) ([SIP, Queues](interfaces/SIP.md#queues)) |
| `sip_registrar_min_expires`, `sip_registrar_max_expires`, `sip_registrar_default_expires`, `sip_registrar_nat_expires` | `60`, `3600`, `600`, `60` | A phone binding's expiry in seconds: its bounds, when the phone asks none, and behind a NAT without Outbound ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `voicemail_max_seconds`, `voicemail_trash_days` | `120`, `30` | The longest voicemail message (10 to 600), and how long a deleted one waits in the trash before it and its audio go (0 keeps them) ([Voicemail](interfaces/Voicemail.md)) |
| `voicemail_prompt_location` | empty | A folder (any resource location) with the recorded speech played after the voicemail tones: `greeting.wav`, `enter-box.wav`, `enter-pin.wav`, `pin-refused.wav`, `menu.wav`, `no-messages.wav`, `goodbye.wav`; empty plays tones only ([Voicemail, Prompts](interfaces/Voicemail.md#prompts)) |
| `voicemail_pin_min_digits` | `6` | The fewest digits a new voicemail PIN has (4 to 12); four fall to the box's limit of failed tries in weeks ([Voicemail](interfaces/Voicemail.md#phone-access)) |
| `voicemail_transcribe`, `voicemail_transcribe_language` | `false`, empty | `true` sends each voicemail to the speech provider (the model bound to `transcribe`) for its transcript, by the deployment's recording and transcription policy; the language as an ISO 639-1 code |
| `node_role` | empty | `front_edge` makes this an edge node, which takes links from paired local nodes on its HTTPS listener and has both option sets, the front (web access, ports, balancing) and the edge (delegate, queue, replicate, mail, chat, calls, SIP, PASS) ([Edge node](#a-local-server-with-an-edge-node)); `front` has the front set alone and `edge` the edge set alone, for a front node and the edge nodes behind it (E19, [Edge-Node2 §4.3](../plans/Edge-Node2.md#43-the-front-option-set)); `all-in-one` makes a full server that also takes local nodes' links, routing each item by its owner ([All-in-one node](#an-all-in-one-node)). Another value is logged and means a normal node |
| `edge_link_key` | generated | The key an edge node's links use; generated on every node, used only on an edge node. Local nodes pin it, so a new one means each must pair again |
| `edge_node_url`, `edge_node_key`, `edge_node_pin` | empty | On a local node, its edge node: the address, this node's private key (a secret) and the pin of the edge node's link key. Pairing writes them to `runtime/.env` and unpairing removes them; they are not generated |
| `edge_calls_enabled` | empty (on) | On a paired local node, whether new calls have their media on the edge node's media node; `false` keeps its media row as draining, so new calls use this node's own media nodes and calls already on the edge node go on. The wizard's Apply writes it and unpairing removes it ([Edge node](#a-local-server-with-an-edge-node)) |
| `edge_sip_enabled` | empty (on) | On a paired local node, whether its trunks and phones go through the edge node's SIP engine; `false` sends the replicated sets `voiceTrunks` and `voiceFallbacks` empty, so the edge node registers no trunk and answers no number from a fallback, and keeps its `SipNode` row but gives it no new call. The wizard's Apply writes it and unpairing removes it ([SIP](interfaces/SIP.md#behind-an-edge-node)) |
| `edge_ssh_tunnel_port`, `edge_ssh_tunnel_host` | empty | On an edge node, a port whose connections go to the SSH server of the one paired local node (refused with several, and while it is away), and the address it listens on (empty for every one). With several local nodes each gets a port of its own: a `moqui.edge.EdgePort` row (System › Edge Clients › Ports), on `edge_ssh_tunnel_host` |
| `edge_xmpp_port`, `edge_xmpp_tls_port`, `edge_xmpp_host` | empty | On an edge node, the ports for every local node's XMPP domain, STARTTLS (5222) and direct TLS (5223), and the address they listen on (empty for every one): each connection goes to the node whose domain it names. A node's own pair of ports is an `EdgePort` row each |
| `edge_call_legs_per_pairing` | 200 | On an edge node, the most call legs of one local node on its media node at once; 0 for no cap |
| `edge_offline_page` | empty | On an edge node, the location of the page a terminate host shows while its node's link is down; empty for the built-in one |
| `edge_public_addresses` | empty | On an edge node, its public IPv4 and IPv6 addresses, comma-separated, which it offers its local nodes for their A and AAAA records; empty, and the wizard expects the addresses of its MX host name |
| `edge_mail_mta` | `moqui` in `conf/edge/MoquiEdgeConf.xml`, else empty | On an edge node, what delivers the local node's mail out: `moqui` (to each domain's MX), or empty or `postfix` (relayed to the MTA beside it, E5a) |
| `edge_mail_relay_host`, `edge_mail_relay_port` | `127.0.0.1`, `10025` | On an edge node with `edge_mail_mta=postfix`, the MTA beside it that mail the local node sends is relayed to (Postfix's relay port) |
| `edge_mail_bounce_domain` | empty | On an edge node, the domain of the envelope sender of mail it sends out, `bounce+<item>@` it, whose bounces the listener takes back to the message; empty sends from the message's own sender |
| `smtp_internet`, `mail_rspamd_url` | empty | `true` runs the SMTP listener in internet mode (port 25; `true` in `conf/edge/MoquiEdgeConf.xml`), with Rspamd's HTTP API for its spam verdict when the URL is set ([Mail-Inbound](interfaces/Mail-Inbound.md#mail-in)) |
| `mail_dkim_key`, `mail_dkim_selector`, `mail_dkim_domains` | generated, `moqui`, empty | The node's DKIM key (base64 PKCS#8, or PEM; RSA-2048 made into `runtime/.env` the first time a server needs it), its selector, and the domains signed for (empty: the hosted accounts' domains). `get#DkimRecord` gives the TXT record. A send by SMTP is signed only when the key is set or the send goes to this node's own listener |
| `mail_arc_trusted_sealers` | empty | Domains whose passing ARC seal overrides a DMARC reject or quarantine on port 25; empty for no ARC exception |
| `mail_delivery_retry_seconds`, `mail_delivery_days` | `300`, `5` | On an edge node with the MTA in moqui, the first wait before a delivery is tried again (doubled each time, to 4 hours), and when a message not delivered goes back as a bounce |
| `mail_mx_override`, `mail_dns_offline` | empty | Testing and staging only: `domain=host:port` pairs (`*` for any) used in place of the MX, and `true` for no DNS query |
| `moqui_log_level`, `moqui_log_dir` | `info`, `runtime://log` | The console level, and the directory of `moqui.log` and `moqui-error.log` |
| `search_backend` | `postgres` | `postgres` or `opensearch` ([search](#search)) |
| `example_disabled` | `false` | `true` leaves out the example component and its gallery on a dev instance |
| `demo_disabled` | `true` | `false` loads the demo lock-down on a dev instance; a demo server names the component in its conf instead |
| `llm_openai_url`, `llm_openai_api_key`, `llm_openai_model` | `https://api.openai.com`, empty, empty | The default model endpoint for Assist and the LLM services (the dev conf points it at `http://127.0.0.1:1234`) |
| `llm_anthropic_api_key`, `llm_dev_model` | empty, `default` | Dev conf only: the Claude API key of its `claude-haiku` model (Claude Haiku 5.5), and the model id its features (Assist, titles, summaries, classify, AI fields) are bound to; `llm_dev_model=claude-haiku` in `runtime/.env` puts Assist on Claude. The startup report's LLM line names the model Assist is on |

The database properties are in [Database and search](#database-and-search), and the cluster properties in
[Cluster](#cluster).

### Resource watch

The server reads what the machine and its own process have left: the CPU load, memory, the filesystems it writes to
(the runtime directory, its `db`, `log`, `tmp`, `txlog` and `opensearch` directories where they are mounted apart, and
the system temp directory), open files and swap. Each reading has a level, `ok`, `warning` or `critical`, from the
conf's `resource-watch` element:

| Reading | Attributes | Warning | Critical |
| --- | --- | --- | --- |
| A filesystem's space used | `disk-warning`, `disk-critical`, `disk-free-critical` | 80 % | 90 %, or under `disk-free-critical` bytes left (1 GiB) on a filesystem larger than that |
| A filesystem's file entries (inodes) used | `inodes-warning`, `inodes-critical` | 80 % | 90 % |
| The server's memory, of the memory it may use | `memory-warning`, `memory-critical` | 80 % | 90 % |
| The machine's memory still available | `memory-available-warning`, `memory-available-critical` | 10 % left | 5 % left |
| The 5-minute load, of the processors | `cpu-load-warning`, `cpu-load-critical` | 80 % | 100 % |
| Open files, of the soft limit | `open-files-warning`, `open-files-critical` | 80 % | 95 % |
| Swap in use | `swap-warning`, `swap-critical` | 50 % | none |
| The days until a certificate the HTTPS listener serves expires | `certificate-days-warning`, `certificate-days-critical` | 30 days left | 7 days left |

```xml
<resource-watch disk-warning="70" disk-critical="85" swap-warning=""/>
```

An empty value turns that level off; an attribute left out keeps its default, and a conf reload applies a change. The
memory the server may use is its container's or service unit's limit (the cgroup's `memory.max`) when one is set
below the machine's memory, else the machine's.

The levels are in the status map (`ec.factory.getStatusMap()`, its `Resources`), which the System app shows, and the
attention list of the System home names every reading that is not `ok`
(`moqui.tools.SystemHomeServices.get#SystemAttention`, beside the readiness checks that are not ready, which System ›
Operations › Health lists in full). With telemetry on, the readings are also recorded each minute
as gauges ([Monitoring](tools/Monitoring.md)), and four alert rule templates in the System folder of Dashboards ›
Alerts (low disk space, high memory use, high CPU load, a certificate expiring) send a notification once an
administrator saves a copy with its contact points. The certificates are those in `runtime/cert`, read again as the
readings are, so a renewed one is seen without a restart. Linux reports every reading; macOS the filesystems only; Windows none, and a reading that is not
reported has no level.

## Command line

`moqui` with no command starts the server. `moqui help` lists the commands, and `moqui help <command>` describes one.

| Command | What it does |
|---|---|
| `moqui [options]` | Start the server in the foreground |
| `moqui serve [options]` | Start it in the background. It returns once the server is listening and prints the address and process id; logs go only to the files. Stop it with SIGTERM |
| `moqui load [name=value ...]` | Load data files into the configured database, then exit |
| `moqui save-db <cluster-dir> <dump-file>` | `pg_dump` of an embedded cluster, usually `runtime/db/embedded` |
| `moqui load-save <cluster-dir> <dump-file>` | `pg_restore` into an embedded cluster; stop the server first |
| `moqui tui [--path P] [--user U]` | The apps in this terminal, on this runtime; `--server URL` reaches a running server instead ([TUI](ui/TUI.md)) |
| `moqui test`, `moqui test-client` | Component specs ([Testing](Build-Test.md#component-specs), [JS-Test-Client](dev/JS-Test-Client.md)) |
| `moqui test-load` | A component's load script against a running server ([Load-Runner](tools/Load-Runner.md)) |
| `moqui lint`, `check`, `refs`, `types`, `lsp` | Checks and tools for developers |
| `moqui version` | The version, package and features of this binary |

Server options:

| Option | What it does |
|---|---|
| `-p N`, `--port N` | The HTTP port (`webapp_http_port`) |
| `--https-port N` | The HTTPS listen port (`webapp_https_listen_port`), or `off` |
| `-c FILE`, `--conf FILE` | The runtime conf, as `MOQUI_CONF` does |
| `--no-run-es` | Do not start the OpenSearch in `runtime/opensearch` (`opensearch_run=false`) |
| `-D name=value` | Set a property for this run; repeatable |

The Java forms `port=N` and `conf=FILE` also work.

`./moq run`, `run-production`, `load` and the other tasks that run the binary pass the arguments after their own
options to it, so `./moq run lint --screens` is `moqui lint --screens` with the dev conf. Their package flags (`--lite`,
`--debug`, `--release`, `--enterprise`) pick the binary, and no flag means the Maximal package (the same as `--release`). They build first unless
given `--nobuild`.

`moqui load` arguments are `name=value` pairs. `./moq load` also takes bare words as data types.

| Argument | What it does |
|---|---|
| `types=a,b` | Data types to load. Omitted: seed, seed-initial and install. Empty or `all`: every type |
| `components=a,b` | Load only these components' data files |
| `location=FILE` | Load only this file |
| `timeout=SECONDS` | Transaction timeout of each file (default 600; each file is one transaction) |
| `raw=true` | For an empty database: no entity ECA rules, audit log, foreign keys or data feed |
| `disableEntityEca`, `disableAuditLog`, `disableFkCreate`, `disableDataFeed` | Each of those alone (`=true`) |

## Data and the first start

When a server starts on a database with no `moqui.basic.Enumeration` rows, it loads the data types in
`entity_empty_db_load` (default `seed,seed-initial,install`; `all` in the dev conf). It then starts its factory again,
and the log has:

```
Found 0 Enumeration records, loading empty-db-load data types (seed,seed-initial,install)
Data loaded into empty DB, re-initializing ExecutionContextFactory
```

Every start checks the tables. `entity_add_missing_startup` (default `true`) creates missing tables and adds missing
columns and indexes. Columns are never dropped or changed.

While the only accounts are the seeded ones nobody logs in as (`_NA_` and the agents' chat users), the login page
shows **Create Initial Admin Account**. The account it makes is in the `ADMIN` group. Once any person has an account,
the form is gone and the service refuses the call.

| Type | What it holds |
|---|---|
| `seed` | What the code needs: enumerations, status flows, security groups, jobs |
| `seed-initial` | Starting rows a deployment may change |
| `install` | Setup for a new instance |
| `demo` | Demo data and users (`john.doe`); never in production |
| others | Components add their own, such as `test` |

To load more on a server that already has data, stop it and run `moqui load types=...` (or `./moq load-production
...`). On embedded Postgres a second process cannot open the data directory while the server holds it.

By default an entity's create-or-update is one statement that inserts or updates (`INSERT ... ON CONFLICT (pk) DO
UPDATE`, on MySQL `ON DUPLICATE KEY UPDATE`) where nothing depends on which it does: no create or update ECA rule, no
audit-logged, create-only or defaulted field, no optimistic lock, and the user may both create and update the entity.
Otherwise it is UPDATE first, then INSERT. `-D create-or-update=true` switches to Moqui Java's read-first form
(SELECT, then INSERT or UPDATE). Where the row is looked for first, the INSERT does nothing when another transaction
has created the row since (`ON CONFLICT DO NOTHING`, on MySQL `ON DUPLICATE KEY UPDATE`), and that row is then updated.
Either way, concurrent calls for one new key do not fail on it
([Parity-Diffs](../plans/Parity-Diffs.md#atomic-create-or-update-2026-10-30)).

## Database and search

### Embedded Postgres

Without `entity_ds_db_conf` (or with Moqui Java's `h2`), the server runs its own Postgres. Every package but Enterprise
embeds it.

- **Data**: `runtime/db/embedded`, with two databases: `moqui` for the entities and `moqui_content` for the stored
  files ([Content database](#content-database)).
- **Binaries**: unpacked from the moqui binary on the first start, into `runtime/install/postgres` or under
  `MOQUI_INSTALL`.
- **Connections**: on Unix there is no TCP port. The server connects through a socket in the data directory, so
  nothing else on the network reaches the database.

### External Postgres or MySQL

Set `entity_ds_db_conf` and the other `entity_ds_*` properties, in the environment, `.env` or the runtime conf. There
are examples in `runtime/conf/MoquiDevConf.xml`. Use `mysql8` for MySQL 8 and `mysql` for MariaDB (which has no
`LATERAL`) and MySQL 5.7. Both create `utf8mb4` tables, so any character (an emoji) can be stored; create the database
with `CHARACTER SET utf8mb4`. A database made before, with 3-byte `utf8` tables, needs a fresh load or each table
converted by hand (`ALTER TABLE ... CONVERT TO CHARACTER SET utf8mb4`). Loading every data type into each was checked
([MySQL-Issues](../plans/archive/MySQL-Issues.md)).

| Property | Example | What it does |
|---|---|---|
| `entity_ds_db_conf` | `postgres` | `postgres`, `mysql` or `mysql8`; empty is embedded Postgres |
| `entity_ds_host` | `db.internal` | Default `127.0.0.1` |
| `entity_ds_port` | | Empty is 5432 or 3306 |
| `entity_ds_database` | `moqui` | |
| `entity_ds_schema` | | Empty for the default schema |
| `entity_ds_user`, `entity_ds_password` | `moqui`, ... | |
| `entity_ds_sslmode` | | Empty is verified TLS. `verify-full` checks the authority and the name, `verify-ca` the authority only, `require` encrypts without checking, and `disable` turns TLS off (for a local dev server only) |
| `entity_ds_sslrootcert` | `/etc/moqui/db-ca.pem` | A PEM file of the authorities to trust instead of the public ones |
| `entity_ds_crypt_pass` | generated | The key of encrypted fields; it must move with the database |

The content datasource has its own `content_ds_*` properties ([Content database](#content-database)).

Each connection to an external database sets the datasource's transaction isolation level for its session: the
`inline-jdbc` `isolation-level`, else the database's `default-isolation-level`, `ReadCommitted` for Postgres and MySQL
in `MoquiDefaultConf.xml`, as Moqui Java does. `None` leaves the server's default (InnoDB's is `REPEATABLE READ`). The
code assumes READ COMMITTED: at REPEATABLE READ or SERIALIZABLE, Postgres fails an `ON CONFLICT` insert whose
conflicting row the transaction's snapshot does not see, and MySQL takes gap locks that two creates of one missing row
can deadlock on. On Postgres a level other than `ReadCommitted` is logged as a warning at startup; the embedded database
does not set it and runs at READ COMMITTED.

### Content database

The stored bytes of files are in a database of their own, apart from the busy business rows. This is the `content`
entity group:

- `DbResource`, `DbResourceFile` and `DbResourceFileHistory`, behind `dbresource://`;
- the content blobs (`ContentBlob*`), which hold every file over 64 KiB;
- the objects, refs and LFS objects of `db` git repositories (`GitObject`, `GitRef`, `GitLfsObject`).

Mail attachments, chat files, call recordings, voicemail, drives, wiki pages and other content files are all
`dbresource://` files, so they are all in it. The rows that point at a file keep only its location.

Only the resource implementations (`dbresource://`, the blob store, git) read and write these entities. Code everywhere
else uses `ec.resource`. No foreign key or view-entity crosses between the content database and the main one.

| `content_ds_db_conf` | Where the content database is |
|---|---|
| empty, embedded transactional database (default) | the `content_ds_database` database (`moqui_content`) in the embedded cluster |
| empty, external transactional database | the transactional database itself, with no second database |
| `postgres`, `mysql` or `mysql8` | `content_ds_database` on that server |

For a server of its own, set `content_ds_host`, `content_ds_port`, `content_ds_user`, `content_ds_password`,
`content_ds_sslmode` and `content_ds_sslrootcert`. Each one left empty takes its `entity_ds_` value. So setting only
`content_ds_db_conf=postgres` puts the content in a `moqui_content` database on the transactional server, which must
exist. `content_ds_schema` names a schema, as `entity_ds_schema` does.

One transaction spans both databases, as Moqui Java's transaction spans every datasource, but without two-phase commit:

- **Commit order**: the content database commits first, then the transactional one. A failure between the two commits
  leaves a file that no row names (a blob of it goes in the blob cleanup), never a row naming a file that is not
  there.
- **Rollback**: a rollback, or an error in either database, rolls back both.
- **The edge case**: a file deleted in a transaction whose transactional commit then fails stays deleted.

Each transaction can hold a connection to each database, so the content pool is the size of the transactional one (90).
Its 82 ordinary connections (8 are reserve) hold the request and service workers' (16 + 22) × 2, one agent worker and a
connection for each streamed download reading a window at once (the `webroot` webapp's `transfer-workers`, 4): 81.
On an external Postgres the content shares the transactional database and its pool, so the server sees about 90
connections, within its default `max_connections` of 100 with a few left for other clients. When
the content shares an external transactional database, that pool holds the same 71. The embedded cluster makes room for
both pools. Startup refuses a pool too small for the workers, and warns about one too small for the downloads' reads,
which then read fewer at once. A cluster of servers shares the content database as it shares the
transactional one, so it needs both external.

### Search

Search runs on Postgres by default: tables in the `search` schema of the entity database, or a dedicated embedded
cluster when the entity database is not Postgres. OpenSearch is optional:

- set `search_backend=opensearch`, and `elasticsearch_url` (default `http://127.0.0.1:9200`), plus
  `elasticsearch_user`, `elasticsearch_password` and `elasticsearch_index_prefix` as needed;
- `./moq opensearch-download` installs OpenSearch in `runtime/opensearch`;
- the server starts that install and stops it on shutdown when `elasticsearch_url` is on this host, nothing answers
  there, and there is no pid file. `opensearch_run=false` (or `--no-run-es`) turns that off;
- `./moq opensearch-start` and `opensearch-stop` run it as a daemon with a pid file.

### Snapshots

`./moq save-db [name]` writes the embedded database to `runtime/save/<name>/moqui.dump`, and the content database beside
it to `moqui_content.dump`. It also writes the OpenSearch data when there is any. `./moq reload-save [name]` empties the
database and restores that save. `./moq load-save -n <name> [types]` runs `clean-all`, loads, and saves. Stop the server
before a restore. These work on the embedded database only; an external database has its own tools.

### Backups

The job `backup_EmbeddedDatabase_daily` backs up the embedded clusters while the server runs. It is paused in the seed
data; set it running on System › Operations › Jobs, or with `embedded_db_backup_scheduled=true`, which each boot applies
(`false` pauses it). It runs daily at 02:00 and keeps 7 daily, 5 weekly and 12 monthly archives, and every yearly one,
in `runtime/db/backup`, or the directory in `embedded_db_backup_dir`.

- **Modes**: `backupMode=dump`, the default, writes a `pg_dump` file of each database: `moqui-<stamp>-d.dump` and
  `moqui_content-<stamp>-d.dump`, with the same stamp. `incremental` takes `pg_basebackup` fulls and block
  incrementals of the whole cluster, both databases in one copy. Each incremental is taken from the generation's full;
  with `chainIncrementals=true` it is taken from the archive before it, which holds only a day's changed blocks.
- **Mirror**: set `systemMessageRemoteId` (the seed job's is `EmbeddedDbBackup`) and on that remote `sendUrl`, the
  user, and a password or key. The remote directory or key prefix is its message type's `sendPath`. Only the archives
  the remote lacks are sent; a promotion is a rename there, and an archive the ladder drops is deleted there. An
  incremental whose parent a promotion renamed is sent again (over plain SFTP only its `moqui-backup.txt`), so every
  copy names its parent as it is now called. With `incremental` and `chainIncrementals`, a night sends one
  incremental. Two job parameters shape it:
  - `mirrorTransport`: `sftp` or `s3`. Empty picks `s3` for a `sendUrl` of `s3://<bucket>`, else `sftp`. For S3 add
    `?region=<region>` (default `us-east-1`) and, for a store other than AWS (MinIO, R2, B2),
    `&endpoint=https://<host>`; the username is the access key, the password the secret key.
  - `mirrorEncrypt`: `true` seals each archive to `mirrorRecipient`, an [age](https://age-encryption.org) recipient
    (`age1...`, from `age-keygen`), before it leaves the server, which holds no key that reads it; keep the identity
    (`AGE-SECRET-KEY-1...`) off the server. Off by default.

  | | plain | encrypted |
  |---|---|---|
  | `sftp` | files and directories as they are | `<name>.age`, `<name>.tar.gz.age` |
  | `s3` | `<name>`, `<name>.tar.gz` | `<name>.age`, `<name>.tar.gz.age` |

  A `.tar.gz` holds the backup directory's contents at the top. Changing `mirrorEncrypt` sends every archive again in
  the new form and leaves the old-form objects alone.
- **Restore**: restore a dump with `moqui load-save` while the server is stopped. It also restores the content dump
  of the same name beside it (`moqui_content-<stamp>-d.dump` for `moqui-<stamp>-d.dump`). A physical backup is put
  together with `pg_combinebackup`: the full, then each incremental in order up to the one restored (each names its
  parent in `moqui-backup.txt`). An encrypted archive is decrypted first: `age -d -i key.txt <name>.age > <name>`, or for a directory
  `mkdir <name> && age -d -i key.txt <name>.tar.gz.age | tar xz -C <name>` (`tar xz` alone for a plain `.tar.gz`).
- **Not included**: `.env` and the field-encryption key are not in the archive. Keep them with the backup.

The details are in [plans/archive/PostgresBackup.md](../plans/archive/PostgresBackup.md). The job does not back up an
external database. `org.moqui.admin.BackupServices` reads the job's runs and the archive directory for an application's
backup screen (IronStone Admin › Data › Backups): coverage as the runs recorded it, the archives as names and sizes, and
a backup asked for now with the job's own parameters ([Attack-Surface](security/Attack-Surface.md)).

## HTTPS, ports and proxies

### Ports

| Port | Listener | Default | Properties |
|---|---|---|---|
| 8080 | HTTP | On, every address | `webapp_http_port`, `webapp_http_bind` |
| 8443 | HTTPS, certificates from `runtime/cert`; HTTP/2 or HTTP/1.1 by ALPN, and gRPC over HTTP/2 when a gRPC service is on | On when there is a certificate (dev makes one) or a name set up with ACME | `webapp_https_listen_port` (`off`), `webapp_http2` |
| 8444 | Cluster: TLS with the cluster certificate, client certificate required; gRPC on HTTP/2, or HTTP/1.1 | On in every serving process, every address | `cluster_https_port`, `cluster_bind_address`, `cluster_protocol` |
| `ssh_port` | SSH: SFTP, git, admin shell, the apps (`ssh -p 2222 john.doe@localhost` in dev) | Off; 2222 on 127.0.0.1 in the dev conf | `ssh_port`, `ssh_host` |
| `smtp_listen_port` | Inbound SMTP behind a mail gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)) | Off; Maximal and Enterprise only | `smtp_listen_port`, `smtp_listen_host` |
| `rtc_media_port` (UDP) | Call media: every browser leg of a call through the SFU ([WebRTC](interfaces/WebRTC.md)); with `rtc_cascade_enabled`, the relay links between the media nodes of a call too, so the nodes must reach each other's | Off; Maximal, Enterprise and Debug only | `rtc_media_port`, `rtc_media_host`, `rtc_public_address`, `rtc_cascade_enabled` |
| `rtc_media_tcp_port` (TCP) | ICE-TCP, for browsers that cannot reach the UDP media port ([TURN](interfaces/TURN.md)) | Off | `rtc_media_tcp_port` |
| `turn_port` (UDP and TCP), `turn_tls_port`, 443 on `turn_tls443_host`; `turn_relay_ports` (UDP) | The embedded TURN server and its relays ([TURN](interfaces/TURN.md)) | Off | `turn_port`, `turn_tls_port`, `turn_tls443_host`, `turn_host`, `turn_relay_ports` |
| `sip_port` (UDP and TCP), `sip_tls_port`; `sip_rtp_ports` (UDP) | SIP trunks, desk and soft phones (the registrar, with `sip_registrar_enabled`; `sip_tls_port` 5061 and `_sips._tcp` for remote phones), and the RTP of phone legs ([SIP](interfaces/SIP.md)) | Off; Maximal, Enterprise and Debug only | `sip_port`, `sip_host`, `sip_tls_port`, `sip_tls_host`, `sip_rtp_ports`, `sip_rtp_host`, `sip_public_address` |
| `xmpp_port` (5222), `xmpp_tls_port` (5223) | XMPP clients, STARTTLS and direct TLS with ALPN `xmpp-client` ([XMPP](interfaces/XMPP.md)); the WebSocket is on the HTTP listener (`/xmpp-websocket`) | Off; Maximal, Enterprise and Debug only | `xmpp_domain`, `xmpp_port`, `xmpp_host`, `xmpp_tls_port`, `xmpp_tls_host` |

XMPP clients find the server by DNS: `_xmpp-client._tcp.<domain>` (5222, STARTTLS) and `_xmpps-client._tcp.<domain>`
(5223, direct TLS, XEP-0368), both at the host's name; the certificates in `runtime/cert` must cover the domain,
`rooms.<domain>` and `upload.<domain>` (a wildcard of the domain does). `_xmpp-server._tcp` waits for federation (C3b).

`/health` answers `{"ready":true}` to loopback and to the addresses in `webapp_status_ips`, and 404 to everyone else.

The gRPC listener (4317, `grpc-port` of the telemetry `receiver` or `pass_authority_grpc_port`) and the edge tunnel
ports (`edge_ssh_tunnel_port`, `edge_xmpp_port`, `edge_xmpp_tls_port`) also open only when they are configured.

Before the database opens, a start checks every port it will bind, over TCP or UDP as the listener uses it, and logs
`Ports checked free: …`. If HTTP, HTTPS, cluster or SSH is busy, the start stops, and the error names each busy port
and the process that holds it. If one of the other listeners has a busy port, the start logs a warning and goes on
without that listener. When the server is ready it logs one report: the instance purpose, every listener and its
address (or why it is off, failed or skipped), every integration, the tool factories, and what the build leaves out.
The report is a warning when something that was configured did not start. The instance purpose also has a line of
its own once the properties are set (`Instance purpose dev (from the conf), node role all`).

### HTTP/2

The HTTPS listener offers HTTP/2 and HTTP/1.1 by ALPN, so browsers and other clients that speak HTTP/2 use it; the
plain HTTP listener is HTTP/1.1 only. `webapp_http2=false` keeps HTTPS to HTTP/1.1.

- **WebSockets** (`/notws`, `/callws`, `/termws`, `/xmpp-websocket`) open on an HTTP/2 connection by extended CONNECT
  (RFC 8441), or on a connection of their own over HTTP/1.1, as the client chooses.
- **gRPC.** When the OTLP/gRPC receiver or the PASS authority's gRPC is on, the HTTPS listener takes their calls over
  HTTP/2 too, so a client can use the HTTPS port instead of 4317; the gRPC listener stays.
- **Limits.** One HTTP/2 connection carries at most `webapp_http2_max_streams` (100) requests at once. One with no
  request for `webapp_header_timeout` (30 seconds) is closed with a GOAWAY, as an idle HTTP/1.1 connection is closed,
  and so is a client's connection when web protection refuses that client.
- **Slow readers.** A connection whose client takes nothing of a response for `webapp_write_timeout` (60 seconds) is
  closed, a WebSocket's included; a download that keeps moving is not. A WebSocket keeps its connection's place in
  `webapp_max_connections` and web protection's `max-per-client` until it closes.
- **Behind a proxy**, HTTP/2 is between the client and the proxy; the proxy may reach the server over HTTP/1.1 or, on
  the HTTPS port, over HTTP/2 (gRPC through a proxy needs HTTP/2 to the server).

### Certificates

Certificates are looked up by the requested domain in `runtime/cert` (git-ignored), in any of these layouts:

- nginx-proxy's `<domain>.crt` and `<domain>.key` (`dhparam.pem` files are ignored);
- certbot's or acme-companion's `<domain>/fullchain.pem` and `privkey.pem`;
- mkcert's `<domain>.pem` and `<domain>-key.pem`.

A subdomain falls back one label up only: first to `_wildcard.<parent>` (mkcert), then to `<parent>` (nginx-proxy).
After that, `default` serves any other name. A domain with no certificate gets a refused handshake. The files are read
again within seconds of a change, so a renewal needs no restart.

In dev (`instance_purpose=dev`), an empty directory gets a self-signed `localhost` certificate. Outside dev with no
certificate, the HTTPS listener does not start until a name is set up with ACME (below) or a certificate is added to
`runtime/cert` (seen within a minute). A `webapp_https_listen_port` set explicitly with no certificate no longer fails
the start: the listener waits the same way.

**System › Certificates** lists the names this node answers (its own host, its sites' hosts, the SMTP host, the XMPP
domain, an edge node's terminate hosts, the files in `runtime/cert`) and the certificate each is served: its issuer,
its expiry and its file. A name with none gets no HTTPS.

**From ACME.** Set up with ACME, on that screen, gets a name's certificate from an ACME authority: Let's Encrypt
(`acme_directory_url`), its staging directory for a first try, or any other authority's directory URL. Nothing is ever
asked for a name nobody set up there.

- **The challenge** is TLS-ALPN-01 on the HTTPS listener: the authority connects to the name on port 443, so 443 must
  reach the listener (a port forward to 8443, the systemd drop-in below, or an edge node). No port 80 is needed.
  Set up first checks that the name's DNS has an address and that 443 there reaches this node; a router that cannot
  connect to its own public address fails the check, and the screen offers to order without it. The check takes up
  to about 20 seconds, and the screen waits for it.
- **The first certificate.** A node with no certificate at all starts its HTTPS listener when a name is set up (on any
  member of its cluster), and for the length of a check. Until a certificate is there it answers only the challenges
  of names set up and refuses every other handshake. An edge node that started with no certificate takes no links
  either: restart it once it has one.
- **Behind an edge node.** For a passthrough host the challenge goes down the tunnel untouched, so the local node sets
  the name up on its own Certificates screen and gets its own certificate. For a terminate host the edge node sets it
  up on its own screen.
- **Files.** The certificate is written as `runtime/cert/<name>/fullchain.pem` and `privkey.pem` (certbot's layout),
  with a `.moqui-acme` marker; a `<name>/` directory another tool wrote is never written over. Another file pair for
  the name (`<name>.crt`) is served before it.
- **Renewal.** The `renew_AcmeCertificates_daily` job, paused until a name is set up, renews each name set up there
  when a third of its certificate's life is left. A failure shows on the screen and on the System home with the
  expiry. Stop renewing and Remove are on the screen.
- **Cluster.** One member orders a name at a time (a lock row whose 15-minute lease is by each member's clock, so the
  clocks must agree); the challenge and the certificate are rows, the key encrypted, so any member answers the
  challenge and every member writes its own files within a minute.
- **Account.** The account key is derived from `acme_account_key`, a secret generated into `runtime/.env`; the members
  of a cluster share it in their shared `.env`, or each is a separate account. For a company's own authority,
  `acme_directory_ca` names the root its HTTPS is checked against.
- **Not built:** HTTP-01 (port 80), DNS-01 and so wildcard names.

### Listen ports and URL ports

`webapp_https_listen_port` is the port the server binds. `webapp_https_port` is the port clients see in generated URLs
and https redirects; empty is 443, as in Moqui Java. To make generated URLs use https, set `webapp_https_enabled=true`.

| Clients reach the server | Set |
|---|---|
| Directly on 8443 | `webapp_https_enabled=true`, `webapp_https_port=8443` |
| On 443, through a port redirect or a proxy | `webapp_https_enabled=true`; leave `webapp_https_port` empty |
| On 443, which the server binds itself | `webapp_https_enabled=true`, `webapp_https_listen_port=443`, and the capability to bind it (below) |

A firewall redirect from 443 to 8443 (nftables or iptables) keeps the server unprivileged. To bind 443 directly under
systemd, add a drop-in `/etc/systemd/system/moqui.service.d/https-443.conf`:

```ini
[Service]
AmbientCapabilities=CAP_NET_BIND_SERVICE
Environment=webapp_https_listen_port=443
```

### Behind a proxy or load balancer

The server terminates HTTPS itself, so a proxy is optional. A proxy or load balancer in front needs these settings:

- `webapp_trusted_proxies` lists the proxies' address ranges (`10.0.0.0/8`, `fd00::/8`, or single addresses). Only
  those peers are believed for `X-Forwarded-Proto` and the client address header. The client address is then the
  rightmost address that is not a trusted proxy.
- `webapp_client_ip_header` names the header: `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP` or
  `CloudFront-Viewer-Address`.
- Setting `webapp_trusted_proxies` also turns on the per-client web protection: rate limits, connection limits and
  bans by client address. Without a proxy, `webapp_web_protection=true` turns it on.
- A `rate-limit` or a client, path or user-agent rule with `host="shop.example.com"` (or `*.example.com`) applies only
  to that site, on top of the rules without `host`, so one site can be stricter than the others.
- A ban is shared with the other cluster members and up the edge link: a ban a local node starts (a sign-in that
  keeps failing) is told to its edge node, which refuses that client for that node's host names only. An edge node
  tells the nodes behind it nothing, as its bans may come from another pairing's visitors. System › Security › Web
  Protection lists what each rule refused and the bans held now, and lifts one.
- Leave `webapp_https_port` empty when the proxy serves 443.

## One production server

### From an archive

`./moq archive` writes a zip, or with `--tgz` a gzipped tar. It expands to one directory, which is a runtime directory
with the binary in it.

- **What goes in**: the package's binary; `runtime/conf/MoquiProductionConf.xml` as `MoquiConf.xml` (`--conf FILE` puts
  in another, such as `conf/edge/MoquiEdgeConf.xml`); `README.txt`; and all of `runtime/` except `log/`, `tmp/`, `save/` and `install/`.
- **`--no-data`** leaves out the database, the OpenSearch data, `.env` and `cert/`, for a fresh instance or an upgrade.
  Without it the embedded database goes in as files, so stop the server first.
- **Platform**: the binary runs on the OS and CPU it was built for, so build on the server's platform.
- **By hand**: copy the binary into a runtime directory with a `MoquiConf.xml` in it, or set
  `MOQUI_CONF=conf/MoquiProductionConf.xml`. From a checkout, `./moq run-production` does the same.

`systemd/moqui.service` runs `/opt/moqui/moqui` as the `moqui` user.

- It is `Type=notify`: the server reports when the listener is bound and when it stops. An in-app restart runs the
  binary again in the same process and reports a reload, so the unit stays up.
- `/etc/moqui/environment` is optional, for variables that must exist before the process starts (`MOQUI_CONF`,
  `MOQUI_RUNTIME`). The server reads `/opt/moqui/.env` itself.
- For a database on the same machine, `systemd/drop-in/` has `After=` and `Wants=` drop-ins for Postgres and MySQL.

### Podman and Quadlet

`podman/build.sh` builds the Maximal package and the image `localhost/moqui:latest`, with whatever components are in
`runtime/components/addon`. One volume at `/var/lib/moqui` holds the database, logs, certificates and generated secrets.
There are compose stacks for embedded Postgres, an external Postgres, Postgres with OpenSearch, and MySQL with
OpenSearch, and `podman/quadlet/` runs the single container as a systemd service. Everything is in
[podman/README.md](../podman/README.md).

### Upgrading

1. Back up. For embedded Postgres, the backup job's latest archive, or a dump taken with the server stopped:

   ```sh
   sudo systemctl stop moqui
   sudo -u moqui mkdir -p /opt/moqui/save
   sudo -u moqui /opt/moqui/moqui save-db /opt/moqui/db/embedded /opt/moqui/save/before-upgrade.dump
   ```

   For an external database, use its own backup.
2. On the build machine, run `./moq git-pull`, then `./moq archive --release --no-data`.
3. On the server, remove the old `moqui` binary and `components/`, so files deleted upstream do not linger, and unpack
   the new archive over `/opt/moqui`:

   ```sh
   sudo rm -rf /opt/moqui/moqui /opt/moqui/components
   sudo cp -a mr-moqui-20261105/. /opt/moqui/ && sudo chown -R moqui:moqui /opt/moqui
   ```

   `db/`, `.env`, `cert/` and `log/` are not in the archive, so they stay. `MoquiConf.xml` is replaced, which is why
   per-host settings belong in `.env`.
4. Start the server: `sudo systemctl start moqui`. The table check adds new tables and columns. New seed data is not
   loaded on its own. To load it, stop the server, run `cd /opt/moqui && sudo -u moqui ./moqui load types=seed`, and start it
   again. Leave out `seed-initial`, whose rows the deployment may have changed since.

A deploy marker for the dashboards can be posted from the upgrade script ([Monitoring](tools/Monitoring.md#deploy-markers-and-annotations)).

### Optional services

| Service | How |
|---|---|
| PDF | Chromium: `./moq chromium-download` into `runtime/chromium`, or `moqui_chromium_path`. The server starts without it, and PDF is unavailable |
| OpenSearch | [Search](#search) |
| Outbound mail | SMTP through a `moqui.basic.email.EmailServer` record; every package has it |
| Inbound mail | IMAP polling, or the SMTP listener behind a gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)); Maximal and Enterprise |
| SSH | `ssh_port`, with `ssh_host=0.0.0.0` to reach it from other machines ([TUI](ui/TUI.md), [Sync](interfaces/Sync.md), [Git-Hosting](interfaces/Git-Hosting.md)) |

## Several sites by host name

One server can answer several host names with a different root screen for each, as Moqui Java does: separate sites
on sub-domains or related domains, such as a forum and a shop beside the business's own apps. It is not the default:
every host gets `webroot.xml`, the apps on paths under it, until a `root-screen` names another root for a host.
`conf/MoquiSitesConf.xml` is a working example (it uses mr-pass for the two public hosts).

```xml
<webapp name="webroot">
    <root-screen host="forum\.example\.org" location="component://mr-pass/screen/Pass.xml" framework-routes="refuse"/>
    <root-screen host="(www\.)?example\.com" location="component://example-site/screen/ExampleSite.xml" framework-routes="include"/>
</webapp>
```

- **Host.** A regular expression matched against the whole `Host` header, without its port, in lower case. The first
  that matches wins; `.*` (`webroot.xml`, in MoquiDefaultConf.xml) is the fallback.
- **The root screen** is a thin wrapper: a home page and the pages of its site, or one app at its root. Files beside it
  (`ExampleSite/` beside `ExampleSite.xml`) are served as `webroot/` is for `webroot.xml`. The shared `/js`, `/css`,
  `/libs` and `/images` come from `webroot.xml` when the wrapper's directory has no such file, and so do the error
  screens.
- **`framework-routes`** says what else the host gets. A conf whose `root-screen` patterns cannot be read (a pattern
  that is not a valid regex) refuses the framework's routes on every host until it is fixed:

  | Value | The host serves |
  |---|---|
  | not set | The root's own screens and the shared static files. The token routes, the servlets and the sockets answer as on every host (Moqui Java's behaviour) |
  | `include` | The root's own screens first, then `webroot.xml`'s for a first path name the root does not have: the shell (`/qapps`), sign-in, the apps, `/rest`, the PWA files and Web Push |
  | `refuse` | The root's own screens and the shared static files only. `/Login`, `/apps`, `/qapps`, `/rest`, the token routes, the servlets, the sockets, `/dav`, `/health`, `/metrics` and the gRPC services of the HTTPS listener are Not Found there, so nobody signs in to the framework on the host. Git at a public site's page URL is the one exception, on a host that is also a `public-site`'s. Use it for a public site |

- **Links.** A link made in a request to such a host keeps that host, with the webapp's scheme and port: only a host a
  `root-screen` pattern names counts, as a plain host name, never another `Host` header. On a `refuse` host it is
  `webapp_http_host`'s, as a link made outside a request (mail, a report, a job) always is: such links are often to the
  apps, which that host does not serve.
- **Sessions.** The session cookie is tied to its host, with no cookie domain: a person signs in to each host. A
  deployment's own sites share accounts through SSO; mr-pass's Account does that for PASS sites. A server that is the
  sign-in provider for its own other hosts is later work.
- **Per origin.** Each host is its own origin: CORS, the service worker, the PWA manifest and Web Push subscriptions
  are each host's.
- **Public sites.** An mr-pass host has its `public-site` (the site id and the app at `/`) and a `root-screen` naming
  the same `Pass.xml` with `framework-routes="refuse"`. The site id is bound by the conf, never by a request
  ([mr-pass Deployment](../runtime/components/addon/mr-pass/docs/Deployment.md)).
- **Each host** needs a DNS record and a certificate for its name ([Certificates](#certificates)). Behind an edge node
  each is an `EdgeHost` of the pairing, added on the local node's System › Edge Node, passthrough or terminate
  ([A local server with an edge node](#a-local-server-with-an-edge-node)).

## Public demo server

A server where anyone can try the apps with the `john.doe` / `moqui` login. It runs on the production purpose with
`conf/MoquiDemoConf.xml`, which loads the demo component (`runtime/components/dev/demo`) and loads all data, demo users
included, on the first start:

```sh
./moq load --release -c conf/MoquiDemoConf.xml
./moq save-db --release demo                    # the database to go back to
./moq run-production --release -c conf/MoquiDemoConf.xml
```

The component keeps the apps writable and takes away running code or SQL, bulk import and export, the file manager,
writes in Tools and System, SSH, WebDAV, MCP, A2A, the LLM gateway, mail to other hosts, and changes that would lock
visitors out of the shared login ([its README](../runtime/components/dev/demo/README.md)). Restore the saved database on
a schedule with `./moq reload-save --release demo`, with the server stopped. Do not run a demo as `instance_purpose=dev`:
the Dev AIDE loads there.

## A local server with an edge node

An edge node is a small server in the cloud in front of a local server, which dials out to it and keeps one link
open ([plan](../plans/archive/Edge-Node.md)). The local server needs no open port, port forward or fixed address. Through
the edge node go: the local node's apps and its SSH server, mail (with the MTA in moqui on port 25, or Postfix and
Rspamd beside the edge node), chat providers' webhooks and MMS files, and mr-pass's public sites, which call the local
node as their PASS authority, the media of calls, with TURN, whose recordings come back to the local node, XMPP
clients through its tunnel ports, and phone calls (SIP trunks and outside phones), with a voicemail fallback while the
local node is away.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, edge_ssh_tunnel_port
 edge node      conf/edge/MoquiEdgeConf.xml    base components and mr-pass; the link's queue and copies
    ^  the local node dials 443 and keeps one link open
 local node     conf/edge/MoquiLocalConf.xml   the apps and the data, in the office or at home
```

The confs, the `.env` names each node needs and the commands are in
[runtime/conf/edge](../runtime/conf/edge/README.md); each conf's comments say what each setting does. The edge node
runs the Maximal package, or Enterprise with an external database (both have the link), with the base components and
mr-pass (`./moq component-get edge`). Its HTTPS listener needs a WebPKI certificate ([Certificates](#certificates)),
which local nodes check when they pair, and no TLS-terminating proxy in front: the link needs ALPN and client
certificates end to end. If a proxy is in front of it, name it in `webapp_trusted_proxies`; on an edge node a
client-IP header with no trusted proxies is ignored.

The browser terminal goes through too, in both web access modes: a client of the edge node is another machine to the
local node, so a dev local node's command line and shells need `terminal_remote=true` and a recent sign-in there, and a
dropped link resumes each terminal by the count of what it drew ([Terminal](ui/Terminal.md#the-desktop-and-mobile-apps-and-phones)).

**Attach a local node in one sitting.**
1. Set up the edge node from its README: its `.env`, a certificate, and a start with `conf/edge/MoquiEdgeConf.xml`. In
   its System › Edge Clients › Clients, make a pairing code. Clients also lists the paired nodes, pings and revokes
   them, and shows the edge node's link key fingerprint; Domains has their host names and what each claims, and Ports
   their tunnel ports.
2. On the local node, in System › Edge Node, choose **Pair with an edge node**. Pairing writes `edge_node_url`,
   `edge_node_key`, `edge_node_pin` and `edge_node_id` to `runtime/.env` and brings the link up; unpairing removes them.
   The wizard's steps:
   1. **Edge node.** Its address and the pairing code.
   2. **Connect.** Check its name and link key fingerprint against Edge Clients. Next pairs and waits for the link.
   3. **Discover.** What the edge node offers, as a form with each area on or off and this node's choices filled in:
      host names (public DNS names, passthrough or terminate), mail domains and which mail goes out through the edge
      node, chat bridges, PASS sites, XMPP, calls and phones.
   4. **Apply.** The whole setup is checked first, and nothing changes when any of it is not right. The edge node's
      part goes first: it takes only this node's names, and refuses a name another node holds and its own names. Then
      this node's part, listed in [`MoquiLocalConf.xml`](../runtime/conf/edge/MoquiLocalConf.xml)'s comments; the host
      or server each mail row had is kept (`edgePriorSmtpHost`, `edgePriorServerId`), and unpairing puts it back.
      Every replicated set is sent whole again. An area turned off is left as it is. Then the DNS records to set, each
      looked up live (Check again looks again).
   5. **Test.** Each area a pass, fail or not-checked line, with round trips on the queue and no outside mail or
      provider: the link, mail in (the edge node takes mail for a hosted account's address), mail out (the system's
      and the accounts' servers are on `edge:`, and the edge node can send: its DKIM key, or its Postfix relay
      answering), the webhooks, PASS, XMPP, calls and phones (the last three as their guides say). Test again runs it again; Continue anyway goes on with a failure; Finish shows the status.
3. Set each chat provider's webhook URL to `<the edge node's address>/channels/<chatBridgeId>/webhook`.

The status screen shows what is set up, with Set up again, which opens the wizard at Discover. Edge Clients › Domains
shows what each node's setup sent. The offer is read fresh each time; a page waits at most 3 s for it, and says when the edge
node is not answering. The area names in the edge node's Hello are
read at its start, so restart it after changing its mail, chat, PASS or XMPP settings. A name the operator adds by
hand on Edge Clients is not checked against the edge node's own names.

**Web access.** Point each host name's DNS at the edge node; in the office's DNS the same name can point at the local
node. A name is **passthrough**, the default (the edge node sends the connection down the link before any TLS, and the
local node ends the TLS with its own certificate, so the edge node sees no content), or **terminate** (the edge node
ends the TLS, runs its web protection, and shows an offline page while the link is down). The apps, `/notws` and the
other sockets, DAV and git over HTTPS work in both, and the local node's logs and web protection see the client's
address.

**Each area**, with its settings in the confs' comments and its detail in its guide:

| Area | Guide |
|---|---|
| Mail in and out, the DNS to publish, Postfix and Rspamd beside the edge node | [Mail-Inbound](interfaces/Mail-Inbound.md#the-edge-node-as-the-gateway) |
| Chat webhooks and MMS | [SMS](interfaces/SMS.md), [Chat-Bridges](interfaces/Chat-Bridges.md) |
| PASS sites, with the local node as their authority | mr-pass [docs/Deployment.md](../runtime/components/addon/mr-pass/docs/Deployment.md), An edge node as the PASS server |
| Calls: the media node and TURN; recordings | [WebRTC](interfaces/WebRTC.md#the-edge-node-as-media-node), [TURN](interfaces/TURN.md#on-an-edge-node), [Call-Recording](interfaces/Call-Recording.md) |
| Phones: trunks, outside phones, the fallback while the local node is away | [SIP](interfaces/SIP.md#behind-an-edge-node), [Voicemail](interfaces/Voicemail.md#while-the-local-node-is-away) |
| XMPP clients through the tunnel ports | [XMPP](interfaces/XMPP.md#behind-an-edge-node) |
| SSH through the tunnel port | [`MoquiEdgeConf.xml`](../runtime/conf/edge/MoquiEdgeConf.xml)'s comments; [Attack-Surface](security/Attack-Surface.md#edge-node), SSH tunnel port |
| Work over the link: delegated services, the queue, replicated sets | the `<edge>` element in `MoquiEdgeConf.xml` and `MoquiDefaultConf.xml`; [plan](../plans/archive/Edge-Node.md) §5.2 |

System › Edge Node shows the outbox and the replicated sets, with Pause and Resync; Edge Clients shows each node's
queue and sets. The edge node's database holds the queue, and its conf sets the backup job running
(`embedded_db_backup_scheduled`).

**A cluster or a standby behind it.** Every member of a local cluster links on its own, and the edge node pools and
balances them ([A local cluster with an edge node](#a-local-cluster-with-an-edge-node)). A second machine started
when the first one dies takes over the pairing ([A local node with a standby](#a-local-node-with-a-standby)).

**Several edge nodes.** The edge node can be a cluster, each local node linked to every member ([An edge
cluster](#an-edge-cluster)).

**Not built** ([Edge-Node2](../plans/Edge-Node2.md#22-not-built)): a PASS read cache on the edge node, and monitoring
over the link.

## An all-in-one node

A full server, with its own users, apps, data, mail domains, chat bridges, calls and XMPP domain, can also be the edge
node of one local node, such as a desktop node ([plan](../plans/archive/Edge-Node.md#10-all-in-one-node-e12) §10). The local
node pairs and runs the wizard as with a dedicated edge node (above), and sees no difference.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223 (its own XMPP), the local node's XMPP tunnel ports
 all-in-one node   conf/all-in-one/MoquiAllInOneConf.xml   its own apps and data; the local node's queue and copies
    ^  the local node dials 443 and keeps one link open
 local node        conf/edge/MoquiLocalConf.xml            the apps and the data
```

- **Conf.** Run `conf/all-in-one/MoquiAllInOneConf.xml` (`node_role=all-in-one`) with the server's full runtime, on
  the Maximal package. The `.env` names and the commands are in
  [runtime/conf/all-in-one](../runtime/conf/all-in-one/README.md). It needs what an edge node needs: a WebPKI
  certificate on the HTTPS listener, `edge_public_addresses`, and the edge node's mail, calls and SIP properties in
  `.env`. It takes the link and never dials an edge node itself. Its conf sets the backup job running: the queue is in
  the server's database.
- **Each item has one owner.** The server's own host names, mail accounts, chat bridges, numbers, calls and XMPP
  domain are served here as on a normal node; the local node's `EdgeHost` names, recipients, bridges, numbers and calls
  go over the link. A mail domain may be split by address.
- **Claims both sides make.** The server's own items win. The wizard's Configure refuses the local node's bridges,
  XMPP domain, SIP domain, numbers and a mail domain the server holds whole (its `edge_mail_bounce_domain`). A
  replicated record that claims a server's item is not applied: System › Edge Clients › Domains lists it under Replica conflicts,
  with Check again. The server's own setup refuses an address, bridge or number the local node holds. An address both
  sides hold gets 451 at `RCPT TO`, and a bridge both hold gets 409 at its webhook; each is logged.
- **XMPP.** The server's XMPP server keeps `xmpp_port` and `xmpp_tls_port`. Give the local node's domain tunnel ports
  of their own, such as `edge_xmpp_port=5242` and `edge_xmpp_tls_port=5243`, named in that domain's SRV records. A
  tunnel port the same as the server's own is off, with an error logged.
- **PASS.** The server's mr-pass keeps its local authority (`pass_backend_url` is not `edge:`). PASS sites for the
  local node are not served here yet.
- **Limits.** One paired node. Its load shares the server's CPU, bandwidth and media ports. The queue is in the
  server's database, so the server's backups cover it. Unpairing leaves the server's own services as they were.

A clustered all-in-one server takes links on every member, as an edge cluster does ([An edge
cluster](#an-edge-cluster)). Several paired nodes: [Several local nodes on one edge node](#several-local-nodes-on-one-edge-node).

## Several local nodes on one edge node

One edge node can serve several local nodes, each on its own database with its own domains: branch offices, or the
separate organizations of one group ([plan](../plans/Edge-Node2.md#41-domains-on-one-edge-node) §4.1). Each local node
pairs and runs the wizard as it would alone; each is a pairing on the edge node, and the edge node routes each name,
address, number, bridge, domain and site to the pairing that owns it.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, a tunnel port per local node for SSH
 edge node      conf/multi-domain/MoquiEdgeConf.xml    the links' queues and copies, one pairing per local node
    ^  each local node dials 443 and keeps its own link open
 local node A   conf/multi-domain/MoquiLocalConf.xml   its apps and data, with its own domains
 local node B   conf/multi-domain/MoquiLocalConf.xml   its apps and data, with its own domains
```

The confs, the `.env` names and the commands are in [runtime/conf/multi-domain](../runtime/conf/multi-domain/README.md).

1. Set up the edge node as for one local node, from `conf/multi-domain/MoquiEdgeConf.xml`.
2. For each local node: make a pairing code on the edge node (System › Edge Clients › Clients), and pair and run the
   wizard on the local node. Give each local node its own `xmpp_domain` and `sip_domain` first.
3. Check Edge Clients › Domains: each local node's host names, mail domains, XMPP domain, SIP realm and numbers, chat
   bridges and PASS sites, and any record refused because another node claimed its item.
4. For SSH, give each local node a tunnel port of its own on Edge Clients › Ports.

- **The first claim holds.** A host name, XMPP domain, SIP realm or PASS site another local node named first is refused
  in the wizard's Apply ("another node has this name"), and on Edge Clients › Domains for a host name added by hand.
  A replicated mail address or number another local node holds is kept as a replica conflict, shown on Domains with
  Check again; an address two nodes both have gets 451 at `RCPT TO` until one lets it go. Chat bridge ids may be the
  same on two nodes: a webhook goes by the request's host name first.
- **Each area by its owner.** Web access by host name; mail in by the recipient's address, the most specific match
  first; chat webhooks by the bridge within the host name; PASS by the site a local node's setup names; XMPP by the
  domain a client names, on one pair of ports; SIP by the number dialled and the phone's realm; calls by the room's
  node, with a TURN secret per pairing; delegated calls and the queue by the owner of the item. A call with no owner is
  refused while more than one local node is paired.
- **Limits per local node.** The queue's size cap holds for each local node alone, and a local node's own cap is set
  on Edge Clients › Clients (Queue cap). Mail delivery gives each local node with mail due an even share of its
  sessions. Call legs have a cap per local node (`edge_call_legs_per_pairing`).
- **Isolation.** Reasonable, not multi-tenant: one operator runs the edge node, and the local nodes belong to one
  organization or to related ones. Shared: the edge node's addresses and mail reputation, its DKIM key (published in
  each domain), its media and TURN ports, web protection's counts, CPU and bandwidth. Revoking a local node ends its
  link and its TURN access.

**Not built** ([Edge-Node2](../plans/Edge-Node2.md#22-not-built)): an operator per local node with a view of its own.
A local node may be a cluster: its members are one pairing ([A local cluster with an edge
node](#a-local-cluster-with-an-edge-node)).

## A local cluster with an edge node

An edge node can be the load balancer of a local cluster ([plan](../plans/Edge-Node2.md#44-pools-load-balancing)
§4.4). The cluster pairs once; every member then links on its own with the pairing's key, and the edge node pools the
members linked for the pairing and balances its names over the live ones. The members need no load balancer of your
own, and no open port, port forward or fixed address.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, ...
 edge node     conf/edge/MoquiEdgeConf.xml                      pools the pairing's members; its queue and copies
    ^  each member dials 443 and keeps its own link open
 member 1   member 2   member 3     conf/local-cluster/MoquiPooledConf.xml; 8444 between members only
    +----------+----------+
       shared Postgres
```

The confs, the `.env` names and the commands are in [runtime/conf/local-cluster](../runtime/conf/local-cluster/README.md).
The cluster itself is as in [Cluster](#cluster): one external database, the same secrets, a distinct instance id.

1. Set up the edge node as for one local node. Start the first member, and pair it and run the wizard (System › Edge
   Node) as for one local node.
2. Copy its whole `.env`, with what pairing wrote, to each other member, add that member's own `cluster_base_url`,
   and start it. Never copy `runtime/db`: each member keeps its own instance id and member key there.
3. Check System › Edge Clients › Clients on the edge node: each member under Members, linked and live.

- **Joining.** While a member is linked, a new one is admitted only when a linked member's cluster lists it as live:
  a member joins the cluster first, then links. Anything else with the key, such as a staging copy on another
  database, is refused "join the cluster first" and shown under Members. With none linked the key admits, which is
  how a pool starts and how it comes back after every member was down. Each member's own key is pinned for its id; a
  member made again with a new key is refused until its key is forgotten (Members, Forget key).
- **Members.** Clients shows each member: linked and live in its cluster; live but not linked, a fault to look at;
  linked with a stale heartbeat; on another version than most of the pool; stopped; refused. Each member's own System
  › Edge Node shows its cluster as its links tell the edge node.
- **Draining and rolling upgrades.** Drain a member before stopping or upgrading it, on its own System › Edge Node or
  on the edge node's Members: it finishes what it has, gets no new work, and its `/health` answers 503. A member also
  says when it is starting or stopping. Drain, stop, upgrade, start, resume, one member at a time.
- **Work from the pool**, replication and shared bans: [Cluster, What is shared and what is
  not](#what-is-shared-and-what-is-not), With an edge node.
- **Drain the last live member** only to take the pairing offline: its names get the offline page, or a refused
  connection, until it resumes.

**Balancing.** The edge node spreads a pairing's work over its members that are linked and live, each kind its own way:

| Traffic | To the member |
|---|---|
| A terminate host's requests | Of the session cookie (`moqui.session`, or the host's own, set on Edge Clients › Domains), by rendezvous hashing over the live members, so a browser stays on one member and every edge node picks the same; with no cookie, in turn. A WebSocket as its opening request |
| A passthrough host's connections, SSH and XMPP tunnels | Of the client's address, an IPv6 address by its /64 |
| PASS authority calls | Of the session handle while a sign-in is in progress, else the one with the fewest calls in flight |
| Delegated calls | Of the call's conversation (its call session, else its phone call), so one call's events reach one member |
| Call signaling | The member whose socket bound the leg |

A member whose requests fail or time out three times in a row is set aside for 30 seconds, then tried again; with every
member set aside they are all tried. A request that fails before any byte of an answer goes once more to another
member when it is a GET or HEAD with no body, or when it was never sent; nothing else is sent twice. A member that
stops moves only its own clients, and they stay signed in: sessions are rows in the pool's database. The defaults are
the `<edge>` element's `sticky-cookie`, `set-aside-failures` and `set-aside-seconds`; Edge Clients › Balancing shows them
with each member's requests in flight and in all and whether it is set aside (`get#EdgeBalancing`), and the `moqui.edgelink.requests`,
`retries` and `set_asides` counters name the pairing and the member.

## A local node with a standby

A local node paired with an edge node can have a hot standby: a second machine, started when the first one dies, that
takes its place at the edge node ([plan](../plans/Edge-Node2.md#45-failover-to-a-standby) §4.5). Nothing starts it by
itself: a person does, or automation the operator adds, such as a monitor of the first machine.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, ...
 edge node        conf/edge/MoquiEdgeConf.xml                 holds the queue and mail while no machine is linked
    ^  one machine at a time dials 443
 first machine    conf/local-failover/MoquiLocalConf.xml      the apps and the data
 standby          conf/local-failover/MoquiLocalConf.xml      started when the first machine dies
```

The conf, the `.env` names and the commands are in
[runtime/conf/local-failover](../runtime/conf/local-failover/README.md).

- **The standby** has the same runtime and `.env` (the pairing's key, the edge node's address and pin, the crypt pass)
  and an instance id and member key of its own (never copy `runtime/db`). Its data is one of: the same external
  database; a replica that is promoted before it starts; or the last backup restored (`moqui load-save`), mirrored off
  the first machine ([Backups](#backups)).
- **Taking over.** With none of the pairing's members linked, the edge node admits the standby: it gets every name of
  the pairing, replication starts with full sends, and the queue delivers. The takeover is noted on the pairing
  (System › Edge Clients › Clients, Takeovers) and sent as a notification to the edge node's administrators. To have
  a new node wait for an administrator's approval instead, set New nodes to Held for approval there; Approve is then
  on its row under Members.
- **After a restore from a backup**, the standby lacks what was delivered since. On Clients use Deliver again with
  the backup's time: the edge node keeps delivered items for the queue's retention period (`queue-retention-days`),
  and the receipts make an item the standby already has do nothing.
- **The first machine coming back** is refused while the standby has the pairing, and after it too, until the
  operator lets it link again on Takeovers, so the two never trade the link with two copies of the data. Make it the
  new standby, or a member of the standby's cluster on the same database.
- **While no machine is linked** the edge node holds what it holds for any local node that is away: the queue for its
  retention period, mail for the replicated recipients, the voicemail fallback and the offline page.

## An edge cluster

Several edge nodes can share one external database and take links, for an edge node that can fail or more capacity at
the edge ([plan](../plans/Edge-Node2.md#46-a-cluster-of-edge-nodes) §4.6). Each local node pairs once, with any member;
every member's answer lists the cluster's live members with their link addresses, and the local node links to each,
takes requests and items from each, and keeps the list in `runtime/db/edge_members`, so a restart finds a member when
the one it paired with is gone.

```
 visitors, mail servers, chat and phone providers
    |  443, 25, ...                                      an A, MX and SRV record for each member
 edge 1     edge 2     conf/edge-cluster/MoquiEdgeConf.xml; 8444 between members only
    +----------+       shared Postgres: the queue, pairings, host names, copies
    ^  each local node dials every member
 local node    conf/edge/MoquiLocalConf.xml (or each member of a local cluster, conf/local-cluster/MoquiPooledConf.xml)
```

The conf, the `.env` names, the DNS records and the commands are in
[runtime/conf/edge-cluster](../runtime/conf/edge-cluster/README.md). The members are a cluster as in
[Cluster](#cluster): one external database, the same `.env` and secrets, a distinct instance id each.

1. Start the first member once, so it generates its secrets into `.env`, among them `edge_link_key`, the one link key
   every member presents and every local node pins.
2. Copy that `.env` to each other member before its first start, and give each member its own `cluster_base_url` and
   `edge_link_url` (its own public `https` address, where local nodes dial it), and its own mail, media and SIP
   addresses.
3. Check System › Edge Clients › Edge cluster on any member: each member live, with its link address. It warns of a
   live member with no usable address while there is more than one.
4. Pair each local node as with one edge node. Its System › Edge Node lists a link to each edge member, its state and
   round trip, and which one carries its work up.

- **Work down** (requests, delegated calls, queue delivery, PASS) comes from each member, each item claimed by lease in
  the shared database, so it is applied once. **Work up** (the local node's outbox, replication, its call and SIP
  rows) goes over one link at a time and moves when that link ends.
- **By area.** Web, webhooks, PASS pages and tunnels: any member. Mail in: any member, each building its recipients
  from the shared copies and again when any member applies a change; mail out: any member claims a delivery row
  ([Mail-Inbound](interfaces/Mail-Inbound.md)). Calls: each member is a media node with its own public address and
  TURN, and the local node keeps a media node row, a SIP node row and a TURN secret for each member it links to; a
  call's legs are all on one member ([WebRTC](interfaces/WebRTC.md), [TURN](interfaces/TURN.md)). SIP: every member
  loads every trunk and one registers them at a time, by a lock row (Edge cluster shows which); a phone's binding names
  the member it registered with, and it rings only for calls whose media is on that member; the registrar's nonces are
  per member, so a phone that reaches another member is challenged again ([SIP](interfaces/SIP.md)).
- **Stopping a member** leaves the others serving its names: DNS sends visitors to each member's address, so a client
  of the stopped one retries another record, and each local node's other links carry its work.
- **A clustered all-in-one server** takes links on every member the same way: `node_role=all-in-one` on each, on one
  external database.

**Not built**: a front node before the cluster and the full stack (E19), cascading call media between edge members
(so a phone on one member rings for a call whose media is on another).

## Cluster

Several members share one database and run behind a load balancer. Members find each other in the
`moqui.server.ClusterMember` table, with heartbeats on the database clock; there is no Hazelcast or Redis. Over gRPC on
TLS with client certificates they send each other entity cache clears, notifications, co-editing updates, session ends
and call signaling.
Sessions are rows in the shared database, so any member can serve any request.

```
                   clients
                      | https
               load balancer             webapp_trusted_proxies on each member
           +----------+----------+
       member 1   member 2   member 3    conf/local-cluster/MoquiLocalConf.xml; 8444 between members only
           +----------+----------+
              shared Postgres            entity_ds_db_conf=postgres
```

Every member runs `conf/local-cluster/MoquiLocalConf.xml`; the `.env` names and the commands are in
[runtime/conf/local-cluster](../runtime/conf/local-cluster/README.md), and the conf's comments say what each setting
does. An edge node can take the load balancer's place: [A local cluster with an edge
node](#a-local-cluster-with-an-edge-node).

### Requirements

- **One external database.** Postgres, or MySQL with the default transport. The Enterprise package has no embedded
  Postgres, so a member cannot start on a database of its own by mistake. The content database shares it unless
  `content_ds_db_conf` names another ([Content database](#content-database)).
- **The same secrets on every member**: `session_cookie_key`, `cluster_tls_cert`, `cluster_tls_key` and
  `entity_ds_crypt_pass`. Startup fails when a live member has a different session key or certificate, with
  "session_cookie_key does not match a live cluster member" or "cluster_tls_cert does not match a live cluster member".
- **A distinct instance id**: each member keeps its own in `runtime/db/instance_id` (or `instance_id`). It is not in
  `.env`, so copying `.env` between members is safe.

### Steps

1. Start the first member once with `conf/local-cluster/MoquiLocalConf.xml` and the shared database settings in
   `.env`, so it generates its secrets into `.env`.
2. Copy that `.env` to each other member before its first start.
3. Set `cluster_base_url=https://<this member's private address>:8444` on each member. Empty is
   `https://127.0.0.1:8444`, which logs an error once other members are live.
4. Open port 8444 between the members only.
5. The conf writes each session change before the response (`<session-config flush="guarantee"/>`), for a load
   balancer that does not keep a client on one member. With sticky sessions it can go.
6. Set `webapp_trusted_proxies` and `webapp_client_ip_header` for the load balancer, then start the other members.
7. Check the cluster with `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database.

### Transports

`cluster_transport` sets how peers reach this member: its cluster port at `cluster_base_url` (the default: TLS with the
cluster certificate), Postgres `LISTEN`/`NOTIFY` (`notify`, for members that cannot connect to each other), or its SSH
server (`ssh`). On the cluster port `cluster_protocol` picks gRPC on HTTP/2 (the default) or HTTP/1.1 only (`rest`).
Members on different settings, or of different builds, still reach each other: the HTTP/1.1 POST is always there. The
values are in the conf's comments.

### What is shared and what is not

- **Shared**: scheduled jobs take a lock row, so each job runs on one member at a time. Content blobs, `db` git
  repositories, sessions and the LLM quotas are in the database.
- **Not shared**:
  - `local` and `remote` git repositories are on one member's disk. Use `db` repositories, or send `/git` and SSH to
    that member.
  - Files on disk are each member's own: the SFTP root (`runtime://sftp`), and any setting that names a directory,
    such as `pass_media_root`. Share the directory between the members.
  - The LLM gateway's requests-per-minute and the web protection's limits are counted on each member separately. A
    ban is shared: one a member starts is told to the others on the cluster topic `web-ban` and holds there too, and
    a member takes it only from a live member of its cluster. System › Security › Web Protection shows one member's
    counts and every ban it holds, and lifts a ban on every member.
- **With an edge node** (plans/Edge-Node2.md §4.4): each member's outbox is one queue for the pool (any linked member
  sends an item, the items of a key in order, each once) and an item from the edge node is applied once whichever
  member it reaches. One member sends replication and keeps the edge node's call media and SIP rows: the one holding
  the lock row `EdgeSenderLock` (claimed while its link is up, taken over 30 seconds after it stops renewing, let go
  when it stops). The others send it their change notes on the cluster topic `edge-replica-note`; a new sender sends
  each replicated set whole.

**Not built**: a screen listing the members of a cluster that no edge node pools (with one, the edge node's Edge
Clients › Clients and each member's System › Edge Node list them).

## PASS servers with a BOSS server or cluster

IronStone PASS (`mr-pass`) serves public sites: Forum, Account, Members and Sites. A PASS server can run without Mantle
or BOSS. It then calls an **authority** for every read and write: a BOSS server, or cluster, holding Mantle and the
business database. The public machines keep only sessions and their own conf.

```
 visitors --https--> PASS server forum.example.com --+
                     base components and mr-pass     |  https: /rest/s1/pass/v1/sites/*
                     its own small database          +------> authority boss.example.com
 visitors --https--> PASS server shop.example.com ---+        mr-boss and mr-mantle (mr-pass for local sites)
                                                               business database, jobs, mail, media
                                                               one server or a cluster
```

A PASS server runs `conf/pass/MoquiPassConf.xml` and the authority `conf/pass/MoquiAuthorityConf.xml`; the `.env` names
and the commands are in [runtime/conf/pass](../runtime/conf/pass/README.md), and the confs' comments say what each
setting does. Change their example names (the host, site id, client id and callback) to yours.

In local mode, mr-pass runs in the same runtime as Mantle and makes in-process calls; that is the default when
`pass_backend_url` is not set. Provisioning, mail, media, workers and imports are in mr-pass
[docs/Deployment.md](../runtime/components/addon/mr-pass/docs/Deployment.md). This section covers the remote layout.

The remote calls go over REST, which every package has and which is Lite's default, or over gRPC where the package has
it (Maximal, Enterprise): direct to the authority's gRPC listener, or from an
[edge node](#a-local-server-with-an-edge-node) to its local node over the edge link (`pass_backend_url=edge:`).
`pass_backend_transport` chooses (`rest`, `grpc`, or empty for automatic); the settings are in Deployment.md. With an
edge node as the PASS server, the local node's Edge Node wizard writes the client properties for it.

### The authority

1. Run a BOSS server: `./moq component-get mr-boss`, which brings mr-mantle. It can be one server or a
   [cluster](#cluster). Use Maximal or Enterprise: forum thumbnails need `image`, and sign-in providers need `sso`.
   The site readiness check reports what is missing. The PASS jobs are in Mantle's data (`ForumSetupData.xml`) and run
   here. Add mr-pass only if this server also serves sites itself.
2. Register each PASS deployment: in the conf its sites and exact SSO callback (`pass_client_<id>_sites`,
   `pass_client_<id>_callback`), in `.env` its secret (`pass_client_<id>_secret`). The conf turns on gRPC for PASS
   clients (`pass_authority_grpc_enabled`); the HTTPS listener takes the same calls over [HTTP/2](#http2), so no other
   port needs to be open.
3. Create the sites and bind their hosts and mounts with `mantle.public.SiteServices.create#Site` and `store#Mounts`
   (Deployment.md, Provisioning).
4. Put the PASS servers' addresses in the conf's `client-rule`, which exempts them from web protection, or all their
   visitors count as one client.

### A PASS server

1. Make a checkout that has no Mantle. mr-pass declares no `depends-on`, and a conf cannot remove a component that is
   in `runtime/components/addon`:

   ```sh
   git clone https://github.com/moqui/mr-moqui.git pass && cd pass
   ./moq component-get mr-pass
   ```

2. Build it with any server package. PASS calls the authority for images and sign-in, so Lite, the smallest, is
   enough: `./moq archive --lite --no-data --conf conf/pass/MoquiPassConf.xml`.
3. Set the authority's origin, the client id and the public site's host and mounts in the conf; they must match the
   ones bound at the authority. Put `pass_client_secret` in `.env`.
4. Embedded Postgres is enough. The PASS server stores framework tables and sessions, and the sessions hold each
   visitor's delegation from the authority.
5. Put the certificate for the public host in `cert/`. Behind a proxy, set `webapp_trusted_proxies` and
   `webapp_client_ip_header`. Each call to the authority carries the visitor's address, and the authority's sign-in rate
   limits count by it.
6. Check:
   - the site answers at its host;
   - logged in as an operator on the authority, `https://boss.example.com/rest/s1/pass/v1/sites/<siteId>/readiness`
     reports the media, mail, SSO, host and mount checks;
   - Deployment.md, Reproducing the remote check, runs the two-runtime test.

### Scaling

- **More PASS servers**: the authority takes any number of client IDs. A client ID has one SSO callback, so give each
  hostname that offers SSO its own client ID.
- **More PASS nodes for one hostname**: each PASS server can have its own database, or several nodes of one server can
  share a Postgres as their own [cluster](#cluster). That cluster is separate from the authority's.
- **The authority as a cluster**: delegations, SSO handoffs and rate-limit counts are rows, so any member can check
  them. The second factor and the SSO front channel use the authority's web session, so set `flush="guarantee"` or keep
  a client on one member. `pass_media_root` and `pass_import_dir` must name storage that every member can read.
- **Latency**: each public page makes at least two calls to the authority, and nothing is cached. Keep the PASS servers
  close to it.

**Not built**:
- a readiness screen: the check is the REST route and `check#Readiness` only, and it does not look at the PASS server's
  own conf;
- a cache of public pages;
- realtime push to a remote PASS server (the stream polls);
- more than one authority for one deployment, which is by design;
- the Commerce, Blog and other PASS apps;
- SAML sign-in.

## Dev instances with a dev group server

Each developer runs a dev instance on their own machine. The team shares a **group server**, a production instance
with IronStone BOSS. It hosts the components' repositories, the Projects work items, and the LLM gateway with the
team's model keys. Developers who do not run a server, often business users who extend screens and entities, work on a
**shared dev-aide server**. That is a separate dev instance that uses the group server as a laptop does.

```
 laptop      ./moq run, dev, 127.0.0.1 ---+   git over https or ssh, /llm, /rest/s1/mantle/my/tasks
 laptop      ./moq run, dev, 127.0.0.1 ---+--------> group server team.example.com, production purpose
 shared dev-aide server, dev, https ------+          mr-boss, mr-mantle: Projects, git hosting, LLM gateway
   dev_aide_shared=true                              one server or a cluster
```

The group server runs production purpose because it holds the team's real data. A dev instance loads Dev AIDE, which
gives its users code execution as the server's OS user, and the dev conf turns off the tarpits and allows every
origin.

The group server runs `conf/dev-group/MoquiGroupConf.xml`; the dev instances and the shared dev-aide server keep the
dev conf, with a few properties in their `.env`. Those names and the commands are in
[runtime/conf/dev-group](../runtime/conf/dev-group/README.md), and the conf's comments say what each setting does.

### The group server

1. Run BOSS: `./moq component-get mr-boss` (which brings mr-mantle), on Maximal, or on Enterprise as a
   [cluster](#cluster), with `conf/dev-group/MoquiGroupConf.xml`. Set it up as in
   [One production server](#one-production-server).
2. **Git hosting** ([Git-Hosting](interfaces/Git-Hosting.md)):
   - HTTPS clones work as soon as HTTPS does: `https://team.example.com/git/<repositoryId>.git`.
   - The conf runs the SSH server on every address, on 2222 (22 is usually the system's sshd).
   - Grant `SSH_GIT` to the developers' group. Seed data grants it to ADMIN only.
   - Create repositories from a project's Repositories tab, or on the Git Repositories screen (System, Resource
     Finder). On a cluster, make them `db` repositories.
   - Commit trailers such as `Task: PROJ-12` and branch names link commits to Projects work items through Mantle's
     gitECA rules.
3. **LLM gateway** ([Gateway](ai/Gateway.md)), which the conf turns on with the alias `default`:
   - add the aliases developers call (System › LLM › Gateway › Aliases, or the conf);
   - grant `LlmGatewayUse` to the developers' group;
   - for each developer's key, add an `LlmApiClient` row with the `gateway` scope (System › LLM › Gateway). Its
     `allowedAliases`, `dailyTokens` and `dailyCost` limit that key.
4. Optionally, turn on MCP (`mcp_enabled=true` in the conf) for coding agents. `/mcp/setup` gives a client config to paste
   ([MCP](ai/MCP.md)).

### Each dev instance

1. **Credential.** On the group server, make an access token: Projects › Code › Git access, or Commons › Settings ›
   Account. It is shown once. It is a login key, and it serves as:
   - the password for git over HTTPS;
   - the dev-aide task source's token;
   - the gateway key, once it has an `LlmApiClient` row.

   For git over SSH, add your public key on the same Git access screen instead.
2. **Repositories.** Put the team's repository and components in your `myaddons.xml`, which is not in git:

   ```xml
   <addons>
       <repository name="team">
           <location type="git" url="https://team.example.com/git/${name}.git"/>
       </repository>
       <component name="acme-site" repository="team"/>
   </addons>
   ```

   Then run `./moq component-get acme-site`. The repository ids on the group server must be the component names. Keep
   the token in a git credential helper (`git config --global credential.helper store`, or the OS keychain). With
   `default-repository="team"` on `<addons>`, every component comes from the group server.
3. **Models.** Point the default model endpoint at the gateway, in `runtime/.env` or with `-D`. The URL has no `/v1`,
   since the endpoint adds `/v1/chat/completions`.

   ```sh
   llm_openai_url=https://team.example.com/llm
   llm_openai_api_key=<access token>
   llm_openai_model=<alias>
   ```

4. **Tasks.** In Dev AIDE (`/qapps/dev`) › Tasks, add a source of kind `moqui-rest` with the base URL
   `https://team.example.com` and the token. It lists your work items from `/rest/s1/mantle/my/tasks`. A task app with
   the group server's base URL (Dev AIDE › Settings) opens them there. Commits from the Workspace carry the current
   task's trailer.
5. **Binding.** Set `webapp_http_bind=127.0.0.1`. Dev AIDE gives its users code execution, so a laptop's server should
   not be reachable from the network.

### The shared dev-aide server

1. Run a dev instance (`./moq run`, or `MOQUI_CONF=conf/MoquiDevConf.xml`) with the components the users extend, and
   set `dev_aide_shared=true`.
2. Give it a certificate, leave `webapp_http_bind` empty, and do not publish 8080. Set `webapp_allow_origins` to the
   server's own origin, such as `https://dev.example.com`, in place of the dev conf's `*`. An empty value would not
   replace it.
3. Put its users in `DEV_AIDE_EXTENDERS`, who may write site components and publish them, or `DEV_AIDE_USERS`, who may
   do everything.
4. Network git (fetch, pull, push, clone) is off on a shared server until an admin turns it on in Dev AIDE › Settings,
   with a deploy token or per-user tokens from the group server.
5. Commits carry the logged-in user as author. A save checks the file on disk by its hash, so a save over another
   user's change offers Compare instead of overwriting it.

### What is not shared

A shared dev database is not supported. Every serving process joins the cluster of its database, so dev instances
with their own generated secrets refuse to start on one. If they did start, one developer's entity changes would alter
everyone's schema, and any instance could run the team's jobs. Each dev instance keeps its own embedded Postgres. To
hand a database to someone else, send a `./moq save-db` save and the matching `entity_ds_crypt_pass`.

**Not built**:
- Moqui is not an identity provider, so a dev instance cannot log in through the group server. Point both at one
  external OIDC provider, or keep separate accounts.
- Pushing a hosted repository to another server; a `remote` repository is a read-only mirror.
- OAuth or OIDC tokens for MCP and A2A; they take login keys and app passwords.

## Components

Components live in `runtime/components/addon`, and dev-only ones in `runtime/components/dev`. Each is its own git repository.
`addons.xml` lists the known components and repositories. A `myaddons.xml` beside it, which is not in git, adds or
overrides `repository`, `component` and `component-set` elements. It can set `default="a,b"` (what `component-get` gets
with no names) and `default-repository` on its root.

| Task | What it does |
|---|---|
| `./moq component-get <name...>` | Get components and the components they depend on (`depends-on` in `component.xml`); a `component-set` name gets the components of the set (`apps`: mr-marble and mr-boss) |
| `./moq component-depends` | Get missing dependencies of what is in `runtime/components/addon` |
| `./moq component-create <name>` | Make a component from the component template, `runtime/components/dev/template`: an entity, services, a REST API, screens, data and specs; `--rest` or `--screens` for one part |
| `./moq component-zip [name...]` | Zip components |
| `./moq git <args>`, `git-status`, `git-pull` | Run git in the root repository and every component repository |

The get tasks take `-t git|current|release|binary` and `-r <repository>`. `--dev` puts the named components in
`runtime/components/dev`. A component with a `Cargo.toml` is linked into the binary on the next build
([Native-Components](dev/Native-Components.md)). A new component loads at the next start. System › Components can also
load it on a running server, with its data, when it has no Rust code
([Component-Deploy](tools/Component-Deploy.md#loading-a-component)).

## Moving from Moqui Java

| Moqui Java | MR Moqui |
|---|---|
| `java -jar moqui.war` | `moqui`, or `./moq run` from a checkout |
| `java -jar moqui.war load types=...` | `moqui load types=...`, or `./moq load ...` |
| `./gradlew addRuntime`, moqui-plus-runtime.war | `./moq archive`: a directory with the binary, not a WAR |
| `MoquiInit.properties`, `-Dmoqui.runtime`, `-Dmoqui.conf` | `MOQUI_RUNTIME`, `MOQUI_CONF`, or `--conf` |
| H2 by default | Embedded Postgres by default; `entity_ds_db_conf=h2` also means embedded |
| JDBC driver jars in `runtime/lib` | Built in: Postgres in every package, MySQL in Maximal and Enterprise |
| Tomcat, Elastic Beanstalk with a WAR | One process: the binary serves HTTP and HTTPS itself |
| Docker Compose with nginx-proxy | Podman stacks, and no proxy container ([podman/README.md](../podman/README.md)) |
| OpenSearch or ElasticSearch for Mantle and the apps | Optional; search is on Postgres by default. `no-run-es` still works |
| Hazelcast for a cluster | The shared database and HTTPS between members |
| `JAVA_TOOL_OPTIONS` heap sizing | No JVM, so no heap to size |

Component and data changes for a Java deployment are in [Moqui-Java-Migration.md](dev/Moqui-Java-Migration.md).

## Directory layout

```
mr-moqui/
  moq                 the task script (bash 3.2 or newer); moq.cmd launches it from PowerShell or cmd
  addons.xml          known components and repositories; myaddons.xml (not in git) adds to it
  crates/             the Rust workspace: moqui (the binary), moqui-host, moqui-core, moqui-lib, moqui-llm, moqui-db,
                      moqui-tui, moqui-browser
  runtime/            the runtime directory (runtime/README.md)
    components/       the component sets; components.xml lists those that load, in order
      base/           framework, tools, webroot, basic-comms
      addon/          components from ./moq component-get or System > Components (not in git)
      dev/            components that load only on a dev instance: dev-aide, example, demo (off until a conf names
                      it); template for component-create
      test/           test-only add-on components, loaded on a dev or test instance (not in git)
    conf/             MoquiDevConf.xml, MoquiProductionConf.xml, MoquiDemoConf.xml, MoquiLoadTestConf.xml; a
                      directory of confs per topology: edge/, all-in-one/, pass/, dev-group/, local-cluster/
    cert/ db/ log/ .env   per-instance files (not in git)
  podman/             the image, compose stacks and Quadlet units
  systemd/            the unit, sysusers file and drop-ins
  scripts/            pg-payload.sh, check-packages.sh, wasm-build.sh
  docs/               reference guides
  plans/              designs, status and reviews
  target/             build output: target/release/moqui, target/lite/moqui, ... (moqui.exe on Windows)
```
