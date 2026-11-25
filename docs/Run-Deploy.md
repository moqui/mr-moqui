# Run and Deploy

This guide covers running MR Moqui on your machine and deploying it on a server, in a container or as a cluster. It
also covers connecting instances to each other: PASS servers to a BOSS server, and developers' instances to a team's
group server. The first sections give the steps for the common cases; the sections after them have the detail. Building
and the build packages are in [Build-Test](Build-Test.md).

- [Quick start](#quick-start): develop on your machine, run one production server, deploy a single binary, run in a
  container
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
    [A local server with an edge node](#a-local-server-with-an-edge-node), [An all-in-one node](#an-all-in-one-node),
    [Managed instances](#managed-instances);
  - reference: [Components](#components), [Moving from Moqui Java](#moving-from-moqui-java),
    [Directory layout](#directory-layout).

## Quick start

### Develop on your machine

You need Rust 1.99 or newer (`rustc --version`; quickjs-oxide declares 1.99, and `rust-toolchain.toml` pins 1.100.0),
git and bash, on Linux, macOS, or Windows. On Windows, install the MSVC toolchain (`x86_64-pc-windows-msvc`), Visual
Studio Build Tools with the "Desktop development with C++" workload, and Git for Windows, then use `moq.cmd` instead
of `./moq` (the binary is `target\release\moqui.exe`). The first build that needs it downloads NASM 3.02 to
`%USERPROFILE%\.moqui\nasm`; set `MOQUI_NASM` to an existing directory or `nasm.exe` to skip that download. A
`cargo build` that does not go through `moq` still needs NASM on `PATH`, and for a dev build or `cargo test`,
`CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed`. NASM may no longer be needed at all: [Windows](Build-Test.md#windows).

```sh
git clone https://github.com/moqui/mr-moqui.git
cd mr-moqui
./moq component-get apps   # optional: Marble ERP (mr-marble), IronStone BOSS (mr-boss) and PASS (mr-pass), with Mantle (mr-mantle)
./moq load                 # builds the Maximal package the first time, then loads every data type, demo included
./moq trust-dev-cert       # trust the shared dev CA for this user
./moq run                  # serves https://localhost:8443; ctrl-c stops it
```

Open `https://localhost:8443` and log in as `john.doe` with the password `moqui`.

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
   sudo chown -R root:root /opt/moqui/moqui /opt/moqui/MoquiConf.xml /opt/moqui/conf   # read-only to the server
   ```

2. Put the certificate for your host in `/opt/moqui/cert/`, for example certbot's `cert/erp.example.com/fullchain.pem`
   and `privkey.pem` ([certificates](#certificates)), or set a name up with ACME once the server runs. A key file
   there works and is named in a warning; once unsealed, `moqui operator secret put tls-key/<name> --file <key>` keeps
   it in the database instead ([where secrets live](#where-secrets-live)).
3. Write the host's settings to `/opt/moqui/.env`. Secrets do not go there: the server makes its own into `seal.json`
   and the database ([where secrets live](#where-secrets-live)).
   `webapp-list/webapp[webroot]@https-port` is the port clients use; leave it out when they reach the server on 443.
   `moqui_conf__user_facade__login__initial_admin_enabled=true` turns on the form that creates the first administrator (step 5); it is off unless set.

   ```sh
   sudo -u moqui tee /opt/moqui/.env > /dev/null <<'EOF'
   moqui_conf__webapp_list__webapp__WEBROOT__http_host=erp.example.com
   moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=true
   moqui_conf__webapp_list__webapp__WEBROOT__https_port=8443
   moqui_conf__user_facade__login__initial_admin_enabled=true
   EOF
   sudo chmod 600 /opt/moqui/.env
   ```

4. Install the unit and start it, then make its seal. A production server starts sealed: it opens no port until it is
   unsealed, and waits on its operator socket for `moqui operator init` ([sealed start](#sealed-start)). With a TPM,
   `tpm` unseals it again at each reboot; write down the recovery shares `init` prints, one per holder.

   ```sh
   sudo cp systemd/moqui.service systemd/moqui-operator.socket /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl enable --now moqui-operator.socket moqui
   sudo /opt/moqui/moqui operator init --method tpm
   sudo /opt/moqui/moqui operator export-seal /root/moqui-seal-bundle.json   # once it serves; with the offline backups
   ```

5. The first start creates the tables and loads the seed, seed-initial and install data. Open
   `https://erp.example.com:8443/Login` and create the first administrator with **Create Initial Admin Account**. The
   form is there only while nobody has an account. From any machine but the server itself it also asks for the setup
   secret, which the server writes to its log at startup (`moqui.log`, the line that begins "No user account exists
   yet"). Check **Also join Administrators - Advanced** only if this account needs the advanced tools
   ([the first administrator](#the-first-administrator)). Then take `moqui_conf__user_facade__login__initial_admin_enabled=true` out of `.env`.
6. Keep the recovery bundle from step 4 with the database's backups, and export it again after a secret is put: it
   holds `seal.json`, whose bootstrap secrets include `entity-facade@crypt-pass`, the key of the encrypted fields.

Upgrades, backups, binding port 443 and the rest are in [One production server](#one-production-server).

### Deploy a single binary

A binary built with the production runtime in it ([Built-in runtime files](#built-in-runtime-files)) is the whole
deployment: no archive to unpack, and an upgrade is a new binary. On the build machine:

```sh
./moq archive --binary --embed-opensearch   # moqui-YYYYMMDD.gz and moqui-YYYYMMDD.sha256
```

`--binary` builds with `--embed production` (every base and add-on component in `runtime/components`, less tests and
plans); `--embed-opensearch` and `--embed-chromium` add those programs when the server has none of its own. On the
server, as in [Run one production server](#run-one-production-server) with the binary in place of the archive:

```sh
sudo install -d -o moqui -g moqui /opt/moqui /etc/moqui
gunzip -c moqui-20261114.gz | sudo install -o root -g root -m 755 /dev/stdin /opt/moqui/moqui
sudo install -o moqui -g moqui -m 644 moqui-20261114.sha256 /opt/moqui/embedded.sha256
sudo tee /etc/moqui/environment > /dev/null <<'EOF'
MOQUI_RUNTIME=/opt/moqui
MOQUI_CONF=conf/MoquiProductionConf.xml
EOF
```

`MOQUI_RUNTIME` makes `/opt/moqui` the runtime though it has no `components/` yet, and `MOQUI_CONF` picks the
production conf built in (the binary alone otherwise runs the dev conf). The first start unpacks Postgres, and
OpenSearch when it is used, into `/opt/moqui/install/`, and reads every other file from the binary. With
`embedded.sha256` there it checks the binary against it first and does not start when a part differs; where settings
come from the environment, set `MOQUI_EMBEDDED_SHA256_RUNTIME` and the others instead (`moqui embedded checksum --env`
prints them). Put an edited conf or a changed file on disk and it is used in place of the built-in one.

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
| Set a conf attribute for one run | `./moq run -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres` | `-Dmoqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres` |
| Load every data type | `./moq load` | `./gradlew load` |
| Load some types | `./moq load seed seed-initial install` | `./gradlew load -Ptypes=seed,seed-initial,install` |
| Load one component's data | `./moq load seed components=mr-mantle` | `java -jar moqui.war load components=...` |
| Load one file | `./moq load -l runtime/components/addon/mr-mantle/data/20-install/GeographyEurope.xml` | `java -jar moqui.war load location=...` |
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
| Remove obsolete cargo artifacts | `./moq clean` | |
| Reset the local instance, keep the current build and remove obsolete cargo artifacts | `./moq clean-dev` | |
| Reset the local instance and remove build output | `./moq clean-all` | `./gradlew cleanAll` |
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
| A front node before a local cluster | Load balancing and web protection for a local cluster, with none of an edge node's comms | [`conf/front/`](../runtime/conf/front/README.md) | Front: Maximal. Members: Enterprise (`--enterprise`) | The front its own (pairings, names, certificates); one external Postgres for the members | [A front node](#a-front-node) |
| Full stack | A higher-volume shop or other public site on mr-pass: a front node, an edge cluster serving the sites and comms, a local cluster with the data and the PASS authority | [`conf/full-stack/`](../runtime/conf/full-stack/README.md) | Front: Maximal. Edge members: Maximal with an external database, or Enterprise. Local members: Enterprise | The front its own; one external Postgres for the edge members, another for the local members | [Full stack](#full-stack) |
| The desktop app | A node on a person's own computer, in a window with a tray, paired with an edge or all-in-one node | In the app; its edge or all-in-one node as above | Maximal, in the app | Embedded Postgres in the app data directory | [moqui-desktop](../crates/moqui-desktop/README.md) |

The conf is the runtime conf each node runs, under `runtime/`. A topology's directory has a conf for each kind of node,
whose comments say what each setting does, and a README with its diagram, the `.env` names each node needs and the
commands ([runtime/conf](../runtime/conf/README.md)). The sections below keep what each topology is, its steps and
what is not built ([Edge-Node2 §3](../plans/Edge-Node2.md#3-topologies)).

## Production checklist

- **Package**: Maximal for embedded Postgres, or Enterprise for an external database. Debug, and the default `--release`
  build of Maximal, carry the dev tools (the file watcher, Dev AIDE): they open only on a dev instance, and the server
  warns at start when it finds them on a production one. A production image leaves them out: `./moq build --enterprise`,
  or Maximal without them (`cargo build --profile release --no-default-features -F maximal -p moqui`)
  ([Packaging](Build-Test.md#packages), SEC-OUT-14).
- **Purpose**: `instance@purpose` is `production` everywhere except in the dev conf. An archive's `MoquiConf.xml` is
  the production conf.
- **Seal**: the server starts sealed and opens its listeners only once unsealed: `moqui operator init` with `tpm` (or
  `passphrase`, `shamir`), the recovery shares with their holders, the recovery bundle with the offline backups
  ([sealed start](#sealed-start)). The `dev` seal is refused on a production instance.
- **Secrets**: none in `runtime/.env`. The server makes its secrets into `seal.json` and the `SecretValue` table, and
  an operator puts the others with `moqui operator secret put` ([where secrets live](#where-secrets-live)). Without
  the recovery bundle (`seal.json`) the encrypted fields cannot be read.
- **Data**: no demo data, so no `john.doe` with password `moqui` ([first start](#data-and-the-first-start)).
- **HTTPS**: a certificate in `runtime/cert`, or a proxy in front with `webapp-list/webapp[webroot]@trusted-proxies` set
  ([HTTPS](#https-ports-and-proxies)).
- **Ports**: firewall 8080. `webapp-list/webapp[webroot]@http-bind` sets the address of both HTTP and HTTPS, so `127.0.0.1` suits only a
  server behind a proxy on the same machine. The cluster listener (8444) starts on every server, on every address, so
  firewall it unless cluster members use it ([ports](#ports)).
- **Backups**: run the embedded database's backup job, or back up the external database
  ([backups](#backups)).
- **Disk and memory**: the thresholds at which the System home warns are `resource-watch`'s; copy the disk, memory
  and CPU alert rules so someone is told ([resource watch](#resource-watch)).
- **Listeners**: SSH, inbound SMTP, A2A, MCP and the LLM gateway are off by default. Turn on only the ones that are used.
- **CORS**: leave `webapp-list/webapp[webroot]@allow-origins` empty. The dev conf names `localhost,127.0.0.1` (this machine on any port);
  `*` is never sent with credentials.
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
else it finds no runtime and makes a new one there. A runtime reads the files built into the binary that it does not
have (the framework, tools, webroot and conf files, and with a production build every component; below), so a lone
binary starts in an empty directory.

### Built-in runtime files

The binary carries a copy of part of the runtime (`crates/moqui-embedded`), at one of three levels chosen when it is
built (`./moq build --embed <level>`, [Embedded runtime and programs](Build-Test.md#embedded-runtime-and-programs)):

| Level | What is built in |
| --- | --- |
| `framework` (the default) | The framework and tools components less their `test/`, a webroot stub listed in `crates/moqui-embedded/webroot-files.txt` (the REST, login, error and html `/apps` screens, the html render mode's style sheet and scripts with Tom Select and SortableJS; no fonts, icons, `/qapps` or web client libraries), and the runtime's `conf/` with every example and starter conf |
| `production` | The framework level with the whole webroot (the web client) and every other component of the base and add-on sets, each less its top-level `test/`, `plan/` and `plans/`: the file set of `./moq archive` |
| `all` | The production level with each component's `test/`, `plan/` and `plans/`, and the dev and test sets: `./moq archive --with-tests --with-dev` |

Neither the binary nor `./moq archive` takes the names `crates/moqui-embedded/archive-excludes.txt` lists (`.git`, a
component crate's `target/`, `__pycache__`, editor backups); the binary also leaves out dot files. It uses the copy
three ways:

- **Files read in place.** Each built-in file that is not on disk is read from the binary, in any runtime; a file on
  disk is used in its place. The framework component is the one `MOQUI_FRAMEWORK` names, tools and webroot are in
  `components/base`, the other components in their sets under `components/`, and the confs in `conf/`, so `--conf
  conf/MoquiProductionConf.xml` and the topology confs work with the binary alone. Without a runtime the server and
  `moqui load` make only its directories (`components/base/framework`, `tools`, `webroot`, a directory for each other
  built-in component, and the database and log directories when they are used). At the framework level, with a
  placeholder page at `/` in place of the web client and the built-in sets' manifest `components/components.xml`, it
  boots, loads the seed data, answers `/rest` (with the tools `moqui` API), `/Login`, `/status` and the html `/apps`
  (System, Tools, Dashboards), and runs `moqui tui`; links to fonts, icons and the web client are broken until the
  full webroot is installed. At the production level it is the whole instance ([Deploy a single
  binary](#deploy-a-single-binary)). Every built-in component loads, as one on disk would. A file written to a
  built-in location (an edit in System, a component put in `components/addon`) lands on disk and is used from then on.
- **Startup comparison.** After the factory boots, the server and `moqui load` compare the components on disk with the
  copies and log `N framework, tools and webroot files are not on disk and are read from this binary` when there are
  some, and `N … files differ from the ones built into this binary`, then one line per file: changed (with the first
  line that differs) or added on disk (the framework level's webroot copy is a subset). For the framework, tools and
  webroot these are warnings, since a changed file may no longer fit the binary's code; for the other components
  (production and all) they are information, and a component on disk in a built-in set that the binary does not have
  is one line. A file the binary has compiled in (`template/XmlActions.js` and three others) is marked as ignored,
  since its disk copy is never read. `moqui embedded diff [<set>/<path>]` prints the diffs (`conf/<path>` and
  `components/<set>/<component>/<path>` too), `moqui embedded list` the built-in files and `moqui embedded info` the
  level, sets and programs.
- **Extract.** `moqui embedded extract` writes each built-in file that is not on disk into the runtime `moqui` uses,
  and `moqui embedded extract <dir>` into another; files already there are never replaced. Then the runtime reads
  none from the binary, and its files can be edited or put in git.

**Programs.** Postgres (the `embedded-postgres` feature of the Lite and Maximal packages), OpenSearch (`--embed-opensearch`)
and Chromium (`--embed-chromium`) can be built in as their archives. Each is unpacked into the install directory,
`<MOQUI_INSTALL>/<program>/<version>` (default `install/` in the runtime, which `./moq archive` leaves out), the first
time it is needed, and runs as its own process; unlike the runtime files they are then ordinary files on disk, so
this is packaging and delivery only. `moqui embedded unpack [postgres|opensearch|chromium|all]` unpacks them ahead of
the first start (a container image). OpenSearch starts when the search backend is `opensearch`, the cluster is local
and not running, and `runtime/opensearch` has no install of its own; its config (seeded from the install on the first
start), data and logs stay in `runtime/opensearch`. Chromium is used for PDF and the browser tests after
`screen-facade/pdf@chromium-path` and `runtime/chromium`, before an installed one.

**Checksums.** `moqui embedded checksum` prints the SHA-256 of each part built in, in `sha256sum` format: `runtime`
(the packed runtime files as built), and `postgres`, `opensearch` and `chromium` (each the `sha256sum` of the archive
file the build embedded); `--env` prints them as `MOQUI_EMBEDDED_SHA256_<PART>=<hex>`. Before it starts, the server
and `moqui load` check the parts named in the file `moqui_embedded_checksums` names (a property, environment
variable or `.env` entry; it must be there), else in `<runtime>/embedded.sha256` when that is there, and in the
`MOQUI_EMBEDDED_SHA256_*` variables, which take the place of the file's lines. A part that differs, or one named that
the binary does not have, is logged as an error with both checksums, and the start is refused. Lines for other files
(the `.gz` that `./moq archive --binary` writes) are skipped, so the same file serves `sha256sum -c
--ignore-missing`. With none given nothing is checked.

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

### Settings and the environment

Every setting is an attribute of an element of the merged conf, with its default written in `MoquiDefaultConf.xml`
(or, for a component's own settings, the component's `MoquiConf.xml`: basic-comms' `comms`, mr-pass's `pass`). A conf
file sets one by its element: `<ssh-server port="2222"/>` in the runtime conf. Docs, scripts and System › Conf name a
setting by its path, a selector: `ssh-server@port`, `webapp-list/webapp[webroot]@http-port` (a keyed element's key in
brackets).

Any attribute can also be set from the environment, over every conf file, by a name built from the same path:

```sh
moqui_conf__ssh_server__port=2222                              # ssh-server@port
moqui_conf__webapp_list__webapp__WEBROOT__http_port=8091       # webapp-list/webapp[webroot]@http-port
moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres
```

The name is `moqui_conf__`, then each element from the root with `__` between them, a keyed element's key value after
it (in capitals by convention, so it stands out), and the attribute last. Case does not matter, nor punctuation within
a segment (`http_port`, `HTTP_PORT` and `http-port` are one name). A name is looked up in this order, and the first
that is set wins over every conf file:

1. a `-D name=value` on the command line (`./moq -D` sets it as an environment variable);
2. the process environment;
3. `runtime/.env`.

An empty value is not applied, so it cannot clear a setting; to clear one, set the attribute empty in the runtime conf.
A name for an element or attribute the conf does not have adds it, with a warning when its element is in the files
(most often a misspelled name). `moqui conf env [text]` lists every attribute with its name and value (secrets masked)
and where it was set, and System › Conf shows each attribute's name and what set it. The boot logs each name it applied.

`runtime/.env` holds `KEY=value` lines. Blank lines and `#` comments are ignored, a matching pair of quotes is
stripped, and `$` is not expanded. It is read before the conf merge, but cannot select `MOQUI_CONF`. An archive made
with `--no-data` leaves it out, so it is the place for per-host settings that should survive an upgrade. It holds no
secret.

When a secret attribute is still `CHANGEME`, its value is generated where the server keeps its secrets, never in
`runtime/.env` ([where secrets live](#where-secrets-live)): `entity-facade@crypt-pass` into `seal.json` at the first
start, and on first use `webapp-list/webapp[webroot]/session-config@cookie-key`, `ssh-server@host-key`,
`git-server@lfs-token-key`, the pair `cluster-member@tls-cert` and `cluster-member@tls-key` (made together),
`edge@link-key`, `acme@account-key`, `push/web@vapid-key` and the other keys the default conf leaves at `CHANGEME` into
the `SecretValue` table. `mail-delivery/dkim@key` (empty by default) is made there too, the first time a server needs
it. A value the conf files or the process environment give wins over the store. One in `runtime/.env`, which the
server's own user can write, is ignored once the runtime has a seal other than the `dev` seal, with an error logged;
before the seal, and under the `dev` seal (whose own file holds its root key: a dev checkout's, a test run's), it is
still read, with a warning. `moqui test` and `moqui test-client` give their runtimes the `dev` seal whatever their
purpose, so a test server on a production purpose keeps its crypt pass in its scratch runtime's seal, and a test
snapshot (`runtime/save/test-*`) keeps that `seal.json` beside its dump; one with none is made again. A secret is never logged, is masked wherever the conf is shown, and is null to a script's
`ec.factory.getConfValue` and `System.getProperty`.

A conf attribute holds its value as written: `${...}` is not expanded, and a conf that still has a `${` in an
attribute, or a `default-property` element, refuses to start, naming each attribute and the environment name that
sets it. Process properties remain for what is not configuration: the state the server sets as it runs, the switches
of a test run, and the `${}` of a `component.xml` (`example_disabled`).

**Upgrading from properties.** Settings were once `default-property` entries with flat names (`entity_ds_crypt_pass`,
`webapp_http_port`), referenced as `${name}` from attributes. Those names are no longer read, in the environment, in
`runtime/.env` or in a conf file. Move each to its attribute's name (`moqui conf env` lists them); in particular copy
the old secrets to their new names before the first start, or a new one is generated: `entity_ds_crypt_pass` to
`moqui_conf__entity_facade__crypt_pass` (without it encrypted fields can no longer be read: `./moq clean-db` and
`./moq load` for a dev runtime), the session cookie key, `ssh_host_key` (else the SSH host key changes) and the edge link
key (else each local node must pair again).

The gRPC listener's port and address moved to the webroot webapp, beside the HTTP and HTTPS ports, since the OTLP
receiver and the PASS authority share one listener: `pass-authority@grpc-host` and `telemetry/receiver@grpc-bind` are
now `webapp-list/webapp[webroot]@grpc-listen-bind`, and `pass-authority@grpc-port` and `telemetry/receiver@grpc-port`
are `webapp-list/webapp[webroot]@grpc-listen-port` (`moqui_conf__webapp_list__webapp__WEBROOT__grpc_listen_port`). The
boot refuses a conf file or an environment name that still sets an old one, naming its replacement. The listener
ports are separate from the ports of generated URLs: `http-listen-port` and `https-listen-port` are what the server
binds, `http-port` and `https-port` what its URLs say.

The settings most deployments set or should know. Their defaults are in `MoquiDefaultConf.xml`.

| Setting | Default | What it does |
|---|---|---|
| `instance@purpose` | `production` (`dev` in the dev conf) | `production`, `dev` or `test`. Only `dev` loads the dev components (Dev AIDE, the example component); the Desktop app's tray item Developer tools sets it for its node |
| `instance@default-locale`, `instance@default-time-zone`, `entity-facade@database-time-zone` | `en_US`, `UTC`, `UTC` | |
| `webapp-list/webapp[webroot]@http-host` | empty: the request's host | The host in generated URLs |
| `webapp-list/webapp[webroot]@http-bind` | empty: every address | The address the HTTP and HTTPS listeners bind; `127.0.0.1` keeps them on the machine |
| `webapp-list/webapp[webroot]@http-port` | empty: 8080 | The HTTP port in generated URLs |
| `webapp-list/webapp[webroot]@http-listen-port` | empty: use http-port | HTTP listener, or `off`. A numeric `-p` sets both ports |
| `webapp-list/webapp[webroot]@https-listen-port` | empty: 8443 | The HTTPS listen port (`--https-port`), or `off` |
| `webapp-list/webapp[webroot]@https-port` | empty: 443 | The HTTPS port in generated URLs and redirects |
| `webapp-list/webapp[webroot]@https-enabled` | `false` | `true` makes generated URLs https |
| `webapp-list/webapp[webroot]@grpc-listen-port` | `4317` (`off` in the dev conf) | The dedicated gRPC listener for the OTLP receiver and the PASS authority, opened only when one is on, or `off`; gRPC over HTTP/2 on the HTTPS listener works either way. Was `pass-authority@grpc-port` and `telemetry/receiver@grpc-port` |
| `webapp-list/webapp[webroot]@grpc-listen-bind` | empty: `http-bind`'s | The address the gRPC listener binds. Was `pass-authority@grpc-host` and `telemetry/receiver@grpc-bind` |
| `webapp-list/webapp[webroot]@allow-origins` | empty (`localhost,127.0.0.1` in the dev conf) | Other origins allowed for CORS; `*` allows any origin without credentials |
| `webapp-list/webapp[webroot]@trusted-proxies` | empty | Address ranges of the proxies in front ([proxies](#behind-a-proxy-or-load-balancer)) |
| `webapp-list/webapp[webroot]@client-ip-header` | empty | The header those proxies put the client address in |
| `webapp-list/webapp[webroot]@status-ips` | `127.0.0.1` | Addresses, besides loopback, that `/health` answers |
| `service-facade@scheduled-job-check-time` | `60` | Seconds between job checks; `0` runs no scheduled jobs |
| `terminal@command-line`, `terminal@host-sessions` | `dev` | The browser terminal's `moqui>` command line, and programs on the server's operating system: `dev` on a dev instance, `true` on for any purpose, `false` off; any other value, empty included, is off with an error in the log ([Terminal](ui/Terminal.md#conf)) |
| `terminal@remote` | `false` | `true` lets those be reached from other machines, over HTTPS with a recent sign-in; `moqui_conf__terminal__remote=true` in `runtime/.env` is the lightest way ([Terminal](ui/Terminal.md#who-may-open-what-and-from-where)) |
| `screen-facade/pdf@chromium-path` | empty | The Chromium for PDF and the browser tests; empty looks in `runtime/chromium`, then for an installed Chromium, Google Chrome or Microsoft Edge (the PATH and the OS's install locations) |
| `load-runner@targets`, `load-runner@max-runs`, `load-runner@max-users` | empty, `1`, `1000` | Load runs from Tools: the URL prefixes a run may reach besides this server and loopback (any URL on a dev instance; plain http to another machine only when listed), the loads at once, and the users a run |
| `ssh-server@port`, `ssh-server@host` | empty, `127.0.0.1` | The SSH server (SFTP, git, the apps in a terminal); off while `ssh-server@port` is empty (`2222` in the dev conf) |
| `media-server@port`, `media-server@host`, `media-server@public-address` | empty, `127.0.0.1`, empty | Call media ([WebRTC](interfaces/WebRTC.md)); off while `media-server@port` is empty; the public address is what browsers are told, for a server behind 1:1 NAT |
| `calls@ring-seconds` | `45` | How long a call rings before it is missed |
| `board@max-document-bytes`, `board@max-update-bytes` | 20 MB, 1 MB | Boards (wiki and meeting whiteboards): the largest scene a board holds, and the largest single co-editing update a client sends |
| `board@max-total-image-bytes` | `209715200` | Maximum total board image bytes (200 MiB). Saving a board prunes unused images after a grace period |
| `webapp-list/webapp[webroot]@socket-updates-per-minute` | `1200` | Collaboration document updates per socket per minute |
| `webapp-list/webapp[webroot]@socket-awareness-per-minute` | `600` | Collaboration awareness messages per socket per minute |
| `board@max-images`, `board@max-image-bytes` | `200`, 10 MB | The images one board may hold, and the largest one |
| `calls@guest-joins-per-minute` | `20` | The most guests who join by one call link in a minute, whatever their client |
| `chat@server-name`, `chat@to-device-retention-days` | `localhost`, `30` | The server name in the Matrix user IDs of browsers' crypto devices (set it before the first device: a change makes every device new), and how long a to-device message waits for a device that does not take it ([Chat keys](interfaces/Chat-Keys.md)) |
| `chat@store-key-secret` | generated | The platform secret each crypto device's browser-store key is derived under (HMAC of user, device and revision), so no table holds the keys. A new value makes every browser's store unreadable: each makes a new device and reads older messages from the key backup ([Chat keys](interfaces/Chat-Keys.md)) |
| `media-server@cascade-enabled` | `false` | Cascading media nodes ([WebRTC](interfaces/WebRTC.md#cascading-media-nodes)): a leg's media goes to the media node of its own socket, which relays to the call's node over both nodes' `media-server@port`; set it on every node |
| `calls/captions@enabled`, `calls/captions@provider`, `calls/captions@store` | `false`, `llm`, `false` | Live captions in calls ([WebRTC, Captions](interfaces/WebRTC.md#captions)): off unless `true`; the provider is the model bound to the `transcribe` feature (`local`, speech-local, is not built yet); `true` keeps each caption as a `CallTranscript` |
| `calls/captions@chunk-seconds`, `calls/captions@queue`, `calls/captions@workers`, `calls/captions@timeout-seconds`, `calls/captions@language` | `5`, `6`, `2`, `20`, empty | The longest chunk of speech sent, the chunks that wait (past this the oldest is dropped), how many of a call's go at once (all calls on a node together use at most half the service pool), how long one may take, and the language (empty lets the model tell) |
| `media-server/turn-server@port`, `media-server/turn-server@tls-port`, `media-server/turn-server@tls443-host`, `media-server/turn-server@public-host`, `media-server/turn-server@auth-secret`, `media-server/turn-server@external-urls`, ... | empty; the secret generated | The embedded TURN server, or coturn instead ([TURN](interfaces/TURN.md)) |
| `calls/recording@enabled`, `calls/recording@ack-required`, `calls/recording@policy-revision` | `false`, `false`, `1` | Call recording by a call's host, with a notice to every leg, and whether each leg must acknowledge it ([Call recording](interfaces/Call-Recording.md)) |
| `calls/recording@retention-days` | `90` | Recordings are deleted this long after they start; `0` keeps them |
| `calls/recording@queue-frames`, `calls/recording@segment-mb`, `calls/recording@segment-minutes`, `calls/recording@max-mb` | `4096`, `64`, `30`, `2048` | The recorder's queue, when a file is cut into the next segment, and where a recording stops |
| `calls/recording@prompt-audio`, `calls/captions@prompt-audio` | empty | WAV files a phone leg hears after the recording and captions tones ([SIP, Notices](interfaces/SIP.md#notices)) |
| `sip-server@port`, `sip-server@host`, `sip-server@tls-port`, `sip-server@tls-host`, `sip-server@rtp-ports`, `sip-server@rtp-host`, `sip-server@public-address`, `sip-server@tls-ca-file` | empty, `127.0.0.1`, empty, `sip-server@host`, `20000-20999`, `sip-server@host`, empty, the system bundle | Phone calls over SIP trunks ([SIP](interfaces/SIP.md)); off while both ports are empty, and only beside call media; the public address is what trunks are told in Contact and SDP |
| `channels@webhooks-enabled`, `channels@webhook-max-bytes` | `false`, `262144` | The chat bridges' routes a provider calls: `POST /channels/{chatBridgeId}/webhook` (its body at most this many bytes) and `GET /channels/{chatBridgeId}/media/{token}` ([SMS](interfaces/SMS.md)) |
| `chat@groupme-self-service` | `true` | Whether users connect their own GroupMe account in Settings, Chat ([GroupMe](interfaces/GroupMe.md#connect)); `false` leaves it to an administrator (System, Chat Bridges) |
| `chat@groupme-node-max` | `50` | The most GroupMe bridges one node holds the push connections of; the others go to other nodes ([GroupMe](interfaces/GroupMe.md#how-it-works)) |
| `channels@public-url`, `channels@media-key`, `channels@media-minutes` | empty, generated, `60` | The https base a provider reaches this server at, for the files of an MMS that goes out (none without it), the key their addresses are signed with, and how long one works |
| `channels/edge-limits@webhooks-per-minute`, `channels/edge-limits@webhooks-per-client-minute`, `channels/edge-limits@unverified-max-bytes`, `channels/edge-limits@unverified-waiting` | `600`, `120`, `32768`, `500` | On an edge node, for a bridge whose provider signs with a secret it does not hold (the local node checks them): the webhooks a minute it queues for the bridge and from one client address, the largest body, and how many of the bridge's may wait; past them 413 or 429 |
| `sip-server/limits@request-rate`, `sip-server/limits@request-burst`, `sip-server/limits@max-transactions`, `sip-server/limits@max-connections`, `sip-server/limits@max-connections-per-source` | `20`, `100`, `2000`, `500`, `100` | The SIP listener's flood caps: new requests a second per source and at once, the transactions past which new requests are dropped, TCP and TLS connections at once, and from one source ([SIP, Settings](interfaces/SIP.md#settings)) |
| `sip-server/registrar@enabled`, `sip-server@domain` | `false`, empty | The registrar for desk and soft phones, off even with the SIP listeners on; the realm and domain phones register to (`sip-server@public-address`, else `moqui`, when empty) ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `xmpp-server@domain`, `xmpp-server@rooms-domain`, `xmpp-server@upload-domain` | empty, `rooms.<domain>`, `upload.<domain>` | XMPP for local users ([XMPP](interfaces/XMPP.md)): off while `xmpp-server@domain` is empty; users are `user@<domain>`, rooms `room@rooms.<domain>` |
| `xmpp-server@port`, `xmpp-server@host`, `xmpp-server@tls-port`, `xmpp-server@tls-host`, `xmpp-server@websocket-enabled` | empty, `0.0.0.0`, empty, `xmpp-server@host`, `false` | The XMPP client listeners: STARTTLS (5222 by convention) and direct TLS (5223), each off while empty; `/xmpp-websocket` on the HTTP listener when `true` |
| `xmpp-server/upload@url`, `xmpp-server/upload@max-bytes`, `xmpp-server/upload@secret` | `https://<domain>` (with `webapp-list/webapp[webroot]@https-port`), `20971520`, generated | The HTTPS base of upload URLs (`/xmpp-upload/`), the largest upload, and the secret the URLs of chat files sent to XMPP clients are derived with |
| `xmpp-server@resume-seconds`, `xmpp-server@fast-token-days` | `300`, `14` | How long a dropped session waits for stream resumption, and how long a FAST token lasts |
| `xmpp-server/upload@seconds` | `300` | The longest a PUT to an upload slot may take |
| `xmpp-server@turn-enabled`, `xmpp-server@turn-per-hour` | `false`, `30` | External services (XEP-0215): TURN credentials for XMPP apps' calls, never to an external user, at most so many a user an hour |
| `xmpp-server@scram-decoy-key` | generated | Derives the SCRAM salts of names that are no user's, so they stay the same across restarts and nodes |
| `xmpp-server@s2s-port` | empty | Server to server (5269, federation, C3b): not built, nothing listens on it yet |
| `calls@forward-max-hops`, `calls@forward-ring-seconds` | `5`, `30` | The most forwards one phone call takes (a user's rules, a ring group's fallback; 1 to 10), and how long a forward to an outside number rings before the call goes on (10 to 120) ([SIP, Call handling](interfaces/SIP.md#call-handling)) |
| `calls/queue@tick-seconds`, `calls@transfer-ring-seconds` | `5`, `30` | How often the SIP engine checks each call waiting in a queue (offers, the wait limit, the caller's place; 1 to 30), and how long a blind transfer's target rings before the caller goes to their voicemail (10 to 120) ([SIP, Queues](interfaces/SIP.md#queues), [Transfer and merge](interfaces/SIP.md#transfer-and-merge)) |
| `calls@hold-music-location`, `calls/queue@prompt-location` | empty | Hold music for calls on hold and in queues without their own (WAV or Ogg Opus, looped; empty: a tone pattern; use only audio you may use), and a folder of `0.wav` to `9.wav` that speaks a queue caller's place (empty: pips) ([SIP, Queues](interfaces/SIP.md#queues)) |
| `sip-server/registrar@min-expires`, `sip-server/registrar@max-expires`, `sip-server/registrar@default-expires`, `sip-server/registrar@nat-expires` | `60`, `3600`, `600`, `60` | A phone binding's expiry in seconds: its bounds, when the phone asks none, and behind a NAT without Outbound ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `voicemail@max-seconds`, `voicemail@trash-days` | `120`, `30` | The longest voicemail message (10 to 600), and how long a deleted one waits in the trash before it and its audio go (0 keeps them) ([Voicemail](interfaces/Voicemail.md)) |
| `voicemail@prompt-location` | empty | A folder (any resource location) with the recorded speech played after the voicemail tones: `greeting.wav`, `enter-box.wav`, `enter-pin.wav`, `pin-refused.wav`, `menu.wav`, `no-messages.wav`, `goodbye.wav`; empty plays tones only ([Voicemail, Prompts](interfaces/Voicemail.md#prompts)) |
| `voicemail@pin-min-digits` | `6` | The fewest digits a new voicemail PIN has (4 to 12); four fall to the box's limit of failed tries in weeks ([Voicemail](interfaces/Voicemail.md#phone-access)) |
| `voicemail@transcribe`, `voicemail@transcribe-language` | `false`, empty | `true` sends each voicemail to the speech provider (the model bound to `transcribe`) for its transcript, by the deployment's recording and transcription policy; the language as an ISO 639-1 code |
| `instance@node-role` | empty | `front_edge` makes this an edge node, which takes links from paired local nodes on its HTTPS listener and has both option sets, the front (web access, ports, balancing) and the edge (delegate, queue, replicate, mail, chat, calls, SIP, PASS) ([Edge node](#a-local-server-with-an-edge-node)); `front` has the front set alone and `edge` the edge set alone, for a front node and the edge nodes behind it ([A front node](#a-front-node), [Full stack](#full-stack)); `all-in-one` makes a full server that also takes local nodes' links, routing each item by its owner ([All-in-one node](#an-all-in-one-node)). Another value is logged and means a normal node |
| `edge@link-key` | generated | The key an edge node's links use; generated on every node, used only on an edge node. Local nodes pin it, so a new one means each must pair again |
| `edge/node@url`, `edge/node@key`, `edge/node@pin` | empty | On a local node, its edge node: the address, this node's private key (a secret) and the pin of the edge node's link key. Pairing writes them to `runtime/.env` and unpairing removes them; they are not generated |
| `edge/node@calls-enabled` | empty (on) | On a paired local node, whether new calls have their media on the edge node's media node; `false` keeps its media row as draining, so new calls use this node's own media nodes and calls already on the edge node go on. The wizard's Apply writes it and unpairing removes it ([Edge node](#a-local-server-with-an-edge-node)) |
| `edge/node@sip-enabled` | empty (on) | On a paired local node, whether its trunks and phones go through the edge node's SIP engine; `false` sends the replicated sets `voiceTrunks` and `voiceFallbacks` empty, so the edge node registers no trunk and answers no number from a fallback, and keeps its `SipNode` row but gives it no new call. The wizard's Apply writes it and unpairing removes it ([SIP](interfaces/SIP.md#behind-an-edge-node)) |
| `edge@ssh-tunnel-port`, `edge@ssh-tunnel-host` | empty | On an edge node, a port whose connections go to the SSH server of the one paired local node (refused with several, and while it is away), and the address it listens on (empty for every one). With several local nodes each gets a port of its own: a `moqui.edge.EdgePort` row (System › Edge Clients › Ports), on `edge@ssh-tunnel-host` |
| `edge@smtp-tunnel-host` | empty | On a front node, the address an `EdgePort` row of kind `smtp` listens on (empty for every one): port 25 passed to an edge cluster's members with a PROXY header naming the client ([Mail-Inbound](interfaces/Mail-Inbound.md), plans/Edge-Node2.md §4.7) |
| `edge@xmpp-port`, `edge@xmpp-tls-port`, `edge@xmpp-host` | empty | On an edge node, the ports for every local node's XMPP domain, STARTTLS (5222) and direct TLS (5223), and the address they listen on (empty for every one): each connection goes to the node whose domain it names. A node's own pair of ports is an `EdgePort` row each |
| `edge@call-legs-per-pairing` | 200 | On an edge node, the most call legs of one local node on its media node at once; 0 for no cap |
| `edge@offline-page` | empty | On an edge node, the location of the page a terminate host shows while its node's link is down; empty for the built-in one |
| `edge@public-addresses` | empty | On an edge node, its public IPv4 and IPv6 addresses, comma-separated, which it offers its local nodes for their A and AAAA records; empty, and the wizard expects the addresses of its MX host name |
| `mail-delivery@mta` | `moqui` in `conf/edge/MoquiEdgeConf.xml`, else empty | On an edge node, what delivers the local node's mail out: `moqui` (to each domain's MX), or empty or `postfix` (relayed to the MTA beside it, E5a) |
| `mail-delivery@relay-host`, `mail-delivery@relay-port` | `127.0.0.1`, `10025` | On an edge node with `moqui_conf__mail_delivery__mta=postfix`, the MTA beside it that mail the local node sends is relayed to (Postfix's relay port) |
| `mail-delivery@bounce-domain` | empty | On an edge node, the domain of the envelope sender of mail it sends out, `bounce+<item>@` it, whose bounces the listener takes back to the message; empty sends from the message's own sender |
| `smtp-server@internet`, `smtp-server@rspamd-url` | empty | `true` runs the SMTP listener in internet mode (port 25; `true` in `conf/edge/MoquiEdgeConf.xml`), with Rspamd's HTTP API for its spam verdict when the URL is set; its hourly limits per client address and per sender are `smtp-server` `internet-messages-per-hour` and `internet-bytes-per-hour` (1000, 1 GiB) ([Mail-Inbound](interfaces/Mail-Inbound.md#mail-in)) |
| `mail-delivery/dkim@key`, `mail-delivery/dkim@selector`, `mail-delivery/dkim@domains` | generated, `moqui`, empty | The node's DKIM key (base64 PKCS#8, or PEM; RSA-2048 made into `runtime/.env` the first time a server needs it), its selector, and the domains signed for (empty: the hosted accounts' domains). `get#DkimRecord` gives the TXT record. A send by SMTP is signed only when the key is set or the send goes to this node's own listener |
| `smtp-server@arc-trusted-sealers` | empty | Domains whose passing ARC seal overrides a DMARC reject or quarantine on port 25, when the sealer recorded `dmarc=pass` for the From domain; empty for no ARC exception |
| `mail-delivery@retry-seconds`, `mail-delivery@days` | `300`, `5` | On an edge node with the MTA in moqui, the first wait before a delivery is tried again (doubled each time, to 4 hours), and when a message not delivered goes back as a bounce |
| `mail-delivery@mx-require-tls` | empty | On an edge node with the MTA in moqui, the recipient domains (comma-separated, `*` for every one) whose MX gets mail only over STARTTLS with a certificate valid for the host, as under an MTA-STS `enforce` policy whatever the domain publishes: an MX with no STARTTLS or a failed handshake or certificate is tried again later and the message bounces after `mail-delivery@days`, never sent in clear text. Empty: STARTTLS when offered, with any certificate, else clear text ([Mail-Inbound](interfaces/Mail-Inbound.md#mail-out)) |
| `mail-delivery@mx-override`, `mail-delivery@dns-offline` | empty | Testing and staging only: `domain=host:port` pairs (`*` for any) used in place of the MX, and `true` for no DNS query |
| `logging@level`, `logging@dir` | `info`, `runtime://log` | The console level, and the directory of `moqui.log` and `moqui-error.log` |
| `search-facade@backend` | `postgres` | `postgres` or `opensearch` ([search](#search)) |
| `example_disabled` (a property its `component.xml` reads) | `false` | `true` leaves out the example component and its gallery on a dev instance |
| `demo_disabled` (a property its `component.xml` reads) | `true` | `false` loads the demo lock-down on a dev instance; a demo server names the component in its conf instead |
| `llm-facade/endpoint[default]@url`, `llm-facade/endpoint[default]@api-key`, `llm-facade/model[default]@name` | `https://api.openai.com`, empty, empty | The default model endpoint for Assist and the LLM services (the dev conf points it at `http://127.0.0.1:1234`) |
| `llm-facade@default-model`, `llm-facade/endpoint[anthropic]@api-key` | `default`, empty | The model of every feature whose `bind` names none (Assist, titles, summaries, classify, AI fields in the default conf), and in the dev conf the Claude API key of its `claude-haiku` model (Claude Haiku 5.5); `moqui_conf__llm_facade__default_model=claude-haiku` with `moqui_conf__llm_facade__endpoint__ANTHROPIC__api_key` in `runtime/.env` puts the dev instance's features on Claude. The startup report's LLM line names the model Assist is on |

The database settings are in [Database and search](#database-and-search), and the cluster settings in
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
| `-p N`, `--port N`, `--http-port N` | Open the HTTP listener and use N in generated HTTP URLs. Pass `off` to close only the listener. |
| `--https-port N` | The HTTPS listen port (`webapp-list/webapp[webroot]@https-listen-port`), or `off` |
| `-c FILE`, `--conf FILE` | The runtime conf, as `MOQUI_CONF` does |
| `--no-run-es` | Do not start the OpenSearch in `runtime/opensearch` (`moqui_conf__search_facade__cluster__run=false`) |
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
`tools@empty-db-load` (default `seed,seed-initial,install`; `all` in the dev conf). It then starts its factory again,
and the log has:

```
Found 0 Enumeration records, loading empty-db-load data types (seed,seed-initial,install)
Data loaded into empty DB, re-initializing ExecutionContextFactory
```

Every start checks the tables. A datasource's `startup-add-missing` (default `true`) creates missing tables and adds
missing columns and indexes. Columns are never dropped or changed.

While the only accounts are the seeded ones nobody logs in as (`_NA_` and the agents' chat users) and
`user-facade/login@initial-admin-enabled` is `true`, the login page shows **Create Initial Admin Account**
([the first administrator](#the-first-administrator)). Once any person has an account, the form is gone and the
service refuses the call. A request from the server's own machine (a loopback peer, straight to the listener, with no
proxy's forwarding header) creates it as is; from any other address the form asks for the one-time setup secret the
server writes to its startup log while no account exists, and a restart makes a new one. On a machine other people
sign in to, `user-facade/login@initial-admin-local` `in-process` makes only a request of the server's own process this
machine's (the Desktop app's node sets it), and a starter that hands the secret to its own client sets it as
`user-facade/login@initial-admin-secret` (a secret of the conf, through its environment name), which the log then does not
show.
Two requests at once make one account: the create takes a row lock and asks again.

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

### The first administrator

`user-facade/login@initial-admin-enabled` is `false` in the default conf and `true` in the dev conf (and on the Desktop app's node). On a
server, set it for the setup, usually as a line in `.env` (`moqui_conf__user_facade__login__initial_admin_enabled=true`), restart, and take it out once
the account is made. While it is off and no account exists, the login page and the startup log say how to turn it on,
and the service refuses the call.

The account the form makes is in the `ADMIN` group. With **Also join Administrators - Advanced** checked it is in
`ADMIN_ADV` too: login as another user, the SQL runner, the SSH admin shell and host terminal, conf editing, component
deploy, load testing and hosted instances. These run code or reach data outside the apps, so leave it off unless the
account needs them; a system may have no member at all. `ADMIN_ADV` is a privileged group: only a member can add a
member. With no member, the operator adds one with a data load, with the server stopped on embedded Postgres (the
`userId` is on System › Security › User Account):

```sh
cat > adv.xml <<'EOF'
<entity-facade-xml type="install">
    <moqui.security.UserGroupMember userGroupId="ADMIN_ADV" userId="100000" fromDate="2026-11-17 00:00:00"/>
</entity-facade-xml>
EOF
moqui load location=$PWD/adv.xml
```

## Database and search

### Embedded Postgres

With the transactional datasource's `database-conf-name` `embedded` (the default; empty and Moqui Java's `h2` are the
same), the server runs its own Postgres. Every package but Enterprise
embeds it.

- **Data**: `runtime/db/embedded`, with two databases: `moqui` for the entities and `moqui_content` for the stored
  files ([Content database](#content-database)).
- **Binaries**: unpacked from the moqui binary on the first start, into `runtime/install/postgres` or under
  `MOQUI_INSTALL`.
- **Connections**: on Unix there is no TCP port. The server connects through a socket in the data directory, so
  nothing else on the network reaches the database.

### External Postgres or MySQL

Set the transactional datasource's attributes, by their names in `runtime/.env` or the environment, or on the
`datasource` element in the runtime conf ([Settings and the environment](#settings-and-the-environment)). There are
examples in `runtime/conf/MoquiDevConf.xml`. Use `mysql8` for MySQL 8 and `mysql` for MariaDB (which has no
`LATERAL`) and MySQL 5.7. Both create `utf8mb4` tables, so any character (an emoji) can be stored; create the database
with `CHARACTER SET utf8mb4`. A database made before, with 3-byte `utf8` tables, needs a fresh load or each table
converted by hand (`ALTER TABLE ... CONVERT TO CHARACTER SET utf8mb4`). Loading every data type into each was checked
([MySQL-Issues](../plans/archive/MySQL-Issues.md)).

```sh
moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres
moqui_conf__entity_facade__datasource__TRANSACTIONAL__host=db.internal
moqui_conf__entity_facade__datasource__TRANSACTIONAL__user=moqui
moqui_conf__entity_facade__datasource__TRANSACTIONAL__password=...
```

| Attribute of `entity-facade/datasource[transactional]` | Example | What it does |
|---|---|---|
| `database-conf-name` | `postgres` | `postgres`, `mysql` or `mysql8`; `embedded` (the default) is embedded Postgres |
| `host` | `db.internal` | Default `127.0.0.1` |
| `port` | | Empty is 5432 or 3306 |
| `database` | `moqui` | |
| `schema-name` | | Empty for the default schema |
| `user`, `password` | `moqui`, ... | |
| `sslmode` | | Empty is verified TLS. `verify-full` checks the authority and the name, `verify-ca` the authority only and needs `sslrootcert` (it trusts that file alone, as libpq does; without it the connection is refused), `require` encrypts without checking, and `disable` turns TLS off (for a local dev server only) |
| `sslrootcert` | `/etc/moqui/db-ca.pem` | A PEM file of the authorities to trust instead of the public ones |

`entity-facade@crypt-pass`, made into `seal.json` by `moqui operator init`, is the key of encrypted fields; it moves
with the database in the recovery bundle. The datasource's `password` is a bootstrap secret too, read before the
database opens: give it in the process environment (`moqui_conf__entity_facade__datasource__transactional__password`,
the unit's `Environment=` or a systemd credential) for the first start, then, once unsealed, keep it with `moqui
operator secret put entity-facade/datasource[transactional]@password` (read at the next start) and drop it from the
environment. A datasource's own `inline-jdbc` with `xa-properties` or a `jdbc-uri`,
as a conf from Moqui Java has it, replaces these attributes.

The content datasource has the same attributes ([Content database](#content-database)).

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

| `entity-facade/datasource[content]@database-conf-name` | Where the content database is |
|---|---|
| empty, embedded transactional database (default) | the database its `database` names (`moqui_content`) in the embedded cluster |
| empty, external transactional database | the transactional database itself, with no second database |
| `postgres`, `mysql` or `mysql8` | its `database` on that server |

For a server of its own, set its `host`, `port`, `user`, `password`, `sslmode` and `sslrootcert`
(`moqui_conf__entity_facade__datasource__CONTENT__host` and the rest). Each one left empty takes the transactional
datasource's. So setting only `moqui_conf__entity_facade__datasource__CONTENT__database_conf_name=postgres` puts the
content in a `moqui_content` database on the transactional server, which must exist. Its `schema-name` names a schema,
as the transactional one's does.

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

- set `moqui_conf__search_facade__backend=opensearch`, and `search-facade/cluster@url` (default `http://127.0.0.1:9200`), plus
  `search-facade/cluster@user`, `search-facade/cluster@password` and `search-facade/cluster@index-prefix` as needed;
- `./moq opensearch-download` installs OpenSearch in `runtime/opensearch`;
- the server starts that install and stops it on shutdown when `search-facade/cluster@url` is on this host, nothing answers
  there, and there is no pid file. `moqui_conf__search_facade__cluster__run=false` (or `--no-run-es`) turns that off;
- `./moq opensearch-start` and `opensearch-stop` run it as a daemon with a pid file.

### Snapshots

`./moq save-db [name]` writes the embedded database to `runtime/save/<name>/moqui.dump`, and the content database beside
it to `moqui_content.dump`. It also writes the OpenSearch data when there is any. `./moq reload-save [name]` empties the
database and restores that save. `./moq load-save -n <name> [types]` resets the instance data as `clean-dev` does,
leaving the cargo artifacts alone, then loads and saves. Stop the server before a restore. These work on the embedded database only; an external database has its own tools.

### Backups

The job `backup_EmbeddedDatabase_daily` backs up the embedded clusters while the server runs. It is paused in the seed
data; set it running on System › Operations › Jobs, or with `moqui_conf__database_backup__scheduled=true`, which each boot applies
(`false` pauses it). It runs daily at 02:00 and keeps 7 daily, 5 weekly and 12 monthly archives, and every yearly one,
in `runtime/db/backup`, or the directory in `database-backup@dir`.

- **Modes**: `backupMode=dump`, the default, writes a `pg_dump` file of each database: `moqui-<stamp>-d.dump` and
  `moqui_content-<stamp>-d.dump`, with the same stamp. `incremental` takes `pg_basebackup` fulls and block
  incrementals of the whole cluster, both databases in one copy. Each incremental is taken from the generation's full;
  with `chainIncrementals=true` it is taken from the archive before it, which holds only a day's changed blocks.
- **Sealed dumps**: with `database-backup@recipient` set to an age recipient (`age1...`, from `age-keygen`), each dump
  is sealed to it as `pg_dump` writes it, `moqui-<stamp>-d.dump.age`, and only the sealed bytes reach the disk: a backup
  directory that another system copies holds no plain copy of the database. A recipient that does not parse fails the
  backup. A physical backup is not sealed. `moqui save-db <cluster> <file>.dump.age --recipient age1...` seals the same
  way. Keep the identity off the server and apart from the backups: without it they do not restore. An encrypted
  mirror seals such a dump again, to its own recipient, so its copy needs both identities; with this set, leave
  `mirrorEncrypt` off.
- **Mirror**: set `systemMessageRemoteId` (the seed job's is `EmbeddedDbBackup`) and on that remote `sendUrl`, the
  user, and a password or key. A remote marked `disabled` receives nothing. A mirror host on this machine or the
  private network (a LAN backup server) needs the remote's `allowPrivateNetwork` set to `Y`; empty is `N` on a
  production instance, and a link-local or metadata address is refused always. The seed `EmbeddedDbBackup` remote is
  not changed from System › Integrations, so set it with a data file of your runtime component, loaded with
  `moqui load` (`<moqui.service.message.SystemMessageRemote systemMessageRemoteId="EmbeddedDbBackup"
  sendUrl="backup.lan" username="backup" allowPrivateNetwork="Y"/>`), or in Tools › Data as an administrator. The remote directory or key prefix is its message type's `sendPath`. Only the archives
  the remote lacks are sent; a promotion is a rename there, and an archive the ladder drops is deleted there. An
  incremental whose parent a promotion renamed is sent again (over plain SFTP only its `moqui-backup.txt`), so every
  copy names its parent as it is now called. With `incremental` and `chainIncrementals`, a night sends one
  incremental. Two job parameters shape it:
  - `mirrorTransport`: `sftp` or `s3`. Empty picks `s3` for a `sendUrl` of `s3://<bucket>`, else `sftp`. For S3 add
    `?region=<region>` (default `us-east-1`) and, for a store other than AWS (MinIO, R2, B2),
    `&endpoint=https://<host>`; the username is the access key, the password the secret key. An `http://` endpoint is
    taken only on an instance that is not production, for this machine or a store on the private network.
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
  of the same name beside it (`moqui_content-<stamp>-d.dump` for `moqui-<stamp>-d.dump`). A sealed dump takes
  `--identity <file>` (or `-` for stdin), the identity of its recipient; each sealed dump is decrypted whole into an
  owner-only file under `runtime/db` before anything is restored, and that file is deleted after. A physical backup is put
  together with `pg_combinebackup`: the full, then each incremental in order up to the one restored (each names its
  parent in `moqui-backup.txt`). An encrypted archive is decrypted first: `age -d -i key.txt <name>.age > <name>`, or for a directory
  `mkdir <name> && age -d -i key.txt <name>.tar.gz.age | tar xz -C <name>` (`tar xz` alone for a plain `.tar.gz`).
- **Not included**: `.env` and the field-encryption key are not in the archive. Keep them with the backup.

The details are in [plans/archive/PostgresBackup.md](../plans/archive/PostgresBackup.md). The job does not back up an
external database. `org.moqui.admin.BackupServices` reads the job's runs and the archive directory for an application's
backup screen (IronStone Admin › Data › Backups): coverage as the runs recorded it, the archives as names and sizes, and
a backup asked for now with the job's own parameters ([Attack-Surface](security/Attack-Surface-Admin.md)).

## Shared runtime (~/.mr-moqui)

On macOS and Linux, `moqui install` installs the running binary and its embedded files under
`~/.mr-moqui`. Add that directory to PATH and run `moqui --installed` from any directory. The
installation has the same layout as an unpacked runtime archive. One directory holds its binary,
configuration, components, and instance data. Set `MOQUI_SHARED_RUNTIME` to another absolute
directory or to `off` to disable it.

`moqui install --from DIR` copies a runtime or unpacked archive, including its top-level
`MoquiConf.xml`. With no instance conf the usual development conf applies. Conflicting files are
listed before installation; use `--force` to replace them. A symbolic link in the source is left
out with a warning. Each install writes `.install-manifest`, the definition files it installed,
and the next install removes the ones its source no longer has (a file changed since it was
installed is listed as a conflict first). Instance data is never removed. `.install-version`
records the binary; a `moqui --installed` run by another binary logs a warning at boot.

`--with-data` also copies instance data and secrets, and refuses while the source's server or
embedded Postgres is running (a live `postmaster.pid`). The machine's own identity in a cluster
or an edge link (`db/instance_id`, `db/edge_member_key`) is left out and made new; `.env` and the
certificate keys are copied, with a warning, so the copy has the source's secrets and TLS
identities. The dev CA signing key stays on the original machine.

### Where runtime files come from

- **`moqui --installed`** (the first argument): `~/.mr-moqui` *is* the runtime, used exclusively.
  Files come from that directory, then from the binary for any it does not have. Nothing is read
  from the working directory or `MOQUI_RUNTIME`. This is by design for production systems and
  desktop installs, so there is one place the runtime's files come from. The boot logs
  `Runtime: installed at <dir> (moqui install); files come from here and the binary`.
- **Without `--installed`**: the runtime is the one found from the starting directory or set by
  `MOQUI_RUNTIME`. Its files on disk come first, then the files built into the binary, then the
  `components/` and `conf/` of `~/.mr-moqui` for what neither has (a shared add-on component; the
  dev CA in `~/.mr-moqui/cert/dev-ca` is read the same way, after the runtime's own). A stale
  shared copy never overrides the runtime or a newer binary. The boot logs `Runtime: <dir>;
  missing files from the binary, then <~/.mr-moqui>` when the shared directory is there.

The database, log, instance conf, environment and server identities always belong to the
runtime itself. `moqui embedded diff` shows the files that differ from the binary, naming the
local or the shared runtime. Windows supports checkout-local operation.

## HTTPS, ports and proxies

### Ports

| Port | Listener | Default | Properties |
|---|---|---|---|
| 8080 | HTTP | Off in dev, otherwise on every address | `webapp-list/webapp[webroot]@http-listen-port`, `webapp-list/webapp[webroot]@http-bind` |
| 8443 | HTTPS, certificates from `runtime/cert`; HTTP/2 or HTTP/1.1 by ALPN, and gRPC over HTTP/2 when a gRPC service is on | On when there is a certificate (dev makes one) or a name set up with ACME | `webapp-list/webapp[webroot]@https-listen-port` (`off`), `webapp-list/webapp[webroot]@http2` |
| 4317 | Dedicated gRPC | Off in dev, otherwise opened when a gRPC service is enabled | Webroot `grpc-listen-port`, `grpc-listen-bind` |
| 8444 | Cluster: TLS with the cluster certificate, client certificate required; gRPC on HTTP/2, or HTTP/1.1 | On in every serving process, every address | `cluster-member@https-port`, `cluster-member@bind-address`, `cluster-member@protocol` |
| `ssh-server@port` | SSH: SFTP, git, admin shell, the apps (`ssh -p 2222 john.doe@localhost` in dev) | Off; 2222 on 127.0.0.1 in the dev conf | `ssh-server@port`, `ssh-server@host` |
| `smtp-server@listen-port` | Inbound SMTP behind a mail gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)) | Off; Maximal and Enterprise only | `smtp-server@listen-port`, `smtp-server@listen-host` |
| `media-server@port` (UDP) | Call media: every browser leg of a call through the SFU ([WebRTC](interfaces/WebRTC.md)); with `media-server@cascade-enabled`, the relay links between the media nodes of a call too, so the nodes must reach each other's | Off; Maximal, Enterprise and Debug only | `media-server@port`, `media-server@host`, `media-server@public-address`, `media-server@cascade-enabled` |
| `media-server@tcp-port` (TCP) | ICE-TCP, for browsers that cannot reach the UDP media port ([TURN](interfaces/TURN.md)) | Off | `media-server@tcp-port` |
| `media-server/turn-server@port` (UDP and TCP), `media-server/turn-server@tls-port`, 443 on `media-server/turn-server@tls443-host`; `media-server/turn-server@relay-ports` (UDP) | The embedded TURN server and its relays ([TURN](interfaces/TURN.md)) | Off | `media-server/turn-server@port`, `media-server/turn-server@tls-port`, `media-server/turn-server@tls443-host`, `media-server/turn-server@host`, `media-server/turn-server@relay-ports` |
| `sip-server@port` (UDP and TCP), `sip-server@tls-port`; `sip-server@rtp-ports` (UDP) | SIP trunks, desk and soft phones (the registrar, with `sip-server/registrar@enabled`; `sip-server@tls-port` 5061 and `_sips._tcp` for remote phones), and the RTP of phone legs ([SIP](interfaces/SIP.md)) | Off; Maximal, Enterprise and Debug only | `sip-server@port`, `sip-server@host`, `sip-server@tls-port`, `sip-server@tls-host`, `sip-server@rtp-ports`, `sip-server@rtp-host`, `sip-server@public-address` |
| `xmpp-server@port` (5222), `xmpp-server@tls-port` (5223) | XMPP clients, STARTTLS and direct TLS with ALPN `xmpp-client` ([XMPP](interfaces/XMPP.md)); the WebSocket is on the HTTP listener (`/xmpp-websocket`) | Off; Maximal, Enterprise and Debug only | `xmpp-server@domain`, `xmpp-server@port`, `xmpp-server@host`, `xmpp-server@tls-port`, `xmpp-server@tls-host` |

XMPP clients find the server by DNS: `_xmpp-client._tcp.<domain>` (5222, STARTTLS) and `_xmpps-client._tcp.<domain>`
(5223, direct TLS, XEP-0368), both at the host's name; the certificates in `runtime/cert` must cover the domain,
`rooms.<domain>` and `upload.<domain>` (a wildcard of the domain does). `_xmpp-server._tcp` waits for federation (C3b).

`/health` answers `{"ready":true}` to loopback and to the addresses in `webapp-list/webapp[webroot]@status-ips`, and 404 to everyone else.
Through a reverse proxy whose client-ip header is believed, the client it names must be in `webapp-list/webapp[webroot]@status-ips` too:
list a monitor that probes through the proxy, or have it probe the port directly.

The gRPC listener (webroot `grpc-listen-port`, default 4317, at `grpc-listen-bind` or the web bind address) and the edge tunnel
ports (`edge@ssh-tunnel-port`, `edge@xmpp-port`, `edge@xmpp-tls-port`) also open only when they are configured.

Before the database opens, a start checks every port it will bind, over TCP or UDP as the listener uses it, and logs
`Ports checked free: …`. If HTTP, HTTPS, cluster or SSH is busy, the start stops, and the error names each busy port
and the process that holds it. If one of the other listeners has a busy port, the start logs a warning and goes on
without that listener. When the server is ready it logs one report: the instance purpose, every listener and its
address (or why it is off, failed or skipped), every integration, the tool factories, and what the build leaves out.
The report is a warning when something that was configured did not start. The instance purpose also has a line of
its own once the properties are set (`Instance purpose dev (from the conf), node role all`).

### HTTP/2

The HTTPS listener offers HTTP/2 and HTTP/1.1 by ALPN, so browsers and other clients that speak HTTP/2 use it; the
plain HTTP listener is HTTP/1.1 only. `moqui_conf__webapp_list__webapp__WEBROOT__http2=false` keeps HTTPS to HTTP/1.1.

- **WebSockets** (`/notws`, `/callws`, `/termws`, `/xmpp-websocket`) open on an HTTP/2 connection by extended CONNECT
  (RFC 8441), or on a connection of their own over HTTP/1.1, as the client chooses.
- **gRPC.** When the OTLP/gRPC receiver or the PASS authority's gRPC is on, the HTTPS listener takes their calls over
  HTTP/2 too, so a client can use the HTTPS port instead of 4317; the dedicated port can be disabled independently.
- **Limits.** One HTTP/2 connection carries at most `webapp-list/webapp[webroot]/web-protection/connections@http2-max-streams` (100) requests at once. One with no
  request for `webapp-list/webapp[webroot]/web-protection/connections@header-timeout` (30 seconds) is closed with a GOAWAY, as an idle HTTP/1.1 connection is closed,
  and so is a client's connection when web protection refuses that client.
- **Slow readers.** A connection whose client takes nothing of a response for `webapp-list/webapp[webroot]/web-protection/connections@write-timeout` (60 seconds) is
  closed, a WebSocket's included; a download that keeps moving is not. A WebSocket keeps its connection's place in
  `webapp-list/webapp[webroot]/web-protection/connections@max` and web protection's `max-per-client` until it closes.
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

In dev (`moqui_conf__instance__purpose=dev`), an empty certificate directory gets two leaves with one key. `localhost`
names only `localhost` and is served when the client asks for that name. `default` also names 127.0.0.1 and ::1 and is
served when the client sends no name, as for `https://127.0.0.1:8443`. gRPC clients built on BoringSSL (grpcio, so
the Python, Ruby, PHP and C++ OTLP exporters) refuse an address under the intermediate's IP constraints, so point them
at `https://localhost:8443`. The server sends a leaf with its constrained signing intermediate. The public root is
`cert/dev-ca/ca.crt`. An issuer made before 2026-11-25 lacks the authority key identifier that Python's strict
verification wants: `moqui dev-cert --rotate`, then `./moq trust-dev-cert`.
Run `./moq trust-dev-cert` once per user to establish the shared issuer and trust the root in the
macOS login keychain or Linux browser NSS stores (`~/.pki/nssdb` for Chromium and Chrome, their snap and Flatpak
stores, and Firefox's profiles). A system-store client on Linux can use
`curl --cacert runtime/cert/dev-ca/ca.crt https://localhost:8443/health` or `--system` trust.
Use `--untrust` at the same scope to remove trust; with the shared root gone, it removes every root named
`MR Moqui development root`. Windows clients can use the runtime-local public root. The host's own clients (`moqui tui
--server`, the devtools commands) trust the dev root only for loopback names, and only a `ca.crt` the current user
owns that no other user can write.

The root is valid ten years and its key is dropped once it signs the intermediate; the intermediate, whose key stays in
`~/.mr-moqui/cert/dev-ca/signer`, is valid two years. `moqui dev-cert --rotate` makes a new root and intermediate and
new leaves; run `./moq trust-dev-cert` again to trust the new root (it removes the old one). Managed leaves renew within
30 days of expiry, at boot and once a day while a dev server runs. A runtime with an older self-signed
`cert/localhost.*` (an unmanaged localhost or default identity) keeps serving it, and `./moq trust-dev-cert` stops
naming the fix: `moqui dev-cert --replace` (`./moq moqui dev-cert --replace` in a checkout) moves the pair aside into
`cert/replaced-<time>` and issues a managed leaf; a key the seal held for that pair is replaced at the server's next
renewal. Outside dev with no
certificate, the HTTPS listener does not start until a name is set up with ACME (below) or a certificate is added to
`runtime/cert` (seen within a minute). A `webapp-list/webapp[webroot]@https-listen-port` set explicitly with no certificate no longer fails
the start: the listener waits the same way.

**System › Certificates** lists the names this node answers (its own host, its sites' hosts, the SMTP host, the XMPP
domain, an edge node's terminate hosts, the files in `runtime/cert`) and the certificate each is served: its issuer,
its expiry and its file. A name with none gets no HTTPS.

**From ACME.** Set up with ACME, on that screen, gets a name's certificate from an ACME authority: Let's Encrypt
(`acme@directory-url`), its staging directory for a first try, or any other authority's directory URL. Nothing is ever
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
  challenge and every member writes its certificate file and holds its key within a minute. The key is served from
  its encrypted row and never written to `runtime/cert` (a `privkey.pem` an earlier build wrote is removed).
- **Account.** The account key is derived from `acme@account-key`, a secret generated into the `SecretValue` table on
  first use, which the members of a cluster share in their database. For a company's own authority,
  `acme@directory-ca` names the root its HTTPS is checked against.
- **Not built:** HTTP-01 (port 80), DNS-01 and so wildcard names.

### Listen ports and URL ports

`webapp-list/webapp[webroot]@https-listen-port` is the port the server binds. `webapp-list/webapp[webroot]@https-port` is the port clients see in generated URLs
and https redirects; empty is 443, as in Moqui Java. To make generated URLs use https, set `moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=true`.

| Clients reach the server | Set |
|---|---|
| Directly on 8443 | `moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=true`, `moqui_conf__webapp_list__webapp__WEBROOT__https_port=8443` |
| On 443, through a port redirect or a proxy | `moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=true`; leave `webapp-list/webapp[webroot]@https-port` empty |
| On 443, which the server binds itself | `moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=true`, `moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port=443`, and the capability to bind it (below) |

A firewall redirect from 443 to 8443 (nftables or iptables) keeps the server unprivileged. To bind 443 directly under
systemd, add a drop-in `/etc/systemd/system/moqui.service.d/https-443.conf`:

```ini
[Service]
AmbientCapabilities=CAP_NET_BIND_SERVICE
Environment=moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port=443
```

### Behind a proxy or load balancer

The server terminates HTTPS itself, so a proxy is optional. A proxy or load balancer in front needs these settings:

- `webapp-list/webapp[webroot]@trusted-proxies` lists the proxies' address ranges (`10.0.0.0/8`, `fd00::/8`, or single addresses). Only
  those peers are believed for `X-Forwarded-Proto` and the client address header. The client address is then the
  rightmost address that is not a trusted proxy.
- `webapp-list/webapp[webroot]@client-ip-header` names the header: `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP` or
  `CloudFront-Viewer-Address`.
- Each trusted proxy must set or replace `X-Forwarded-Proto`, `X-Forwarded-Host` and `X-Forwarded-Port` (or append its
  own value to each, as it appends to the client address header), never pass a client's on alone. The server reads each
  from the hop it reads the client address from (the value at that hop's place when the header has as many values,
  else the last), so a value a client sent ahead of the proxy's is not taken; a proxy that forwards a client's header
  untouched lets the client choose the scheme or host its request counts as (SEC-WEB-91).
- Setting `webapp-list/webapp[webroot]@trusted-proxies` also turns on the per-client web protection: rate limits, connection limits and
  bans by client address. Without a proxy, `moqui_conf__webapp_list__webapp__WEBROOT__web_protection__enabled=true` turns it on.
- A `rate-limit` or a client, path or user-agent rule with `host="shop.example.com"` (or `*.example.com`) applies only
  to that site, on top of the rules without `host`, so one site can be stricter than the others.
- A ban is shared with the other cluster members and up the edge link: a ban a local node starts (a sign-in that
  keeps failing) is told to its edge node, which refuses that client for that node's host names only. An edge node
  tells the nodes behind it nothing, as its bans may come from another pairing's visitors. System › Security › Web
  Protection lists what each rule refused and the bans held now, and lifts one.
- Leave `webapp-list/webapp[webroot]@https-port` empty when the proxy serves 443.

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

- The file system is read-only to it (`ProtectSystem=strict`) but `/opt/moqui` and `/opt/moqui/components`
  (`ReadWritePaths`): the database, logs, certificates, `.env` and deployed components. The binary, `MoquiConf.xml`
  and `conf/` are read-only inside it as well (`ReadOnlyPaths`), so a compromised server cannot change what runs at
  its next start, and root owns them. A conf edit is made as root and reloaded with `systemctl reload moqui`; a
  server that writes elsewhere under `/opt/moqui` needs a drop-in that adds the path to `ReadWritePaths`.

- It is `Type=notify`: the server reports when the listener is bound and when it stops. An in-app restart runs the
  binary again in the same process and reports a reload, so the unit stays up.
- `/etc/moqui/environment` is optional, for variables that must exist before the process starts (`MOQUI_CONF`,
  `MOQUI_RUNTIME`). The server reads `/opt/moqui/.env` itself.
- For a database on the same machine, `systemd/drop-in/` has `After=` and `Wants=` drop-ins for Postgres and MySQL.

### Sealed start

A server keeps its keys the way HashiCorp Vault does ([Sealed-Secrets](../plans/Sealed-Secrets.md) §3.3): it starts
**sealed**, opens no HTTP, SSH, cluster or other listener and starts no job until it is **unsealed**, and its root key
is then in memory only. While sealed it answers only on its operator socket. `runtime/db/seal.json` holds the root key
wrapped by the unseal method and by the recovery key, the bootstrap secrets and the `SecretValue` data key, each
wrapped, and nothing usable alone ([where secrets live](#where-secrets-live)).

**The operator socket** is root's: where it is and who may use it come from nothing the server's user can write (not
the conf, `runtime/.env` or the environment), so a child of the server can neither replace it nor read what an operator
enters there. Root and the socket's group run every command; the server's own user, and so every child and job of the
server, runs only `status`. Reach it at the console, or through the host's own sshd running `moqui operator`.

- **Under systemd**, `systemd/moqui-operator.socket` makes `/run/moqui-operator/moqui.sock` (the directory root's, mode
  0755; the socket root's, group `moqui-operator`, mode 0660) and hands it to the service. `systemd/moqui.sysusers`
  makes the group; add operators to it, never the `moqui` user.
- **In a container**, the entrypoint, still root, makes the same socket with `moqui operator bind
  /run/moqui-operator/moqui.sock [--group NAME] -- <command>` before it drops to the server's user, and the server takes
  it over. With no group, only root (`podman exec -u 0`) runs operator commands.
- **Anywhere else** (a development machine, macOS), the server binds `seal@socket`, default `runtime/db/operator.sock`,
  itself, mode 0600, and logs that it is not a protection boundary: its own user could replace it.

`moqui operator` reaches `/run/moqui-operator/moqui.sock` (always, when run as root), or `runtime/db/operator.sock` for
another user when that one is not there, or `--socket PATH`; it reads no conf for it.

```sh
moqui operator status                    # sealed, unsealing or unsealed; the method; the recovery shares
moqui operator init --method tpm         # make the root key, seal.json and the recovery shares (printed once)
moqui operator unseal                    # a passphrase or one share, echo off; an automatic method needs nothing
moqui operator unseal --recovery         # one recovery share, which opens any method's seal
moqui operator seal                      # drop every key from memory now and close the listeners
moqui operator rekey --method shamir     # wrap the root key under another method, with the recovery shares
moqui operator rotate-root               # a new root key, every wrap rewritten in one rename, with the recovery shares
moqui operator export-seal FILE          # the recovery bundle: seal.json and the database identity, mode 0600
moqui operator init --from FILE          # restore a bundle on a clean runtime, then unseal --recovery
moqui operator secret put NAME           # keep a secret (echo off, stdin, or --file PATH); never printed
moqui operator secret list               # the kept secrets' names, kinds and generations
moqui operator secret rm NAME
```

**Methods** (`seal@method`, or `init --method`):

| Method | Unattended reboot | For |
|---|---|---|
| `tpm` | Yes: `systemd-creds` with the TPM2, bound to its PCRs (`/usr/bin/systemd-creds`, or `seal@systemd-creds-command`, an absolute path) | A home or office server; the default for BOSS |
| `passphrase` | No: Argon2id at m=256 MiB, t=3; a production instance refuses less | A laptop, or the highest care |
| `shamir` | No: `seal@threshold` of `seal@shares` operators each enter a share | Separation of duties |
| `kms` | Yes: a cloud KMS key wraps the root key, and unwraps it at each start (below) | A cloud host; the default for an edge node in the cloud |
| `transit` | Yes: an OpenBao or Vault Transit key wraps it (below) | Where an OpenBao or Vault runs already; a managed instance, whose manager keeps its transit key |
| `dev` | Yes: the key is in `seal.json` itself, so its recovery bundle carries the root key in the clear | Development and tests only; refused when `instance@purpose` is `production` |

**`kms` and `transit`.** No cloud SDK: each is a few HTTPS calls (S11). `seal.json` keeps only the wrapped root key;
where it is sent and whose credential goes with it are `seal` attributes read from the conf files only (an environment
name or `runtime/.env` that sets one is ignored, with an error logged), so a writer of `seal.json` or `.env` cannot send
the server's credential elsewhere. A credential is never written in the conf: `seal@kms-credentials` and
`seal@transit-token` name a file of root's (an absolute path; mode 0400 or 0440 with the server's group, in a directory
of root's: one the server's own user owns, or that its group may write or others may read, is refused), or
`systemd:NAME`, the unit's `LoadCredential=`/`LoadCredentialEncrypted=` credential `NAME`, looked up only when the
method reads it; `kms-credentials` empty uses the host's own identity. `kms-endpoint`, an Azure key URL and
`transit-url` must be https (a loopback address excepted); the metadata addresses stay http, and Azure's sign-in
endpoint is https as its client secret goes there. `transit-ca-file` replaces the system's authorities for the Vault's
HTTPS.

| Provider | Settings | Calls | Credentials |
|---|---|---|---|
| AWS KMS (`kms-provider="aws"`) | `kms-key` (key id, ARN or alias), `kms-region` | `TrentService.Encrypt` and `Decrypt`, signed with SigV4, encryption context `moqui-seal=root-key` | The instance role by IMDSv2; or a file `{"access_key_id", "secret_access_key", "session_token"}`. The role needs `kms:Encrypt` and `kms:Decrypt` on the key |
| GCP Cloud KMS (`kms-provider="gcp"`) | `kms-key` (`projects/P/locations/L/keyRings/R/cryptoKeys/K`) | `:encrypt` and `:decrypt`, additional authenticated data `moqui-seal root-key` | The metadata server's token of the VM's service account; or a service account's JSON key file. It needs `roles/cloudkms.cryptoKeyEncrypterDecrypter` |
| Azure Key Vault (`kms-provider="azure"`) | `kms-key` (`https://VAULT.vault.azure.net/keys/NAME`) | `wrapkey` and `unwrapkey`, RSA-OAEP-256 (an RSA key); the key version the wrap used is kept, and taken only when it is a version of the conf's key. RSA-OAEP is a public-key wrap, so anyone could make a blob that unwraps to a root key of their choosing: the wrapped key is signed with the key (`sign`, RS256) and the signature checked (`verify`) before each unwrap | The managed identity by IMDS; or a file `{"tenant_id", "client_id", "client_secret"}` for the client credentials grant. It needs the key's wrap, unwrap, sign and verify permissions |
| OpenBao or Vault Transit (`method="transit"`) | `transit-url`, `transit-key`, `transit-mount` (`transit`), `transit-namespace`, `transit-ca-file` | `POST /v1/<mount>/encrypt/<key>` and `/decrypt/<key>` with `X-Vault-Token` | `transit-token`: a token whose policy allows `update` on those two paths |

`kms-endpoint` and `kms-token-endpoint` replace the provider's API and token addresses (a private endpoint). A failed
call at a start leaves the server sealed with `STATUS=sealed: auto-unseal failed` and the reason in `moqui operator
status`; the recovery shares open it, and `moqui operator unseal` tries the method again. Move a server to `kms` or
`transit` with `moqui operator rekey --method kms` (or `transit`) once the settings are in the conf.

```xml
<seal method="kms" kms-provider="aws" kms-key="alias/moqui-seal" kms-region="eu-west-1"/>
<seal method="transit" transit-url="https://bao.example:8200" transit-key="moqui-erp" transit-token="systemd:bao-token"/>
```

The method is fixed at init: `seal.json` records it, and a conf, `.env` or environment that names another is ignored
and logged, so whoever can write those files cannot move the server to the `dev` seal. Only `rekey` changes it.
`seal@socket`, `seal@systemd-creds-command`, `seal@unseal-max-delay-seconds` and the `kms-*` and `transit-*`
settings are read from the conf files only: an environment name or `runtime/.env` that sets one is ignored, with an
error logged. The `tpm` program is never taken
from `seal.json`, so a restored bundle cannot name a program to run.

**The first start with no `seal.json`.** A dev or test instance makes the `dev` seal by itself, so `./moq run` and the
tests start as before. A production instance waits sealed for `moqui operator init`, unless `seal@method` names an
automatic method (`tpm`, `kms`, `transit`): then it makes its seal and starts, and holds the recovery shares in memory, logging an error
until `moqui operator init --take-shares` reads them once (they are lost if the server stops or is sealed first).
`moqui load` and other commands that boot a runtime open its seal with no operator: an automatic method, or on a
production runtime with no seal yet a root key in memory only, never a seal of their own; a `passphrase` or `shamir`
seal is opened only on the server's socket. A server on a host with no Unix socket (Windows) does not start in
production without its `seal.json`.

**Recovery shares** (3 of 5 by default, `seal@threshold` of `seal@shares`) are made at init for every method but
`dev`, printed once and never stored on the server: give each to a different holder. They unseal when the TPM or the
passphrase is lost, and they authorize `rekey` and `rotate-root`. `rekey` keeps the root key and the recovery shares;
`rotate-root` makes a new root key, writes the new wraps beside the old ones and switches `seal.json` in one rename (a
crash before it leaves the old root in use), and keeps the recovery shares. Run `rotate-root` first after a process
compromise. A cluster member unseals on its own with its own `seal.json`; a new member is unsealed the first time
with a copy of the bundle and the recovery shares, then rekeyed to its own method.

**Unseal attempts.** One runs at a time. After a wrong passphrase or share the next attempt waits one second, then
twice as long after each one more, up to `seal@unseal-max-delay-seconds` (300), and an alert is logged. A wrong share
is refused when it is entered: `seal.json` keeps a salted hash of each.

**systemd.** The unit is `Type=notify`. A server waiting for a passphrase or shares sends `READY=1` with
`STATUS=sealed` once its socket is open, so `TimeoutStartSec` does not end it; an automatic method's server is ready
once unsealed and listening. A failed automatic unseal leaves it sealed and ready with `STATUS=sealed: auto-unseal
failed`, for the recovery shares. `seal` stops the listeners, the jobs and the embedded Postgres and says
`STATUS=sealed` (not stopping), and the process waits sealed for `unseal`, even with an automatic method: the file
`db/sealed-by-operator` keeps a restart, or a reboot, sealed until `unseal` too. A signal while sealed, or after the
unseal and before the listener opens, ends the process. The unit takes the socket from `moqui-operator.socket` and sets
`LimitCORE=0`. The server logs an error when it runs as root, since every child of it would be root too.

#### Where secrets live

No secret is in `runtime/.env` or the runtime directory ([Sealed-Secrets](../plans/Sealed-Secrets.md) §3.1, S4):

| What | Where | Kept by |
|---|---|---|
| Bootstrap secrets, needed before the database opens: `entity-facade@crypt-pass` and `@crypt-pass-old`, each datasource's `password` (the embedded Postgres superuser's is the transactional datasource's), a cluster member's own edge key (`edge-member-key`) | `seal.json`, sealed under a key derived from the root key | `init` makes the crypt pass, the first start the embedded password and the edge key; an operator puts the others |
| Every other secret: the generated keys (`session-config@cookie-key`, `ssh-server@host-key`, `git-server@lfs-token-key`, the cluster TLS pair, `edge@link-key`, `acme@account-key`, `push/web@vapid-key`, the DKIM key, ...), what pairing and the edge setup write, and what an operator puts | the `moqui.security.SecretValue` table, AES-256-GCM under the SecretValue data key with the name and generation as associated data | Made on first use; shared by every member of a cluster through the database |
| ACME certificates' keys | their `AcmeCertificate` row, encrypted | Served from memory; only `fullchain.pem` is written |
| An operator's own TLS key | the secret `tls-key/<name>` beside `runtime/cert/<name>.crt` | `moqui operator secret put tls-key/<name> --file <key>`; a key file still serves, and is named in a warning |

`moqui operator secret put NAME` keeps a secret by name, once unsealed: a conf selector the conf leaves empty or at
`CHANGEME` (a value the conf files or the process environment give wins over it), or a name of its own such as
`tls-key/<name>`. A bootstrap secret goes to `seal.json`, so export the recovery bundle again after it, and is read at
the next start; another goes to `SecretValue` once the database is open, and is read by this server at once and by the
other members of a cluster within 30 seconds (each reads the rows again then). A new `entity-facade@crypt-pass` needs
`--force`, and the pass it replaces becomes `entity-facade@crypt-pass-old`; `secret rm` refuses a crypt pass or a
datasource password. Putting a secret removes a line an earlier build kept for it in `runtime/.env`. The value is read with the terminal's
echo off, from standard input (a whole PEM piped in), or with `--file`, and is never printed: `secret list` gives names,
kinds and generations only. A script's `System.getProperty` answers no name the store keeps. Admin's write-only fields
(connections, OAuth providers) keep their own encrypted rows. Code reads a secret by its handle
(`ExecutionContextFactory::secret`, `conf_env::secret`), a value cleared when dropped.

The embedded Postgres password is handed to the cluster's tools in their environment (`PGPASSWORD`) only while they run
(`moqui save-db` and `load-save` read it from the runtime's seal too), and initdb reads it from a new owner-only file
that is removed once it ran, or failed; no `pgpass` is kept. A `pgpass` or `db/edge_member_key` an earlier build left
is moved into `seal.json` at the next start; one that holds another value than the seal's is kept, and named in an
error. A member's own edge key is not in the recovery bundle: a member restored from a bundle makes its own. A runtime from before has its
other secrets in `runtime/.env`: under the `dev` seal, or before a seal is made, they are still read, each named in a
warning; under any other seal they are ignored, each named in an error, since the server's own user can write `.env`.
No migration moves them; put each with `moqui operator secret put` and remove its line, or start a new runtime.

The members of a cluster share the `SecretValue` rows through their database, and each needs the same bootstrap
secrets and SecretValue data key in its own `seal.json`: start a new member from the first one's recovery bundle
(`moqui operator init --from`, then `rekey` to its own method), never with a seal of its own: a member whose seal opens
none of the database's `SecretValue` rows refuses to read them, with an alert. `init` makes the crypt pass and the
SecretValue data key, so the bundle exported right after it carries them; export it again once the first member serves,
for the embedded cluster's password. Copying `.env` between members carries no secret now.

A boot with no seal (`moqui tui`, the desktop app or a tool on a production runtime before `moqui operator init`) holds
its root key in memory only and keeps nothing: the crypt pass is unset unless the conf files or the environment give
one, so the first encrypted field written or read fails with an error naming `moqui operator init`; a session or
signing key it makes lives in that process only, logged as an error; and `secret put` is refused. `moqui load` refuses
such a runtime outright, unless the environment gives the crypt pass. Run the server and make the seal first.

**Backup and restore.** A database backup is useless without the matching `seal.json`. Write the recovery bundle at
init and after each `rekey`, `rotate-root` and bootstrap-secret change, and keep it with the offline backups, apart
from the shares. To restore on a clean host: start the new server (it waits for init), `moqui operator init --from
bundle.json`, `moqui operator unseal --recovery` with the threshold of shares, then `moqui operator rekey --method tpm`
for the new host's TPM. The old host's TPM is not needed.

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
   sudo chown -R root:root /opt/moqui/moqui /opt/moqui/MoquiConf.xml /opt/moqui/conf
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
| PDF | Chromium: `./moq chromium-download` into `runtime/chromium`, or `screen-facade/pdf@chromium-path`. The server starts without it, and PDF is unavailable |
| OpenSearch | [Search](#search) |
| Outbound mail | SMTP through a `moqui.basic.email.EmailServer` record; every package has it |
| Inbound mail | IMAP polling, or the SMTP listener behind a gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)); Maximal and Enterprise |
| SSH | `ssh-server@port`, with `moqui_conf__ssh_server__host=0.0.0.0` to reach it from other machines ([TUI](ui/TUI.md), [Sync](interfaces/Sync.md), [Git-Hosting](interfaces/Git-Hosting.md)) |

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
  | `refuse` | The root's own screens and the shared static files only. `/Login`, `/apps`, `/qapps`, `/rest`, the token routes, the servlets, the sockets, `/dav`, `/health`, `/metrics` and the gRPC services of the HTTPS listener are Not Found there, so nobody signs in to the framework on the host. Git and CI runners are the exception, each behind its own authentication and web protection: `/git/` (smart HTTP and LFS), git at a public site's page URL on a host that is also a `public-site`'s, and `/ci/runner/`. Use it for a public site |

- **Links.** A link made in a request to such a host keeps that host, with the webapp's scheme and port: only a host a
  `root-screen` pattern names counts, as a plain host name, never another `Host` header. On a `refuse` host it is
  `webapp-list/webapp[webroot]@http-host`'s, as a link made outside a request (mail, a report, a job) always is: such links are often to the
  apps, which that host does not serve.
- **Sessions.** The session cookie is tied to its host, with no cookie domain: a person signs in to each host. Over
  https it is `__Host-moqui.session`, which no other host of the domain and no plain-http page can set or shadow; a
  browser that still holds a `moqui.session` cookie from an older server signs in once again. A
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
a schedule with `./moq reload-save --release demo`, with the server stopped. Do not run a demo as `moqui_conf__instance__purpose=dev`:
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
    |  443, 25, 3478, 10000 (UDP), 5060/5061, 5222/5223, edge@ssh-tunnel-port
 edge node      conf/edge/MoquiEdgeConf.xml    base components and mr-pass; the link's queue and copies
    ^  the local node dials 443 and keeps one link open
 local node     conf/edge/MoquiLocalConf.xml   the apps and the data, in the office or at home
```

The confs, the `.env` names each node needs and the commands are in
[runtime/conf/edge](../runtime/conf/edge/README.md); each conf's comments say what each setting does. The edge node
runs the Maximal package, or Enterprise with an external database (both have the link), with the base components and
mr-pass (`./moq component-get edge`). Its HTTPS listener needs a WebPKI certificate ([Certificates](#certificates)),
which local nodes check when they pair, and no TLS-terminating proxy in front: the link needs ALPN and client
certificates end to end. If a proxy is in front of it, name it in `webapp-list/webapp[webroot]@trusted-proxies`; on an edge node a
client-IP header with no trusted proxies is ignored.

The browser terminal goes through too, in both web access modes: a client of the edge node is another machine to the
local node, so a dev local node's command line and shells need `moqui_conf__terminal__remote=true` and a recent sign-in there, and a
dropped link resumes each terminal by the count of what it drew ([Terminal](ui/Terminal.md#the-desktop-and-mobile-apps-and-phones)).

**Attach a local node in one sitting.**
1. Set up the edge node from its README: its `.env`, a certificate, and a start with `conf/edge/MoquiEdgeConf.xml`. In
   its System › Edge Clients › Clients, make a pairing code. Clients also lists the paired nodes, pings and revokes
   them, and shows the edge node's link key fingerprint; Domains has their host names and what each claims, and Ports
   their tunnel ports.
2. On the local node, in System › Edge Node, choose **Pair with an edge node**. Pairing writes `edge/node@url`,
   `edge/node@key`, `edge/node@pin` and `edge/node@id` to `runtime/.env` and brings the link up; unpairing removes them.
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
| SSH through the tunnel port | [`MoquiEdgeConf.xml`](../runtime/conf/edge/MoquiEdgeConf.xml)'s comments; [Attack-Surface](security/Attack-Surface-Cluster-Edge.md#edge-node), SSH tunnel port |
| Git hosting and CI: the URLs, the SSH tunnel's clone URLs, uploads, refusing hosts, runners' tokens | [Git-Hosting](interfaces/Git-Hosting.md#behind-a-front-or-edge-node-and-in-a-pool), [Pipelines](interfaces/Pipelines.md#behind-a-front-or-edge-node-and-in-a-pool) |
| Work over the link: delegated services, the queue, replicated sets | the `<edge>` element in `MoquiEdgeConf.xml` and `MoquiDefaultConf.xml`; [plan](../plans/archive/Edge-Node.md) §5.2 |

System › Edge Node shows the outbox and the replicated sets, with Pause and Resync; Edge Clients shows each node's
queue and sets. The edge node's database holds the queue, and its conf sets the backup job running
(`database-backup@scheduled`).

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

- **Conf.** Run `conf/all-in-one/MoquiAllInOneConf.xml` (`moqui_conf__instance__node_role=all-in-one`) with the server's full runtime, on
  the Maximal package. The `.env` names and the commands are in
  [runtime/conf/all-in-one](../runtime/conf/all-in-one/README.md). It needs what an edge node needs: a WebPKI
  certificate on the HTTPS listener, `edge@public-addresses`, and the edge node's mail, calls and SIP properties in
  `.env`. It takes the link and never dials an edge node itself. Its conf sets the backup job running: the queue is in
  the server's database.
- **Each item has one owner.** The server's own host names, mail accounts, chat bridges, numbers, calls and XMPP
  domain are served here as on a normal node; the local node's `EdgeHost` names, recipients, bridges, numbers and calls
  go over the link. A mail domain may be split by address.
- **Claims both sides make.** The server's own items win. The wizard's Configure refuses the local node's bridges,
  XMPP domain, SIP domain, numbers and a mail domain the server holds whole (its `mail-delivery@bounce-domain`). A
  replicated record that claims a server's item is not applied: System › Edge Clients › Domains lists it under Replica conflicts,
  with Check again. The server's own setup refuses an address, bridge or number the local node holds. An address both
  sides hold gets 451 at `RCPT TO`, and a bridge both hold gets 409 at its webhook; each is logged.
- **XMPP.** The server's XMPP server keeps `xmpp-server@port` and `xmpp-server@tls-port`. Give the local node's domain tunnel ports
  of their own, such as `moqui_conf__edge__xmpp_port=5242` and `moqui_conf__edge__xmpp_tls_port=5243`, named in that domain's SRV records. A
  tunnel port the same as the server's own is off, with an error logged.
- **PASS.** The server's mr-pass keeps its local authority (`pass@backend-url` is not `edge:`). PASS sites for the
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
   wizard on the local node. Give each local node its own `xmpp-server@domain` and `sip-server@domain` first.
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
  sessions. Call legs have a cap per local node (`edge@call-legs-per-pairing`).
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
2. Copy its whole `.env`, with what pairing wrote, to each other member, add that member's own `cluster-member@base-url`,
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
- **Git hosting and CI** behind the edge node and in the pool (the URLs to set, the SSH tunnel, the shared SSH host
  key and LFS key): [Git-Hosting](interfaces/Git-Hosting.md#behind-a-front-or-edge-node-and-in-a-pool),
  [Pipelines](interfaces/Pipelines.md#behind-a-front-or-edge-node-and-in-a-pool).
- **Drain the last live member** only to take the pairing offline: its names get the offline page, or a refused
  connection, until it resumes.

**Balancing.** The edge node spreads a pairing's work over its members that are linked and live, each kind its own way:

| Traffic | To the member |
|---|---|
| A terminate host's requests | Of the session cookie (`__Host-moqui.session` over https, else `moqui.session`, or the host's own, set on Edge Clients › Domains), by rendezvous hashing over the live members, so a browser stays on one member and every edge node picks the same; with no cookie, in turn. A WebSocket as its opening request |
| A passthrough host's connections, SSH and XMPP tunnels | Of the client's address, an IPv6 address by its /64 |
| PASS authority calls | Of the session handle while a sign-in is in progress, else the one with the fewest calls in flight |
| Delegated calls | Of the call's conversation (its call session, else its phone call), so one call's events reach one member |
| Call signaling | The member whose socket bound the leg |

A member whose requests fail or time out three times in a row is set aside for 30 seconds, then tried again; with every
member set aside they are all tried. A request that fails before any byte of an answer goes once more to another
member when it is a GET or HEAD with no body, or when it was never sent; nothing else is sent twice. A member that
stops moves only its own clients, and they stay signed in: sessions are rows in the pool's database. The defaults are
the `<edge>` element's `sticky-cookie`, `set-aside-failures` and `set-aside-seconds`. A forwarded request waits for its
answer while its body keeps moving, a gap of `forward-idle-seconds` (120) at most, and then `forward-head-seconds`
(3600) once it is all sent, so an upload answered once it is stored (Git LFS, CI artifacts) is not cut off. A GET or
HEAD with no body waits `forward-first-byte-seconds` (30) for its answer's head, so one held by a hung member goes to
another and the hung one is set aside; a first-byte timeout that goes to no other member (there is none, or it was the
second try) is not counted against the member, and any other request with no body (a POST) is never sent again and
waits `forward-head-seconds`. The edge node's own `request-timeout` does not apply to a request it forwards;
the member's does. System › Edge Clients › Balancing shows each member's requests in flight and in all and whether it
is set aside (`get#EdgeBalancing`), and the `moqui.edgelink.requests`, `retries` and `set_asides` counters name the
pairing and the member.

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

1. Start the first member once, so it generates its secrets into `.env`, among them `edge@link-key`, the one link key
   every member presents and every local node pins.
2. Copy that `.env` to each other member before its first start, and give each member its own `cluster-member@base-url` and
   `edge@link-url` (its own public `https` address, where local nodes dial it), and its own mail, media and SIP
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
- **A clustered all-in-one server** takes links on every member the same way: `moqui_conf__instance__node_role=all-in-one` on each, on one
  external database.

**Before the cluster** a front node can face visitors and mail servers: [Full stack](#full-stack). **Not built**:
cascading call media between edge members (so a phone on one member rings for a call whose media is on another).

## A front node

A front node has the front option set alone (`moqui_conf__instance__node_role=front`, [plan](../plans/Edge-Node2.md#47-a-front-node) §4.7):
web access by host name, terminate or passthrough; load balancing over a pairing's members, sticky by the session
cookie, with health and set-aside; web protection with rules per host; TLS and certificates; TCP ports; the offline
page. It has none of the edge set (no mail, call media, TURN, SIP, XMPP server, queue, delegated calls, replication or
mr-pass) and starts none of it even with its ports set. Its database holds its pairings, host names, ports and
certificates, and no business data. It dials no other node: the nodes behind it pair with it and dial it.

Before a local cluster it is the cluster's load balancer and web protection, with none of the edge node's comms: the
cluster keeps its own mail, chat, calls and phones, or has none. Before an edge cluster it is the [full stack](#full-stack).

```
 visitors (and mail servers, with an smtp port)
    |  443 (25, ...)
 front node     conf/front/MoquiFrontConf.xml          pairings, names, certificates; no data
    ^  each member dials 443 and keeps its own link open
 member 1   member 2   member 3     conf/front/MoquiLocalConf.xml; 8444 between members only
    +----------+----------+
       shared Postgres
```

The confs, the `.env` names, the DNS and the commands are in [runtime/conf/front](../runtime/conf/front/README.md).

1. Start the front node, with a certificate for its own name. Start the first member of the cluster.
2. Make a pairing code on the front (System › Edge Clients › Clients) and pair the first member with it (System › Edge
   Node; the Pair screen shows the role, front, before the code is spent). Run the wizard: host names and XMPP.
3. Copy that member's `.env` to each other member, with its own `cluster-member@base-url`, and start it.

- **The front pairing.** Pairing with a node whose role is `front` writes `edge/front-node@url`, `edge/front-node@key`,
  `edge/front-node@pin` and `edge/front-node@id`, a pairing of its own beside any edge pairing (`edge_node_*`). It carries web
  access and ports only: no queue, delegated calls, replication, call or SIP rows. System › Edge Node shows it, and
  Unpair (or Unpair front, beside an edge pairing) ends it.
- **The pool, balancing, admission, draining and shared bans** are as with an edge node
  ([A local cluster with an edge node](#a-local-cluster-with-an-edge-node)). A ban a member starts is told up the link,
  and the front refuses that client for the pairing's names.
- **Mail in** through the front: port 25 as a TCP port of kind SMTP for the pairing (System › Edge Clients › Ports),
  each connection passed to a live member's SMTP listener with a PROXY header naming the client, the listener in
  internet mode on every member ([Mail-Inbound](interfaces/Mail-Inbound.md)); this path to a local cluster has no test
  yet (an edge cluster's has). Mail out is the cluster's own server.
- **Not through a front:** call media, TURN and SIP. A local cluster's calls use its own media nodes at addresses of
  their own ([WebRTC, Cluster](interfaces/WebRTC.md#cluster)).
- **Your own load balancer** in place of a front node stays supported ([Cluster](#cluster)). Before an edge cluster it
  must pass TLS through with a PROXY header, since the links need ALPN and client certificates end to end.
- **Two front nodes** are a cluster of front nodes: one external database and one `.env`, each with its own
  `edge@link-url`, and each node behind them links to every member, as with an [edge cluster](#an-edge-cluster). The
  names' address records name each front member.

## Full stack

A front node before an edge cluster, before a local cluster ([plan](../plans/Edge-Node2.md#3-topologies) §3, §4.7): for
a higher-volume shop or other public site on mr-pass. The front faces visitors and mail servers; the edge cluster serves
the mr-pass sites, the mail edge, chat webhooks, calls and SIP, and the queue of its local nodes; the local cluster
holds the apps and the data and is the sites' PASS authority. Any one node below the front can stop and the site stays
up.

```
 visitors, mail servers, chat providers                calls, TURN, SIP trunks and phones
    |  443, 25                                              |  each edge member's own address
 front node      conf/full-stack/MoquiFrontConf.xml          |
    ^  each edge member dials the front: one pairing        |
 edge 1   edge 2   conf/full-stack/MoquiEdgeConf.xml       <-+   shared Postgres
    ^  each local member links to every edge member, at its link name through the front
 local 1  local 2  conf/full-stack/MoquiLocalConf.xml            shared Postgres; the PASS authority
```

The confs, the `.env` names per tier, the DNS and the order to pair are in
[runtime/conf/full-stack](../runtime/conf/full-stack/README.md). Each cluster is as in [Cluster](#cluster); the edge
cluster as in [An edge cluster](#an-edge-cluster).

1. Start the front node. Start the edge cluster and give its own name (`webapp-list/webapp[webroot]@http-host`, at the members' own
   addresses) a certificate (below). Pair one edge member with the front (System › Edge Node, Pair with a
   front node) and copy the `front_node_*` lines to every edge member. On the front, add port 25 of kind SMTP for the
   edge cluster's pairing (Edge Clients › Ports).
2. Start the local cluster, pair one local member with the edge cluster at the edge cluster's own name, run the wizard
   (host names, mail, chat, calls, phones, PASS), and copy its `.env` to the other local members.
3. Create the sites on the authority with the hosts the edge members' `public-site` bindings name.

- **The edge members' role.** `front_edge` (the conf's default): the members forward their local nodes' names, which
  they give the front, and a local node has one pairing. `edge`: the members forward no web access and bind no tunnel
  port, so each local node pairs with the front too (a second pairing, the same Pair flow) for its names, XMPP domain
  and ports, and with the edge cluster for the rest.
- **What the edge members give the front.** Their sites' hosts and chat address's host as terminate hosts, so the
  front sees the session cookie and keeps a browser on one edge member; with `front_edge`, every name of their local
  nodes in its mode. A name another pairing of the front holds is refused and given again after a minute.
- **The local nodes' links through the front.** Each edge member names its `edge@link-url` in its Hello; the front
  keeps the host as that member's link name (Edge Clients › Domains) and passes its connections through untouched to
  that member alone, picked by the server name before the link's ALPN. A local node dials each edge member there, so
  the edge members need no inbound port for links. A link name is never balanced: while its member is not linked it is
  refused. The front is then a single point for the links as well as the web: while it is down the local nodes'
  queue, delegated calls and replication stop too. Set an edge member's `edge@link-url` to a name at its own address
  instead (443 open on it) to keep the links up without the front.
- **The edge members' first certificate.** An edge member with no certificate starts no link server (no EdgeServer,
  no relay down), and an ACME standby listener takes no links, so a link name's first certificate cannot come through
  the front. Give the edge cluster's own name (`webapp-list/webapp[webroot]@http-host`, its System app, with address records at the
  members' own addresses) a certificate first: System › Certificates with ACME at a member, which answers the
  challenge on its own 443, or a copied one in `runtime/cert`. The `AcmeCertificate` rows are shared, so every member
  writes it. Pair the local nodes there, or at a link name once its certificate is set up the same way.
- **Mail.** The MX of each domain names the front; its port 25 passes each connection to a live edge member's SMTP
  listener with a PROXY header, and the member makes every check as for a direct connection. Mail out leaves from each
  edge member's own address, so reverse DNS and SPF name the edge members: list their addresses in SPF (`ip4:`), as
  the wizard's `v=spf1 mx` would authorize the front. STARTTLS ends on the member, so the MX name's certificate goes on
  every member ([Mail-Inbound](interfaces/Mail-Inbound.md)).
- **Not through the front:** call media, TURN and SIP are at each edge member's own address, with their own records
  ([WebRTC](interfaces/WebRTC.md), [TURN](interfaces/TURN.md), [SIP](interfaces/SIP.md)).
- **Web protection** on every tier: the front's first, by the visitor's address; each edge member's and each local
  member's by the client its link names. A ban goes up only the links of the node that started it and is not told
  on: a local member's ban reaches its edge members (and the front only by its own front pairing, with `edge`
  members); an edge member's reaches the front, which refuses that client for the edge cluster's names.
- **PASS.** The sites are on the edge members (`moqui_conf__pass__backend_url=edge:`), each host the root of mr-pass's `Pass.xml`;
  every edge member calls the local cluster as the authority down its own link (mr-pass's `docs/Deployment.md`, Full
  stack).

## Cluster

Several members share one database and run behind a load balancer. Members find each other in the
`moqui.server.ClusterMember` table, with heartbeats on the database clock; there is no Hazelcast or Redis. Over gRPC on
TLS with client certificates they send each other entity cache clears, notifications, co-editing updates, session ends
and call signaling.
Sessions are rows in the shared database, so any member can serve any request.

```
                   clients
                      | https
               load balancer             webapp-list/webapp[webroot]@trusted-proxies on each member
           +----------+----------+
       member 1   member 2   member 3    conf/local-cluster/MoquiLocalConf.xml; 8444 between members only
           +----------+----------+
              shared Postgres            database-conf-name=postgres
```

Every member runs `conf/local-cluster/MoquiLocalConf.xml`; the `.env` names and the commands are in
[runtime/conf/local-cluster](../runtime/conf/local-cluster/README.md), and the conf's comments say what each setting
does. An edge node can take the load balancer's place: [A local cluster with an edge
node](#a-local-cluster-with-an-edge-node).

### Requirements

- **One external database.** Postgres, or MySQL with the default transport. The Enterprise package has no embedded
  Postgres, so a member cannot start on a database of its own by mistake. The content database shares it unless
  the content datasource's `database-conf-name` names another ([Content database](#content-database)).
- **The same secrets on every member**: `webapp-list/webapp[webroot]/session-config@cookie-key`, `cluster-member@tls-cert`, `cluster-member@tls-key` and
  `moqui_conf__entity_facade__crypt_pass`. Startup fails when a live member has a different session key or certificate, with
  "webapp-list/webapp[webroot]/session-config@cookie-key does not match a live cluster member" or "cluster-member@tls-cert does not match a live cluster member".
  For git hosting, `ssh-server@host-key` and `git-server@lfs-token-key` too: behind a load balancer or an edge node's pool a client's
  SSH connections and LFS requests reach any member, and another one after a failover, so a member with its own host
  key is a changed host to git clients and one with its own LFS key refuses the others' tokens. A member whose SSH
  host key differs from a live member's warns at startup; no member row shows the LFS key. Copying the first member's
  whole `.env` gives each the same ([Git-Hosting](interfaces/Git-Hosting.md#behind-a-front-or-edge-node-and-in-a-pool)).
- **A distinct instance id**: each member keeps its own in `runtime/db/instance_id` (or `instance@id`). It is not in
  `.env`, so copying `.env` between members is safe.

### Steps

1. Start the first member once with `conf/local-cluster/MoquiLocalConf.xml` and the shared database settings in
   `.env`, so it generates its secrets into `.env`.
2. Copy that `.env` to each other member before its first start.
3. Set `moqui_conf__cluster_member__base_url=https://<this member's private address>:8444` on each member. Empty is
   `https://127.0.0.1:8444`, which logs an error once other members are live.
4. Open port 8444 between the members only.
5. The conf writes each session change before the response (`<session-config flush="guarantee"/>`), for a load
   balancer that does not keep a client on one member. With sticky sessions it can go.
6. Set `webapp-list/webapp[webroot]@trusted-proxies` and `webapp-list/webapp[webroot]@client-ip-header` for the load balancer, then start the other members.
7. Check the cluster with `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database.

### Transports

`cluster-member@transport` sets how peers reach this member: its cluster port at `cluster-member@base-url` (the default: TLS with the
cluster certificate), Postgres `LISTEN`/`NOTIFY` (`notify`, for members that cannot connect to each other), or its SSH
server (`ssh`). On the cluster port `cluster-member@protocol` picks gRPC on HTTP/2 (the default) or HTTP/1.1 only (`rest`).
Members on different settings, or of different builds, still reach each other: the HTTP/1.1 POST is always there. The
values are in the conf's comments.

### What is shared and what is not

- **Shared**: scheduled jobs take a lock row, so each job runs on one member at a time. Content blobs, `db` git
  repositories, sessions and the LLM quotas are in the database.
- **Not shared**:
  - `local` and `remote` git repositories are on disk under `runtime/git`. In a cluster they are allowed only with
    the conf setting that says `runtime/git` is on storage every member mounts, and refused without it; else use
    `db` repositories ([Git-Hosting](interfaces/Git-Hosting.md#behind-a-front-or-edge-node-and-in-a-pool)).
  - Files on disk are each member's own: the SFTP root (`runtime://sftp`), and any setting that names a directory,
    such as `pass@media-root`. Share the directory between the members.
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
`pass@backend-url` is not set. Provisioning, mail, media, workers and imports are in mr-pass
[docs/Deployment.md](../runtime/components/addon/mr-pass/docs/Deployment.md). This section covers the remote layout.

The remote calls go over REST, which every package has and which is Lite's default, or over gRPC where the package has
it (Maximal, Enterprise): direct to the authority's gRPC listener, or from an
[edge node](#a-local-server-with-an-edge-node) to its local node over the edge link (`moqui_conf__pass__backend_url=edge:`).
`pass@backend-transport` chooses (`rest`, `grpc`, or empty for automatic); the settings are in Deployment.md. With an
edge node as the PASS server, the local node's Edge Node wizard writes the client properties for it.

### The authority

1. Run a BOSS server: `./moq component-get mr-boss`, which brings mr-mantle. It can be one server or a
   [cluster](#cluster). Use Maximal or Enterprise: forum thumbnails need `image`, and sign-in providers need `sso`.
   The site readiness check reports what is missing. The PASS jobs are in Mantle's data (`15-seed-initial/Communications.xml`) and run
   here. Add mr-pass only if this server also serves sites itself.
2. Register each PASS deployment: in the conf its sites and exact SSO callback (`pass-authority/client[<id>]@sites`, `@callback`), in `.env` its secret (`pass-authority/client[<id>]@secret`). The conf turns on gRPC for PASS
   clients (`pass-authority@grpc-enabled`); the HTTPS listener takes the same calls over [HTTP/2](#http2), so no other
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
   ones bound at the authority. Put `pass@client-secret` in `.env`.
4. Embedded Postgres is enough. The PASS server stores framework tables and sessions, and the sessions hold each
   visitor's delegation from the authority.
5. Put the certificate for the public host in `cert/`. Behind a proxy, set `webapp-list/webapp[webroot]@trusted-proxies` and
   `webapp-list/webapp[webroot]@client-ip-header`. Each call to the authority carries the visitor's address, and the authority's sign-in rate
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
  a client on one member. `pass@media-root` and `pass@import-dir` must name storage that every member can read.
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
   moqui_conf__dev_aide__shared=true                 one server or a cluster
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
4. Optionally, turn on MCP (`moqui_conf__llm_facade__mcp__server_enabled=true` in `runtime/.env`, or `llm-facade/mcp@server-enabled` in the conf) for coding agents. `/mcp/setup` gives a client config to paste
   ([MCP](ai/MCP.md)).

### Each dev instance

1. **Connect.** In Dev AIDE (`/qapps/dev`) › Settings › Group servers, give the group server's address
   (`https://team.example.com`) and Connect. The browser opens the group server's sign-in, as the desktop app's does:
   sign in there (with its identity provider, or its own login) and confirm. The dev instance gets a login key of
   your account, kept encrypted and sent only to that server ([Logging in](interfaces/Git-Hosting.md#logging-in); the
   redirect is the dev instance's own loopback address by its IP literal, `https://127.0.0.1:8443/...` even when the
   page is at `localhost`, so you may be asked to sign in to the dev instance at `127.0.0.1` once). A browser signed
   in to the group server already stays signed in there and enters its password again on the confirmation. A server
   without the sign-in takes an access token pasted from its Git access page (Projects › Code › Git access, or Commons
   › Settings › Account) instead.
2. **Confirm what Connect sets**, each a checkbox, from what the server offers
   (`/rest/s1/moqui/hosting/offer`):
   - the repository entry in `myaddons.xml` (not in git) for the server's git URL, and optionally as the
     `default-repository` when the file names none; then `./moq component-get acme-site`. The repository ids (or
     paths) on the group server are the component names. A repository of the same name that you wrote is not replaced:
     give the server's another name;
   - the task source (`moqui-rest`) with your token: Tasks lists your work items, and commits carry the current
     task's trailer;
   - the LLM gateway's lines in `runtime/.env` (`llm-facade/endpoint[default]@url`, the alias as `llm-facade/model[default]@name`, and as
     `llm-facade/endpoint[default]@api-key` a key the server makes for its gateway alone, never your account's key), applied at the next
     start. The lines you had before are kept and put back by Remove. The gateway needs `LlmGatewayUse`; the key,
     made for the gateway alone, is its own client, so its `require-client` asks no `LlmApiClient` row of it
     ([Gateway](ai/Gateway.md));
   - git's credential for the server's host: git's own helper (the default), or the token of the connection.

   For git over SSH, add your public key on the server's Git access screen instead. Settings › Group servers shows each
   connection's state, Check, Sign in again and Remove, which takes back what Connect set and nothing else: a
   `myaddons.xml` repository only as Connect wrote it, the `.env` lines only while they are as it wrote them (yours put
   back), and the keys it was given ended on the server (the gateway's, and the sign-in's; a pasted token stays, as it
   may serve elsewhere).
3. **The Workspace** then knows the hosted repository of each working tree whose remote is the server's (a Merge
   requests tab with Review: the changes in the merge view with the server's threads, a reply, resolve, a comment on a
   line and a review; open threads in the editor's gutter on a checked-out merge request; Propose in place of Push for
   a repository you may only view, Fork where the server makes forks, Publish to a publish branch opening a merge
   request; check chips on Commits and Branches with the runs, a job's log, its failed tests as links and Re-run; the
   contributor agreement's warning and Sign). Checking out a merge request runs its code here (the file watcher
   reloads it): read it first. Merging stays on the server's page. For a pre-commit hook or a coding agent in a
   terminal, `moqui check --agreement` asks the server the same (`MOQUI_GROUP_SERVER`, `MOQUI_GROUP_TOKEN`).
4. **Binding.** Set `moqui_conf__webapp_list__webapp__WEBROOT__http_bind=127.0.0.1`. Dev AIDE gives its users code execution, so a laptop's server should
   not be reachable from the network.

The same by hand, as before Connect: a `myaddons.xml` repository (`<repository name="team"><location type="git"
url="https://team.example.com/git/${name}.git"/></repository>`), the default LLM endpoint's three lines in `runtime/.env` (`moqui_conf__llm_facade__endpoint__DEFAULT__url`,
`moqui_conf__llm_facade__endpoint__DEFAULT__api_key` and `moqui_conf__llm_facade__model__DEFAULT__name`), a
`moqui-rest` task source with the token in Dev AIDE › Tasks, and the token in a git credential helper.

### The shared dev-aide server

1. Run a dev instance (`./moq run`, or `MOQUI_CONF=conf/MoquiDevConf.xml`) with the components the users extend, and
   set `moqui_conf__dev_aide__shared=true`.
2. Give it a certificate, leave `webapp-list/webapp[webroot]@http-bind` empty, and do not publish 8080. Set `webapp-list/webapp[webroot]@allow-origins` to the
   server's own origin, such as `https://dev.example.com`, in place of the dev conf's `*`. An empty value would not
   replace it.
3. Put its users in `DEV_AIDE_EXTENDERS`, who may write site components and publish them, or `DEV_AIDE_USERS`, who may
   do everything.
4. Network git (fetch, pull, push, clone) is off on a shared server until an admin turns it on in Dev AIDE › Settings,
   with a deploy token or per-user tokens from the group server. Each user connects to the group server as
   themselves (Settings › Group servers); a shared server's address is not loopback, so the group server lists its
   `/qapps/dev/Settings/serverSignedIn` URL in `webapp-list/webapp[webroot]@app-redirect-uris`, or its users paste a token. Connect's
   `myaddons.xml` and `.env` items are not offered on a shared server.
5. Commits carry the logged-in user as author. A save checks the file on disk by its hash, so a save over another
   user's change offers Compare instead of overwriting it.

### What is not shared

A shared dev database is not supported. Every serving process joins the cluster of its database, so dev instances
with their own generated secrets refuse to start on one. If they did start, one developer's entity changes would alter
everyone's schema, and any instance could run the team's jobs. Each dev instance keeps its own embedded Postgres. To
hand a database to someone else, send a `./moq save-db` save and the matching `moqui_conf__entity_facade__crypt_pass`.

**Not built**:
- Moqui is not an identity provider, so a dev instance's own users do not log in through the group server (Connect
  only gets the dev instance a key of the user's account there). Point both at one external OIDC provider, or keep
  separate accounts.
- Pushing a hosted repository to another server; a `remote` repository is a read-only mirror.
- OAuth or OIDC tokens for MCP and A2A; they take login keys and app passwords.

## Managed instances

A server can provision and run other instances, MR Moqui, Postgres, MySQL and OpenSearch, on Docker and Podman hosts,
with every credential in an operational vault ([Vaults](security/Vaults.md#operational-vaults)). This is System ›
Instances, the services of `org.moqui.dev.InstanceServices` (Rust, `crates/moqui-instance`, in the Enterprise and
Maximal builds) and the container driver CI's runs use too (`moqui_lib::docker`). Every service needs the sealed
`INSTANCE_ADMIN` (`ADMIN_ADV`). The plan is [Vault-Instances](../plans/Vault-Instances.md) §4; built so far are M1,
the manager link (M2, [below](#the-manager-link)), Monitor for managed instances (M4: the fleet screen, each
instance's identity and ingest key, scrape targets, alert rules and the `instance` recovery adapter,
[Monitoring](tools/Monitoring.md#managed-instances)) and deploys (M3: an instance's or an environment's Deploy and Roll
back, and a pipeline's deployment to an environment,
[Component-Deploy](tools/Component-Deploy.md#a-deploy-over-the-link)) and rotation (M5, [below](#rotation)): a managed
sealed start comes later.

**The rows.** System › Instances has a tab for each:

| Tab | What it holds |
| --- | --- |
| Environments | Production, staging or demo: its operational vault (where its items are and where a provision deposits what it makes), its approver group, required pipeline, machine access, unseal policy, transport and lease time limit |
| Hosts | A Docker or Podman host: `unix` (a socket's path), `https` with `tlsCaCert` and `tlsClientCert`, or `http` to this machine only. The client key is the `tls-key-pair` item `InstanceHost:<id>:tlsClientKey` (field `privateKey`) of the host's vault, the environment's when the host names none. A runner host for CI names the environment whose machine access opens its key (`environmentId`) |
| Images | An image of a kind. MR Moqui's own is `podman/build.sh`'s `localhost/moqui:latest`, with one data volume, `/var/lib/moqui`. A registry login is the `registry-login` item `InstanceImage:<id>:registry`, its `registry` field the image's `registryLocation`, which is the registry `imageName` pulls from |
| Database hosts | A database server not managed here, of kind `postgres` or `mysql`; its admin login is the `database-credential` item `DatabaseHost:<id>:admin`, whose `host` and `port` fields are the row's |
| Kinds | `mr-moqui`, `postgres`, `mysql`, `opensearch` (seed data, `framework/data/InstanceTypeData.xml`): each one's settings, volumes, health check, the vault items it needs and what it depends on |
| Fleet | Each environment's instances as Monitor sees them: kind, version, health, seal and link, cluster, last deploy, open incidents, items past their rotation age ([Monitoring](tools/Monitoring.md#managed-instances)) |
| Instances | Each instance's kind, environment, host, desired state and what its host last said |
| Rotation | An environment's operational items with their age, rotation age and whether each is due; Rotate, Rotate due and Rotate all ([Rotation](#rotation)) |

No row holds a secret. An instance's page shows its secret bindings (the item's binding and type, the file it is given
as, and when it was deposited), never a value.

**Making an instance.** *New instance* (`create#AppInstance`) takes an image, an environment, a host, a container name,
and the dependencies the kind takes: a database (another instance of the environment, or a database host) and search
(an OpenSearch instance). It writes the instance's rows from its kind: settings, volumes and health check, with the
names filled in (`${instanceName}`, `${databaseHost}` and the others of `kinds::PLACEHOLDERS`; nothing else is
expanded), and its secret bindings (`AppInstance:<id>:admin`, `:dbPassword`, `:ingestKey`). It makes nothing on a
host. A database instance is on the same host as the instances that use it: they reach it on its network.

**Provisioning** (`provision#AppInstance`, the instance page's Provision):

1. The page previews what will be made: containers, volumes, networks, the databases and users, the items deposited,
   host names, and the lease it needs, with where each login goes: a host's client key (`to docker host
   host:2376`), a registry login (`to registry.example.com`), a database host's admin login (`to db.example.com:5432`),
   or the admin login of a database instance made before.
2. A provision that opens items runs only under a lease of them that an approver opened, or under its environment's
   machine access; without either it is refused, and the refusal is an action row. Provision asks for the lease and the
   page goes to the lease dialog (System › Vaults › Lease); once an approver opens it, the provision runs on the server
   that asked. With machine access (the environment's *Machine access* on, and the items given in System › Vaults), it
   runs at once under a lease approved by "machine", for the environment the instance's own row names. A provision
   that opens no item (a local socket, public images, its database made with it) needs no lease. One action runs under
   a lease at a time. An environment that names an approver group takes its actions only from that group's members.
3. **A login goes only where its item says.** The operation the lease names, which the server signs and the approver
   reads, lists each item with its destination. At its use the item's own destination is checked against the row: a
   database host's admin login (`database-credential`) must name the row's host and port, a registry login
   (`registry-login`) the image's `registryLocation`, and the image's own name must pull from that registry (the login
   goes to the registry `imageName` names). A row changed after the approval, or an item of another server, is refused
   (`destinationMismatch`, `registryMismatch`) before the login is sent.
4. Dependencies are made first. For each instance, the credentials its kind makes (a database's admin password, an
   MR Moqui instance's database password and ingest key) are made by the server and deposited into the environment's
   vault, wrapped to its deposit key: the server writes them and cannot read them back, and a member takes them in
   when they next open the vault. The database password's item records the address the instance's container uses.
5. **Its database and user.** On a database instance they are made from inside the instances' network: a one-shot
   client container of the server's own image, on its host and network, reads the admin login and the statements
   from secret files and runs them; no login crosses another network, and the server publishes no port for the
   manager. On a database host the manager connects over TLS that checks the server's certificate and name
   (`verify-full`), and the drivers never send a password in clear text over a connection without TLS. A Postgres user
   is made with a SCRAM-SHA-256 verifier, so the password itself is in no statement or server log (MySQL takes
   `CREATE USER ... IDENTIFIED BY`, its password letters and digits). A user or database that is there already is
   refused and named (`databaseExists`): reusing one would leave the vault's item wrong or give a new user a database
   that holds data. Names are plain identifiers.
6. Then its network (one per environment, `moqui-<environment>`), volumes and container, started and checked. A
   published port (`hostPort`) is bound to the host's loopback only; a proxy on the host reaches it.
7. **Secrets are files.** No secret goes into a container's environment, labels or command, where `docker inspect` and
   `/proc/*/environ` show them: the driver refuses such a container before anything is sent. A secret is the file
   `/run/secrets/<name>`, 0400 and owned by the kind's user, put into the created container before it starts on Docker
   and a Podman secret on Podman, and the image reads it through its `*_FILE` setting (`POSTGRES_PASSWORD_FILE`,
   `MYSQL_ROOT_PASSWORD_FILE`). OpenSearch reads no such setting: its entrypoint is wrapped to read its admin password
   from the file, so the OpenSearch process has it in its own environment, as the image requires, and `docker inspect`
   does not. A new MR Moqui instance gets two files: its one-time pairing token, `moqui-pairing-token`, made after
   its image is pulled (it works for 15 minutes; the instance's row keeps its SHA-256), and its Monitor ingest key,
   `moqui-ingest-key`, which its telemetry export reads (`telemetry@otlp-headers-file`). Its database password reaches
   it in the bootstrap bundle over the manager link (M2), not as a file.
8. **All or nothing.** A volume that is there already (an earlier instance's data, with its own credentials) is refused
   before anything is made (`volumeKept`). When a step fails, what the provision made is undone, last first: its
   containers and volumes removed, a database and user it made on a server made before dropped, its deposits withdrawn
   (still wrapped to the deposit key, so no member had taken them in). Then the provision can run again. The action
   row records what was made and undone, never a value, with its lease or machine access; the lease ends with the
   provision.

**Start, stop, restart and remove** run the host type's services (`InstanceHostType`'s start, stop and remove
services, only of `org.moqui.dev.InstanceServices`), under a lease when the host's client key is a vault item. Remove
removes the container and, only when asked, its volumes, which hold its data; its row and its vault items stay.

**The reconcile job** (`reconcile_AppInstances`, every five minutes) writes what each host says of each instance (its
state, health and image digest) beside its desired state, and changes nothing on a host. An instance on a host reached
with a client key is observed during an action, under its lease, until the manager link reports its state.

**CI runner hosts.** The run services of a CI job reach a host through the same rows, and a CI run is unattended: a
runner host reached over https with a client certificate names its environment (`InstanceHost.environmentId`), and each
CI call opens the client key under that environment's machine access, a lease approved by "machine" that ends when the
call has its client ([Vaults, Machine access](security/Vaults.md#machine-access)). A host with no environment, or an
environment not given the key, is refused (`machineAccessMissing`); a runner host on a `unix` socket needs neither.

**Tests.** `cargo test -p moqui-instance` runs the rules against the fake engine on Docker and on Podman. Against a real
engine, opt in: `MOQUI_CI_DOCKER_SOCKET=$HOME/.docker/run/docker.sock cargo test -p moqui-instance docker_real --
--ignored` provisions a Postgres instance whose health check signs in with its password file and a second instance
whose database and user the one-shot makes on it, rotates that user's password and the first's admin password
([Rotation](#rotation)), then removes both.

### The manager link

A managed MR Moqui instance dials its manager and keeps one connection open: the manager's HTTPS port, ALPN
`moqui-manage/1`, gRPC on HTTP/2 (`crates/moqui-host/src/manage_link/`, `proto/manage.proto`; the `manage` feature, in
every package; the manager's end needs `instances` too). The instance opens no port for it, and it dials while sealed.

- **Keys.** Each end has a link key, `runtime/db/manage_link_key` (32 bytes, mode 0600), from which its TLS key and the
  X25519 key of its bootstrap bundle are made; the other end pins it by SHA-256 of its public key. A key not pinned is
  refused, on either end. The key must work while the instance is sealed, so it is not under the root key: without a
  TPM it is a usable key on disk, which the plan names as the limit of the link key (§4.4). On a host with a TPM2,
  `moqui_conf__manage__key_store=systemd-creds` keeps it as a `systemd-creds` credential bound to the host (the seal's
  `tpm` helper and `seal@systemd-creds-command`); it opens on no other host.
- **The manager's address.** `manage@link-url` on the manager is what its instances dial: its HTTPS listener. An edge
  node's own HTTPS listener takes no manager links. A provision sets `moqui_conf__manage__url` and
  `moqui_conf__manage__pin` (the manager's fingerprint) in a new MR Moqui instance's settings; neither is a secret.
- **A manager behind a firewall** (§4.4: reached through its own edge node by a passthrough host name) is not built
  yet. The edge node already passes a passthrough host's connection on before any TLS, whatever ALPN it offers, but the
  local node's end of the tunnel ends the TLS with its web settings (`acme::accept` in `edge_link/down.rs`), not the
  HTTPS listener's hand-off, so `moqui-manage/1` is not taken there. Until it is, such a manager's instances reach its
  HTTPS port directly (a VPN, or a port opened to the instances' addresses only).
- **Pairing.** A provision gives the new container its one-time token as the file `/run/secrets/moqui-pairing-token`
  (`manage@token-file`), never in its environment, labels or command. It works once, within 15 minutes, for that
  instance only: the manager finds the row by the token's SHA-256, checks it in constant time, clears it, and pins the
  instance's link key and bundle key on the row (`linkKeyFingerprint`, `linkBundleKey`). An instance the manager did
  not make (an existing server being adopted) pairs with a code made on its page in System › Instances (*Make a pairing
  code*, `create#InstancePairingCode`), shown once with the manager's address and link key fingerprint; on the instance
  `org.moqui.dev.InstanceLinkServices.pair#Manager` (`url`, `pin`, `code`, INSTANCE_ADMIN) pairs and dials. What the
  instance pinned is `runtime/db/manage_link.json` (mode 0600, no secret).
- **Approvers.** At pairing the instance takes and pins the signing keys (the Ed25519 halves of the personal keys) of
  its environment's approver group, version 1; it uses them to check its recovery holders' keys (below). A changed set
  reaches it only signed by an approver it pinned, over the instance's id and its link key fingerprint (so a signed set
  does not replay to another manager's instance of the same id), the new version and every key
  (`update#InstanceApprovers`, `moqui_vault::approvers`); a set the manager signed itself is refused. Any one pinned
  approver can replace the whole set. An environment with no approver group, or none of whose members has a personal
  key, pins an empty set, which no one can change: the pairing logs a warning, and the instance pairs again once the
  environment has approvers. A manager that is compromised at pairing can give a set of its own choice; one compromised
  later cannot change the pinned set (§4.9).
- **Sealed.** While the instance is sealed it makes only the sealed calls, and the manager answers no other: Hello and
  its status, the bootstrap bundle on a first start, and unseal, by transit or by recovery shares sealed to it
  ([Managed sealed start](#managed-sealed-start)). Once unsealed it sends an event (started, unsealed) and opens the
  stream the manager's commands come down; a seal closes that stream, and the manager then answers a status from what
  the instance last said.
- **"Unsealed" as the manager takes it.** The instance's own claim is not enough. Every command but status and seal
  runs only on a link whose "unsealed" the manager took: it unsealed the instance on that link by transit, or the
  instance proved on it that it holds its root key, by a signature over the link's generation and a challenge of the
  manager's with an Ed25519 key derived from the root key (`moqui_core::seal::unsealed_proof_seed`), whose public half
  the manager pinned (`AppInstance.unsealProofKey`). A claim with neither is taken as sealed, and the command is refused
  ("taken as sealed"). The manager pins that key from the root key it unwraps at a transit unseal (`unsealProofTrust`
  transit); for an environment whose method it does not unseal (tpm, passphrase, shamir) it takes the key the instance
  gives on its first unsealed link (first-use: nothing the manager holds checks that first one, so whoever held a new
  instance's link key before its first unseal could pin a key of its own, and the instance's own proofs would then fail,
  which shows; `forget#InstanceUnsealProofKey` lets the next proof pin again after a rotate-root under such a method).
  `get#InstanceLink` shows it (`unsealVerified`, `unsealProofTrust`).
- **A link from a new address.** A link of an instance from another address than its live link's, or than its last
  link's within `manage@address-change-wait-seconds` (300) of that link going down, is refused and told to the
  administrators (topic `InstanceLinkAddress`), until an operator confirms the move on the instance's page (*Allow a new
  address*, `confirm#InstanceLinkAddress`) or the wait passes; pairing again confirms it too. A holder of a stolen link
  key gets no command from a claim of unsealed; under the automatic unseal policy it still gets the root key by
  transit from the instance's own address, or after the wait, until the instance is revoked (the limit of the link key,
  §4.4), and that unseal from a new address raises Monitor's alert.
- **The bootstrap bundle.** A provision holds a new instance's bootstrap secrets (its database password, as
  `entity-facade/datasource[transactional]@password`) in the manager's memory, never in a row, until the instance takes
  them over its link, HPKE-sealed to the bundle key it pinned, with its id as associated data; a manager restart or the
  token's 15 minutes end them, and the instance then needs its secrets put by `call#InstanceCommand` (put-secret) or
  `moqui operator secret put`. The instance keeps them in `seal.json` with the seal's bootstrap API (Sealed-Secrets S4),
  never in `.env`: its gate starts the dialer before it waits, keeps the bundle in memory while sealed, and once
  unsealed puts it in the seal before the boot reads the datasource password (waiting up to
  `manage@bootstrap-wait-seconds`, 120, on a first start).
- **Commands** (`call#InstanceCommand`): status (version, seal state, schema hash, live cluster members, health), seal,
  restart, drain and undrain, put-secret (`moqui operator secret put`'s rules), deploy-component (Component-Deploy's
  deploy: with no signature the manager's own, the pinned link standing for the permission, so the instance needs no
  attestation secret; M3's deploys and rollbacks send it), export-seal (the recovery
  bundle, wrapped values only) and surface-report (Sealed-Secrets S12: "not built"). On the manager each is checked
  against the instance's environment and the caller: INSTANCE_ADMIN, and a member of the environment's approver group
  when it names one. On the instance they run as the link's one identity, which runs only these commands: a command it
  does not know is refused, and nothing on the link runs an arbitrary service.
- **SSH, the other transport.** An environment's transport is link, SSH or both (`InstanceEnvironment.transportEnumId`;
  both is the link, and SSH while it is down). Over SSH the manager runs `moqui operator status`, `seal` and
  `secret put` (the value on standard input) by the instance's SSH admin shell or its host's sshd (`sshAddress`,
  `sshUser` on the instance's row), with its host key pinned (`sshHostKey`, one known_hosts key; an unknown one is
  refused). ssh reads no conf file (`-F /dev/null`) and no known host but the pinned one, takes the user by `-l` and
  the host after `--`; a user, host or host key that is not a plain name, host name or address, or one key line, is
  refused before it runs. The key is the
  `ssh-key` item `AppInstance:<id>:sshKey`, used under a lease (`leaseId`) or the environment's machine access and
  written for the one call to a file of mode 0600 that is removed after. The other commands are the link's.
- **Revoke** (`revoke#InstanceLink`, the page's Revoke) unpins the instance's link key, clears a pairing token or code not
  yet used, and closes its link; it pairs again only with a new code. A link whose key was revoked or replaced between
  its connection and its Hello is refused at the Hello, and pairing again closes the old key's link.
- **What the bundle keeps.** Only bootstrap secrets, and never a crypt pass other than the one the instance keeps
  already; each refused one is logged as an error and named, and the first start goes on without it. On an instance
  with no seal (its root key in memory only) the bundle cannot be kept, said as an error.

The instance's page in System › Instances shows the link: linked or not, its generation, the address it came from, its
last status, its pinned key, when it paired and took its bundle, and this manager's own key fingerprint. Tests:
`cargo test -p moqui-host --lib manage_link`, `cargo test -p moqui-host --test it -- manage_link` (a real manager and the
instance's dialer in one process) and `cargo test -p moqui-instance link`.

### Managed sealed start

A managed instance's seal (plans/Vault-Instances.md §4.5, M6; [Sealed start](#sealed-start) is the seal itself).

- **The method.** A provision sets `moqui_conf__seal__method` on a new MR Moqui container from its environment's
  `unsealMethod`: `transit` (the manager's, the default for a container) or `tpm`, each automatic, so the first start
  makes its seal by itself; `passphrase` and `shamir` leave it waiting sealed for `moqui operator init` on its socket.
  A passphrase for it is a vault item an operator uses under a lease; shares are never typed into the manager.
- **Transit by the manager.** The manager keeps one X25519 transit key per instance whose environment's method is
  transit, in its own keyring (`SecretValue` `instance-transit/<id>`, a platform secret: an unattended unseal needs
  it), and gives the public half in the link's Hello. The key is made once, when no row holds it, and only read after:
  a row that does not open, or a manager with no seal of its own, is an error, never a new key. At init the instance
  wraps its root key to it (HPKE, its id as associated data: `seal.json` records `backend: moqui-manager` and the wrap),
  so the manager sees no root key then. At each start the instance sends the wrap with a key it made for that one
  unseal; while its link key is pinned (a revoke unpins it) the manager unwraps it with that instance's transit key and
  seals the root key to that key. The manager sees the root key during the unwrap, as every transit seal does. A seal.json
  whose transit is OpenBao's, or a `seal@transit-url`, stays the core's transit; an environment whose method is not
  transit gets no transit key, and its Unseal is refused.
- **Only the instance's own root key.** HPKE base mode names no sender, so whoever holds the link key can wrap a key of
  their own to the public transit key. The manager therefore takes only the root key whose unsealed-proof key is the one
  it pinned: the first is pinned at the instance's first start (its first unseal within the pairing window, which the
  instance makes right after init, on that link), or, later, with a member's approval; after it, an unseal whose root
  key gives another proof key is refused, recorded (`IasRefused`) and told to the administrators (topic
  `InstanceUnsealRefused`), and the pinned key stays. A rotate-root's new key reaches the manager signed by the old one
  (`RotateProof`, with the new transit wrap, which must open to it); when the instance restarted between and lost the
  old key, an operator forgets the pinned one (`forget#InstanceUnsealProofKey`) and the next unseal pins the new one with
  a member's approval. Re-pairing clears the pinned key, the link's signing key and the share set.
- **The unseal policy** (`InstanceEnvironment.unsealPolicyEnumId`): `IupAutomatic` answers while the link key is pinned;
  `IupApproval` holds each unseal for a member's approval: an unseal `InstanceAction` waits, the approver group hears of
  it (topic `InstanceUnsealApproval`), `approve#InstanceUnseal` (INSTANCE_ADMIN and the approver group) lets the
  instance's next unseal through, once, on the link generation and from the address that asked, within 15 minutes. Every unseal is an `InstanceAction` and a Monitor unseal
  (`moqui.instance.unseals`), told apart when it comes from an address the instance never unsealed from
  (`moqui.instance.new_address`, the shipped alert rule). The policy is a rule the manager keeps: a compromised manager
  holds the transit keys whatever it says (§4.9).
- **While sealed.** When the transit unseal fails (the manager out of reach, or holding the unseal for an approval) the
  instance stays sealed and ready (`STATUS=sealed: auto-unseal failed`), tries again on each status of its link, and
  takes recovery shares: it makes a key for this unseal, with a time it stops being taken (an hour, then a new key),
  signs both with its link key, and offers it over the link (`SealedShares`); a holder runs `moqui vault --server
  <manager> --user <them> seal-share <appInstanceId> --vault <their personal vault>`, which takes the share they filed
  and checks the key's signature against the link key fingerprint filed with it (never one the manager hands over then:
  a key the manager makes in the instance's name is refused) and its time, and seals the share to it. The manager keeps
  the ciphertext in rows (`InstanceSealedShare`, so any member of a manager cluster answers) until the instance takes
  it, unseals or its link ends; only that unseal opens it. A share of another split is refused on entry and leaves the
  shares entered before it. A holder can also enter a share at the instance's own
  operator socket (`moqui operator unseal --recovery`), the way when the manager is gone; `rekey` then moves the
  instance to another method.
- **Recovery shares to holders.** The environment names its holders (`recoveryUserGroupId`). After an automatic first
  start the instance takes each holder's public keys over the link with the key checks of them by the environment's
  approvers, and takes a key only with a check signed by an approver it pinned at pairing; a key without one is refused,
  so the manager cannot put a key of its own in a holder's place. It seals each share to one holder (one each, and one
  key each, so no place holds two) and sends it as a delivery signed with its link key; only on a link the manager takes
  as unsealed. The holder runs `moqui vault --server <manager> --user <them> shares --vault <their personal vault>`: the
  delivery is checked against the instance's link key as the manager pinned it, the share filed as a `recovery-share`
  item ("Recovery share 2 of 5, <instance>") with that link key's fingerprint, the holder's own anchor of the instance
  from then on, and a confirmation signed with their key goes back, which the instance checks itself. The held shares
  stay on the operator socket (`init --take-shares`) until every share is confirmed; init is complete then. The instance
  logs the unconfirmed ones, and `get#InstanceLink` lists each share with its holder and whether it is confirmed. When
  fewer holders have a checked key than there are shares, nothing is sent and the shares wait on the socket, printed. A
  set the manager refuses for good is dropped and a new one made for the current holders. When a holder is replaced (no
  longer among the checked holders), the instance makes a new split, sends it, and keeps it in its seal only once the
  manager took its deliveries: then the old shares stop opening its seal. The manager chooses the holders among the
  users an approver checked; that the holders are the right people rests on the approvers' checks (§4.9).
- **First-use proof keys.** For an environment whose method is tpm, passphrase or shamir the manager cannot derive the
  instance's proof key: the key the instance gives on its first unsealed link is held as pending, the administrators
  are told (topic `InstanceUnsealRefused`), and its claims are not taken until an operator confirms it
  (`confirm#InstanceUnsealProofKey`). Until a link is taken as unsealed, its status reports `sealState` `unverified`,
  which a deploy (M3) and a rotation (M5) read as not up, and Monitor keeps the instance sealed.
- **The recovery bundle.** Each time a link the manager takes as unsealed serves commands, the manager runs export-seal
  over it and, when it is a seal bundle (at most 1 MiB) and not the one deposited last (`sealBundleHash`), deposits it
  (wrapped values only) as a `note` item of the environment's operational vault (`AppInstance:<id>:sealBundle:<time>`,
  `sealBundleDate` on the row): the first time, and after a rekey or rotate-root; `store#InstanceSealBundle` does the
  same on demand. The manager does not read it back without a lease, though it could unwrap the root key
  in it with the transit key (§4.9).
- **The limits** (plans/Vault-Instances.md §4.4, §4.9). The link key works while sealed, so without a TPM it is a usable
  key on disk; under the automatic policy a stolen disk with the link key and `seal.json` gets the root key from the
  manager until someone revokes the instance (`revoke#InstanceLink`, after which transit refuses it): production uses
  the approval policy or a TPM-bound link key (`manage@key-store`). A compromised manager sees each instance's root key
  at its next transit unseal, can unwrap the root key in any recovery bundle it reads, and with a backup reads every
  row, whatever the policy. A manager compromised when an instance pairs can give it approvers of its own choice and
  then get its shares; the holders' confirmations show a holder who received nothing. The manager seals with tpm, kms,
  passphrase or shamir, never by transit to an instance it manages.

Tests: `cargo test -p moqui-vault managed`, `cargo test -p moqui-instance transit`, `cargo test -p moqui-host --lib
manage_link`, `cargo test -p moqui-host --test it manage_link`; the acceptance test `ManagedSealedStart` is
`cargo test -p moqui-host --test it managed_sealed_start -- --ignored` (a restart unseals by transit; with the manager
out of reach the instance stays sealed and the link-down alert's gauge fires; shares, at the socket and relayed, unseal
it).

### Rotation

The manager changes a managed credential at its source, then on each instance that uses it, then in the vault
([Vault-Instances](../plans/Vault-Instances.md) §4.7; `crates/moqui-instance/src/rotate/`). Rotating opens the item and
what changes it (a database's admin login, a host's client key, an instance's SSH key), so it runs under a lease an
approver opens or the environment's machine access, as a provision does.

**What the manager rotates.** A kind's secret whose `InstanceKindSecret.rotateService` is
`org.moqui.dev.InstanceServices.rotate#Credential`:

| Credential | Its source | Who uses it |
| --- | --- | --- |
| An MR Moqui instance's database password (`AppInstance:<id>:dbPassword`) | Its database: `ALTER ROLE ... PASSWORD` with a SCRAM-SHA-256 verifier (the password is in no statement or server log), or MySQL `ALTER USER ... IDENTIFIED BY`, run as the database's leased admin login from a one-shot client on the database's network, or over verified TLS to a database host | The instance, as `entity-facade/datasource[transactional]@password` |
| A Postgres or MySQL instance's admin password (`AppInstance:<id>:admin`) | The database itself, signed in as the admin with the value it replaces (MySQL: `root@%` and `root@localhost`) | The manager only. The container's `/run/secrets` file keeps the value it was made with, which the image reads only to make an empty data directory |
| An OpenSearch instance's admin password (`AppInstance:<id>:admin`) | Its security API, `PUT _plugins/_security/api/account` with the current and the new password, from a one-shot of its image on its network (the login in a netrc file, never the command) | Each MR Moqui instance whose search it is, as `search-cluster-password` |

**The order**, for each credential (an `InstanceAction`, `IatRotate`, on the instance whose secret it is):

1. Before anything changes: no other rotation of the item runs or waits for its lease (one at a time, on any cluster
   member: its `InstanceRotation` row's claim, `rotationRunning`); the vault's deposit key is checked; each instance
   that uses the credential must be up and unsealed on its link (as the manager takes it, [The manager
   link](#the-manager-link): a bare claim reads `unverified` and is not up; a new value goes only to an instance the manager
   would deploy to); and the value it holds now could go back where the new one goes (a MySQL password with a quote or
   a backslash, or an OpenSearch one a netrc file cannot hold, is changed by hand). Otherwise nothing is changed.
2. A new value is made and **staged before the source changes**: the item's body opened under the lease with only the
   rotated field changed (its other fields, notes, tags and URIs stay), deposited as a `VaultItemPending` row, a new
   item key wrapped to the vault's deposit key, so the manager writes it with no lease and cannot read it back, and the
   new value is never only in the manager's memory.
3. The value is set at the source, as the table says. The value it replaces, read from the lease, signs the change in
   where the source needs it.
4. The value is put on each instance that uses it (`moqui operator secret put` over its link from the manager's own
   Rust path, never a service's parameter; over SSH on standard input) and the instance is restarted (over its link;
   over SSH, its container through its host).
5. The check: the source takes the new value (a sign-in as the user or admin with it), and each instance comes back up
   and unsealed within `manage@deploy-wait-seconds` (600).
6. The staged value becomes the item's next revision (`VeItemRotated`), only from the revision the rotation read
   (another write meanwhile refuses it, `itemChanged`). The revision before stays in `VaultItemHistory`, with its
   wrapped key and body as they were: the way back, which a member opens in their client. A member takes the new
   revision in before a lease names the item again (System › Vaults, the vault's *Take in*, below), and an environment's
   machine access to the item is marked stale and given again ([Vaults](security/Vaults.md#machine-access)). Staging
   and the new revision each move the vault's revision, so a member removal computed before the rotation is refused.

When a step after the source fails (an instance refuses the value or does not come back, the source does not take it,
the item was written meanwhile), the old value is put back at the source and on each instance that took the new one,
which is restarted, the item is not changed, and the staged value is dropped. **When the old value cannot be put back
everywhere** (the source refuses the change back, an instance the old value), the staged value is kept
(`keptDate`, a `VeItemUpdated` event): the source may hold it, and a member with the reveal role opens it
(`list#VaultItemHistory`'s `pendingList`, then `reveal#VaultItem` with its `pendingId`) and sets it in the vault. This
matters most for a database's admin password, which no one else holds: the value is not lost. A manager that stops
mid-rotation leaves its staged value pending with no `keptDate`, and its claim until the lease's time (or four hours)
passes. Each step and each put-back is in the action's report, never a value. One credential that fails is reported
and the rest of the rotation goes on. A database's users are rotated before its admin login in one rotation, since
their change signs in with the admin login the lease holds.

**When.**

- **At the policy age.** An operational item is due past its rotation age: its own (`VaultItem.rotationDays`, set by a
  manager of the vault with `set#VaultItemRotation`), else its type's (`VaultItemType.rotationDays`: 90 days for a
  database credential and a login, 180 for a registry login, 365 for an API key, an SSH key and a TLS key pair), from
  when its value last changed at its source (`rotatedDate`: made, deposited, rotated, or written with `rotated`). An
  item written before rotation ages has none: each start gives such items the time of the start, so after an upgrade
  their age counts from it, and the job does not find every older item due at once. The job
  `rotate_DueInstanceCredentials` (daily, `rotate#DueCredentials`) rotates each environment's due items that the
  manager rotates and a member has taken in: at once under its machine access, else it asks one lease of them, and not
  again while a rotation waits for its lease. An item whose last rotation failed within `manage@rotate-retry-days` (7)
  is held back and told to the administrators (topic `InstanceRotationFailed`) instead of being tried each night. Run
  by a person (authorization on), it rotates only the environments whose approver group they are in. The fleet
  screen's *Items past rotation* column and the gauge `moqui.instance.items_past_rotation` (the shipped alert rule
  `instance-item-past-rotation`) count the due items.
- **By hand from a page.** An instance's page has Rotate on each secret binding, and System › Instances › Rotation
  lists an environment's items with their age, rotation age and whether each is due, with Rotate, Rotate due and
  Rotate all (`request#InstanceRotation`; under machine access it runs at once, else it goes to the lease dialog). The
  list shows each item's name and binding (never a value) to INSTANCE_ADMIN in the environment's approver group,
  whether or not they are a member of its vault: metadata the instance pages show too.
- **On removing a member** from an operational vault: the removal's answer (`remove#VaultMember`, `rotationOffer`)
  lists the vault's items, which the manager can rotate and what to do by hand for the rest, since a removed member may
  have kept values ([Vaults](security/Vaults.md)). Rotate them with `request#InstanceRotation` (`bindingList`) or on
  the Rotation tab.
- **After an incident** (Sealed-Secrets §3.12): Rotate all on the Rotation tab (`all`), every item of the environment
  that has a rotate service, and the list says what to do by hand for the others.

**Taking new values in.** A rotation's new revision, like a provision's credentials, is wrapped to the deposit key.
A vault's page in System › Vaults offers *Take in* when it has such items: in the browser's vault worker (as the lease
dialog loads it, with integrity hashes) the member's key passphrase opens their personal key and the vault key, each
item's key is rewrapped from the deposit key to the vault key (`deposit_adopt`), and only that wrapped key goes back
(`take#VaultDeposit`, a member who may write; the body stays as it is).

**What the manager cannot change** shows as due with what to do by hand: the kind's `rotateByHand` (an ingest key: a
new key in Monitor › Sources, then a deploy of the instance's image so its new container has the file), or by what the
binding is (a host's client key, a registry token, a database host's admin login, an SSH key). Change it at its source,
then write the new value to the item in your vault client with `rotated` (`update#VaultItem`, its event says so),
which starts its age again; taking a deposit in or editing notes does not.

**Limits.** The OpenSearch image's demo configuration serves a self-signed certificate, so the one-shot that changes
its admin password does not verify it. The call stays on the environment's own network, but a container on that
network with `NET_RAW` (Docker's default capability set has it) can spoof ARP and read the old and new admin passwords
in transit; give the environment's network only the instances that belong on it, or drop `NET_RAW` from them. The
OpenSearch path is checked against the fake engine only, not a real OpenSearch, and the security plugin may refuse a
reserved admin's change through the account API: the action then says so (403), and the admin is changed with the
plugin's own tools by hand. Over SSH an instance is restarted through its container's host; one with no container this
manager made is restarted by hand.

Tests: `cargo test -p moqui-instance rotate` (the fake engine and a fake of the instances' link), and the opt-in real
engine test of [Managed instances](#managed-instances).

## Components

Components live in `runtime/components/addon`, and dev-only ones in `runtime/components/dev`. Each is its own git repository.
`addons.xml` lists the known components and repositories. A `myaddons.xml` beside it, which is not in git, adds or
overrides `repository`, `component` and `component-set` elements. It can set `default="a,b"` (what `component-get` gets
with no names) and `default-repository` on its root.

| Task | What it does |
|---|---|
| `./moq component-get <name...>` | Get components and the components they depend on (`depends-on` in `component.xml`); a `component-set` name gets the components of the set (`apps`: mr-marble, mr-boss and mr-pass) |
| `./moq component-depends` | Get missing dependencies of what is in `runtime/components/addon` |
| `./moq component-create <name>` | Make a component from the component template, `runtime/components/dev/template`: an entity, services, a REST API, screens, data and specs; `--rest` or `--screens` for one part |
| `./moq component-zip [name...]` | Zip components |
| `./moq git <args>`, `git-status`, `git-pull` | Run git in the root repository and every component repository |

A `myaddons.xml` repository can name a server's `addons.xml`, such as the one a group of a hosting server serves of its
public components with their clone addresses (`B/<group>/-/addons.xml` on an MR PASS server):
`<repository name="team"><location type="addons" url="https://team.example.com/B/acme/-/addons.xml"/></repository>`.
The get tasks fetch it once per run (https; http only to a loopback address; or a `file:` URL) and look components and
repositories up in it after `myaddons.xml` and `addons.xml`, so `./moq component-get acme-site` clones from the
server's own git location, and a name `addons.xml` defines (`mr-mantle`, `apps`, `github`) stays the official one. A
component name is letters, digits, `-` and `_`, never a path. Its root's `default` and `default-repository` are not used. One that cannot be read, or is
not https, is skipped with a warning. A repository entry with only the `addons` location leaves the others to the
served file's repository of the same name; one with a `git` location of its own uses that.

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
| H2 by default | Embedded Postgres by default; a `database-conf-name` of `h2` also means embedded |
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
