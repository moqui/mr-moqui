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
    [Dev instances with a dev group server](#dev-instances-with-a-dev-group-server);
  - reference: [Components](#components), [Moving from Moqui Java](#moving-from-moqui-java),
    [Directory layout](#directory-layout).

## Quick start

### Develop on your machine

You need Rust 1.95 or newer (`rustc --version`), git and bash, on Linux or macOS.

```sh
git clone https://github.com/moqui/mr-moqui.git
cd mr-moqui
./moq component-get apps   # optional: Marble ERP (mr-marble) and IronStone BOSS (mr-boss), with Mantle (mr-mantle)
./moq load                 # builds the Dev package the first time, then loads every data type, demo included
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
| Load one file | `./moq load -l runtime/component/mr-mantle/data/SomeData.xml` | `java -jar moqui.war load location=...` |
| Start over with an empty database | `./moq clean-db && ./moq load` | `./gradlew cleanDb load` |
| Load once, then save a snapshot | `./moq load-save` | `./gradlew loadSave` |
| Save a snapshot / restore it | `./moq save-db before-upgrade`, `./moq reload-save before-upgrade` | `./gradlew reloadSave` |
| Pull every repository | `./moq git-pull` | `./gradlew gitPullAll` |
| Status of every repository | `./moq git-status` | `./gradlew gitStatusAll` |
| Get a component and its dependencies | `./moq component-get mr-boss` | `./gradlew getComponent -Pcomponent=...` |
| Run a component's specs | `./moq test mr-mantle` | `./gradlew runtime:component:mantle-usl:test` |
| Run client specs against a server | `./moq test-client framework` | |
| Archive the runtime with the binary | `./moq archive --release` | `./gradlew addRuntime` |
| Remove the database, logs and old build output | `./moq clean-all` | `./gradlew cleanAll` |
| Show the binary's package and features | `target/develop/moqui version` | |

## Choose a topology

| Topology | Use it for | Package | Database | Section |
|---|---|---|---|---|
| One dev instance | Developing on your machine | Dev (the default) | Embedded Postgres | [Quick start](#develop-on-your-machine) |
| One production server | One business or one site | Maximal (`--release`) | Embedded Postgres, or an external server | [One production server](#one-production-server) |
| Cluster | More capacity, or a member that can fail | Enterprise (`--enterprise`) | One external Postgres for all members | [Cluster](#cluster) |
| PASS servers with a BOSS server or cluster | Public sites (forum, members) with no business data on the public machines | PASS: any server package, Lite is the smallest. BOSS: Maximal or Enterprise | Each PASS server its own; BOSS its own | [PASS](#pass-servers-with-a-boss-server-or-cluster) |
| Dev instances with a dev group server | A team: shared repositories, tasks and models | Laptops: Dev. Group server: Maximal or Enterprise | Each its own | [Dev group](#dev-instances-with-a-dev-group-server) |
| Public demo server | The apps for anyone to try with the `john.doe` login | Maximal (`--release`) | Embedded Postgres, restored on a schedule | [Public demo server](#public-demo-server) |

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
- **Listeners**: SSH, inbound SMTP, A2A, MCP and the LLM gateway are off by default. Turn on only the ones that are used.
- **CORS**: leave `webapp_allow_origins` empty. The dev conf sets `*`.
- **Lock-down**: the full list is in [Attack-Surface.md, Heavy lock-down](security/Attack-Surface.md#heavy-lock-down).

---

## Runtime directory and conf

The runtime directory holds everything that varies per instance: components, conf, certificates, the database, logs
and secrets ([runtime/README.md](../runtime/README.md)). The binary takes the first directory with a `base-component/`
in it, from this list:

1. `MOQUI_RUNTIME`;
2. `runtime` in the working directory;
3. `runtime` beside the binary;
4. the binary's own directory, which is how an unpacked archive runs as `<dir>/moqui` from anywhere.

Keep the runtime path short. The embedded Postgres socket, `<runtime>/db/embedded/.s.PGSQL.5432`, must be under 108
bytes, or the start fails with "postgres socket path is N bytes".

The conf is merged in this order:

1. `base-component/framework/conf/MoquiDefaultConf.xml`;
2. each component's `MoquiConf.xml`, in component order;
3. the runtime conf, on top.

The runtime conf is the first of these that is found:

1. the file `MOQUI_CONF` names, relative to the runtime directory or absolute (`./moq run -c FILE` and `moqui --conf
   FILE` set it);
2. `<runtime>/MoquiConf.xml`;
3. `<runtime>/conf/MoquiDevConf.xml`.

`./moq run` uses the dev conf and `./moq run-production` uses `conf/MoquiProductionConf.xml`. Conf merges add
components but never remove one. To leave a component out, keep it out of `runtime/component`. The example dev
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
covers `entity_ds_crypt_pass`, `session_cookie_key`, `ssh_host_key`, `git_lfs_token_key`, `cluster_tls_cert` and
`cluster_tls_key`. A new file gets mode 0600, and a `.env` that cannot be written fails the start.

The properties most deployments set or should know. Their defaults are in `MoquiDefaultConf.xml`.

| Property | Default | What it does |
|---|---|---|
| `instance_purpose` | `production` (`dev` in the dev conf) | `production`, `dev` or `test`. Only `dev` loads the dev components (Dev AIDE, the example component) |
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
| `pdf_chromium_path` | empty | The Chromium that prints PDF; empty looks in `runtime/chromium`, then on the PATH |
| `ssh_port`, `ssh_host` | empty, `127.0.0.1` | The SSH server (SFTP, git, the apps in a terminal); off while `ssh_port` is empty (`2222` in the dev conf) |
| `moqui_log_level`, `moqui_log_dir` | `info`, `runtime://log` | The console level, and the directory of `moqui.log` and `moqui-error.log` |
| `search_backend` | `postgres` | `postgres` or `opensearch` ([search](#search)) |
| `example_disabled` | `false` | `true` leaves out the example component and its gallery on a dev instance |
| `demo_disabled` | `true` | `false` loads the demo lock-down on a dev instance; a demo server names the component in its conf instead |
| `llm_openai_url`, `llm_openai_api_key`, `llm_openai_model` | `https://api.openai.com`, empty, empty | The default model endpoint for Assist and the LLM services (the dev conf points it at `http://127.0.0.1:1234`) |

The database properties are in [Database and search](#database-and-search), and the cluster properties in
[Cluster](#cluster).

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
| `moqui test`, `moqui test-client` | Component specs ([Testing](Build-Test.md#component-specs), [Client-Testing](dev/Client-Testing.md)) |
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
`--debug`, `--release`, `--enterprise`) pick the binary, and no flag means the Dev package. They build first unless
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

By default an entity's create-or-update is UPDATE first, then INSERT. `-D create-or-update=true` switches to Moqui
Java's read-first form (SELECT, then INSERT or UPDATE).

## Database and search

### Embedded Postgres

Without `entity_ds_db_conf` (or with Moqui Java's `h2`), the server runs its own Postgres. Every package but Enterprise
embeds it.

- **Data**: `runtime/db/embedded`.
- **Binaries**: unpacked from the moqui binary on the first start, into `runtime/install/postgres` or under
  `MOQUI_INSTALL`.
- **Connections**: on Unix there is no TCP port. The server connects through a socket in the data directory, so
  nothing else on the network reaches the database.

### External Postgres or MySQL

Set `entity_ds_db_conf` and the other `entity_ds_*` properties, in the environment, `.env` or the runtime conf. There
are examples in `runtime/conf/MoquiDevConf.xml`.

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

`./moq save-db [name]` writes the embedded database to `runtime/save/<name>/moqui.dump`. It also writes the OpenSearch
data when there is any. `./moq reload-save [name]` empties the database and restores that save. `./moq load-save -n
<name> [types]` runs `clean-all`, loads, and saves. Stop the server before a restore. These work on the embedded
database only; an external database has its own tools.

### Backups

The job `backup_EmbeddedDatabase_daily` backs up the embedded clusters while the server runs. It is paused in the seed
data; set it running on System › Service Jobs. It runs daily at 02:00 and keeps 7 daily, 5 weekly and 12 monthly
archives, and every yearly one, in `runtime/db/backup`.

- **Modes**: `backupMode=dump`, the default, writes one `pg_dump` file. `incremental` takes `pg_basebackup` fulls and
  block incrementals.
- **Mirror**: archives can be copied to an SFTP server. Set `sendUrl`, the user and a password or key on the
  `EmbeddedDbBackup` system message remote.
- **Restore**: restore a dump with `moqui load-save` while the server is stopped. A physical backup is put together
  with `pg_combinebackup`.
- **Not included**: `.env` and the field-encryption key are not in the archive. Keep them with the backup.

The details are in [plans/archive/PostgresBackup.md](../plans/archive/PostgresBackup.md). The job does not back up an
external database.

## HTTPS, ports and proxies

### Ports

| Port | Listener | Default | Properties |
|---|---|---|---|
| 8080 | HTTP | On, every address | `webapp_http_port`, `webapp_http_bind` |
| 8443 | HTTPS, certificates from `runtime/cert` | On when there is a certificate (dev makes one) | `webapp_https_listen_port` (`off`) |
| 8444 | Cluster HTTPS, client certificate required | On in every serving process, every address | `cluster_https_port`, `cluster_bind_address` |
| `ssh_port` | SSH: SFTP, git, admin shell, the apps (`ssh -p 2222 john.doe@localhost` in dev) | Off; 2222 on 127.0.0.1 in the dev conf | `ssh_port`, `ssh_host` |
| `smtp_listen_port` | Inbound SMTP behind a mail gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)) | Off; Maximal and Enterprise only | `smtp_listen_port`, `smtp_listen_host` |

`/health` answers `{"ready":true}` to loopback and to the addresses in `webapp_status_ips`, and 404 to everyone else.

### Certificates

Certificates are looked up by the requested domain in `runtime/cert` (git-ignored), in any of these layouts:

- nginx-proxy's `<domain>.crt` and `<domain>.key` (`dhparam.pem` files are ignored);
- certbot's or acme-companion's `<domain>/fullchain.pem` and `privkey.pem`;
- mkcert's `<domain>.pem` and `<domain>-key.pem`.

A subdomain falls back one label up only: first to `_wildcard.<parent>` (mkcert), then to `<parent>` (nginx-proxy).
After that, `default` serves any other name. A domain with no certificate gets a refused handshake. The files are read
again within seconds of a change, so a renewal needs no restart.

In dev (`instance_purpose=dev`), an empty directory gets a self-signed `localhost` certificate. Outside dev with no
certificate, the HTTPS listener does not start, or the start fails when the port was set explicitly.

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
- Leave `webapp_https_port` empty when the proxy serves 443.

## One production server

### From an archive

`./moq archive` writes a zip, or with `--tgz` a gzipped tar. It expands to one directory, which is a runtime directory
with the binary in it.

- **What goes in**: the package's binary; `runtime/conf/MoquiProductionConf.xml` as `MoquiConf.xml` (`--conf FILE` puts
  in another); `README.txt`; and all of `runtime/` except `log/`, `tmp/`, `save/` and `install/`.
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
`runtime/component`. One volume at `/var/lib/moqui` holds the database, logs, certificates and generated secrets.
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
3. On the server, remove the old `moqui` binary, `base-component/` and `component/`, so files deleted upstream do not
   linger, and unpack the new archive over `/opt/moqui`:

   ```sh
   sudo rm -rf /opt/moqui/moqui /opt/moqui/base-component /opt/moqui/component
   sudo cp -a mr-moqui-20261105/. /opt/moqui/ && sudo chown -R moqui:moqui /opt/moqui
   ```

   `db/`, `.env`, `cert/` and `log/` are not in the archive, so they stay. `MoquiConf.xml` is replaced, which is why
   per-host settings belong in `.env`.
4. Start the server: `sudo systemctl start moqui`. The table check adds new tables and columns. New seed data is not
   loaded on its own. To load it, stop the server, run `sudo -u moqui /opt/moqui/moqui load types=seed`, and start it
   again. Leave out `seed-initial`, whose rows the deployment may have changed since.

A deploy marker for the dashboards can be posted from the upgrade script ([Monitoring](tools/Monitoring.md#deploy-markers-and-annotations)).

### Optional services

| Service | How |
|---|---|
| PDF | Chromium: `./moq chromium-download` into `runtime/chromium`, or `pdf_chromium_path`. The server starts without it, and PDF is unavailable |
| OpenSearch | [Search](#search) |
| Outbound mail | SMTP through a `moqui.basic.email.EmailServer` record; every package has it |
| Inbound mail | IMAP polling, or the SMTP listener behind a gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)); Maximal and Enterprise |
| SSH | `ssh_port`, with `ssh_host=0.0.0.0` to reach it from other machines ([TUI](ui/TUI.md), [Sync](interfaces/Sync.md), [Git-Hosting](interfaces/Git-Hosting.md)) |

## Public demo server

A server where anyone can try the apps with the `john.doe` / `moqui` login. It runs on the production purpose with
`conf/MoquiDemoConf.xml`, which loads the demo component (`runtime/dev-component/demo`) and loads all data, demo users
included, on the first start:

```sh
./moq load --release -c conf/MoquiDemoConf.xml
./moq save-db --release demo                    # the database to go back to
./moq run-production --release -c conf/MoquiDemoConf.xml
```

The component keeps the apps writable and takes away running code or SQL, bulk import and export, the file manager,
writes in Tools and System, SSH, WebDAV, MCP, A2A, the LLM gateway, mail to other hosts, and changes that would lock
visitors out of the shared login ([its README](../runtime/dev-component/demo/README.md)). Restore the saved database on
a schedule with `./moq reload-save --release demo`, with the server stopped. Do not run a demo as `instance_purpose=dev`:
the Dev AIDE loads there.

## Cluster

Several members share one database and run behind a load balancer. Members find each other in the
`moqui.server.ClusterMember` table, with heartbeats on the database clock; there is no Hazelcast or Redis. Over HTTPS
with client certificates they send each other entity cache clears, notifications, co-editing updates and session ends.
Sessions are rows in the shared database, so any member can serve any request.

```
                   clients
                      | https
               load balancer             webapp_trusted_proxies on each member
           +----------+----------+
       member 1   member 2   member 3    8444 between members only (cluster_base_url)
           +----------+----------+
              shared Postgres            entity_ds_db_conf=postgres
```

### Requirements

- **One external database.** Postgres, or MySQL with the default transport. The Enterprise package has no embedded
  Postgres, so a member cannot start on a database of its own by mistake.
- **The same secrets on every member**: `session_cookie_key`, `cluster_tls_cert`, `cluster_tls_key` and
  `entity_ds_crypt_pass`. Startup fails when a live member has a different session key or certificate, with
  "session_cookie_key does not match a live cluster member" or "cluster_tls_cert does not match a live cluster member".
- **A distinct instance id**: each member keeps its own in `runtime/db/instance_id` (or `instance_id`). It is not in
  `.env`, so copying `.env` between members is safe.

### Steps

1. Start the first member once with the shared database settings, so it generates its secrets into `.env`.
2. Copy that `.env` to each other member before its first start.
3. Set `cluster_base_url=https://<this member's private address>:8444` on each member. Empty is
   `https://127.0.0.1:8444`, which logs an error once other members are live.
4. Open port 8444 between the members only.
5. If the load balancer does not keep a client on one member, set guaranteed session writes in the runtime conf:

   ```xml
   <webapp-list><webapp name="webroot"><session-config flush="guarantee"/></webapp></webapp-list>
   ```

6. Set `webapp_trusted_proxies` and `webapp_client_ip_header` for the load balancer, then start the other members.
7. Check the cluster with `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database.

### Transports

`cluster_transport` sets how peers reach this member:

| Value | How |
|---|---|
| empty (default) | HTTPS POST to `cluster_base_url` |
| `notify` | Postgres `LISTEN`/`NOTIFY` on the shared database, for members that cannot connect to each other; needs Postgres |
| `ssh` | Through the member's SSH server (`ssh_port`); needs the `ssh` feature |

### What is shared and what is not

- **Shared**: scheduled jobs take a lock row, so each job runs on one member at a time. Content blobs, `db` git
  repositories, sessions and the LLM quotas are in the database.
- **Not shared**:
  - `local` and `remote` git repositories are on one member's disk. Use `db` repositories, or send `/git` and SSH to
    that member.
  - Files on disk are each member's own: the SFTP root (`runtime://sftp`), and any setting that names a directory,
    such as `pass_media_root`. Share the directory between the members.
  - The LLM gateway's requests-per-minute and the web protection's limits are counted on each member separately.

**Not built**: shared bans across members; a screen listing the members.

## PASS servers with a BOSS server or cluster

IronStoa PASS (`mr-pass`) serves public sites: Forum, Account, Members and Sites. A PASS server can run without Mantle
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

In local mode, mr-pass runs in the same runtime as Mantle and makes in-process calls; that is the default when
`pass_backend_url` is not set. Provisioning, mail, media, workers and imports are in mr-pass
[docs/Deployment.md](../runtime/component/mr-pass/docs/Deployment.md). This section covers the remote layout.

### The authority

1. Run a BOSS server: `./moq component-get mr-boss`, which brings mr-mantle. It can be one server or a
   [cluster](#cluster). Use Maximal or Enterprise: forum thumbnails need `image`, and sign-in providers need `sso`.
   The site readiness check reports what is missing. The PASS jobs are in Mantle's data (`ForumSetupData.xml`) and run
   here. Add mr-pass only if this server also serves sites itself.
2. Register each PASS deployment with these properties, kept with the other secrets in `.env` or the environment:

   | Property | Value |
   |---|---|
   | `pass_client_<id>_secret` | The deployment's secret |
   | `pass_client_<id>_sites` | The site IDs it may serve, comma-separated |
   | `pass_client_<id>_callback` | The exact SSO callback, such as `https://forum.example.com/account/Sso` |

3. Create the sites and bind their hosts and mounts with `mantle.public.SiteServices.create#Site` and `store#Mounts`
   (Deployment.md, Provisioning).
4. When web protection is on (`webapp_trusted_proxies` set, or `webapp_web_protection=true`), exempt the PASS servers,
   or all their visitors count as one client:

   ```xml
   <webapp-list><webapp name="webroot"><web-protection>
       <client-rule name="pass-servers" action="exempt" cidr="10.0.2.0/24"/>
   </web-protection></webapp></webapp-list>
   ```

### A PASS server

1. Make a checkout that has no Mantle. mr-pass declares no `depends-on`, and a conf cannot remove a component that is
   in `runtime/component`:

   ```sh
   git clone https://github.com/moqui/mr-moqui.git pass && cd pass
   ./moq component-get mr-pass
   ```

2. Build it with any server package. PASS calls the authority for images and sign-in, so Lite, the smallest, is
   enough: `./moq archive --lite --no-data`.
3. Configure it in `.env` and `MoquiConf.xml`. Set `pass_backend_url` to the authority's final https origin, so that
   no call depends on a redirect.

   ```sh
   pass_backend_url=https://boss.example.com
   pass_client_id=forum
   pass_client_secret=...
   ```

   ```xml
   <webapp-list><webapp name="webroot">
       <public-site host="forum.example.com" site-id="FORUM_SITE" screen-location="component://mr-pass/screen/Pass.xml"
               resolver-service="pass.RouteServices.resolve#Route">
           <mount path="/forum" screen="Forum"/>
           <mount path="/account" screen="Account"/>
           <mount path="/members" screen="Members"/>
           <mount path="/sites" screen="Sites"/>
       </public-site>
   </webapp></webapp-list>
   ```

   The hosts and mounts here must match the ones bound at the authority.
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

### The group server

1. Run BOSS: `./moq component-get mr-boss` (which brings mr-mantle), on Maximal, or on Enterprise as a
   [cluster](#cluster). Set it up as in [One production server](#one-production-server).
2. **Git hosting** ([Git-Hosting](interfaces/Git-Hosting.md)):
   - HTTPS clones work as soon as HTTPS does: `https://team.example.com/git/<repositoryId>.git`.
   - For SSH, set `ssh_port` (such as 2222, since 22 is usually the system's sshd), `ssh_host=0.0.0.0`, and
     `ssh_public_port` for the clone URLs shown.
   - Grant `SSH_GIT` to the developers' group. Seed data grants it to ADMIN only.
   - Create repositories from a project's Repositories tab, or on the Git Repositories screen (System, Resource
     Finder). On a cluster, make them `db` repositories.
   - Commit trailers such as `Task: PROJ-12` and branch names link commits to Projects work items through Mantle's
     gitECA rules.
3. **LLM gateway** ([Gateway](ai/Gateway.md)):
   - set `llm_gateway_enabled=true` and the aliases developers call;
   - grant `LlmGatewayUse` to the developers' group;
   - for each developer's key, add an `LlmApiClient` row with the `gateway` scope (System › LLM › Gateway). Its
     `allowedAliases`, `dailyTokens` and `dailyCost` limit that key.
4. Optionally, turn on MCP (`mcp_enabled=true`) for coding agents. `/mcp/setup` gives a client config to paste
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

Components live in `runtime/component`, and dev-only ones in `runtime/dev-component`. Each is its own git repository.
`addons.xml` lists the known components and repositories. A `myaddons.xml` beside it, which is not in git, adds or
overrides `repository`, `component` and `component-set` elements. It can set `default="a,b"` (what `component-get` gets
with no names) and `default-repository` on its root.

| Task | What it does |
|---|---|
| `./moq component-get <name...>` | Get components and the components they depend on (`depends-on` in `component.xml`); a `component-set` name gets the components of the set (`apps`: mr-marble and mr-boss) |
| `./moq component-depends` | Get missing dependencies of what is in `runtime/component` |
| `./moq component-create <name>` | Make a component from the component template, `runtime/dev-component/template`: an entity, services, a REST API, screens, data and specs; `--rest` or `--screens` for one part |
| `./moq component-zip [name...]` | Zip components |
| `./moq git <args>`, `git-status`, `git-pull` | Run git in the root repository and every component repository |

The get tasks take `-t git|current|release|binary` and `-r <repository>`. `--dev` puts the named components in
`runtime/dev-component`. A component with a `Cargo.toml` is linked into the binary on the next build
([Native-Components](dev/Native-Components.md)). A change to the set of components takes a restart.

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
  moq                 the task script (bash 3.2 or newer)
  addons.xml          known components and repositories; myaddons.xml (not in git) adds to it
  crates/             the Rust workspace: moqui (the binary), moqui-host, moqui-core, moqui-lib, moqui-llm, moqui-db,
                      moqui-tui, moqui-browser
  runtime/            the runtime directory (runtime/README.md)
    base-component/   framework, tools, webroot
    component/        components from ./moq component-get (not in git)
    dev-component/    components that load only on a dev instance: dev-aide, example, demo (off until a conf names
                      it); template for component-create
    conf/             MoquiDevConf.xml, MoquiProductionConf.xml, MoquiDemoConf.xml
    cert/ db/ log/ .env   per-instance files (not in git)
  podman/             the image, compose stacks and Quadlet units
  systemd/            the unit, sysusers file and drop-ins
  scripts/            pg-payload.sh, check-packages.sh, wasm-build.sh
  docs/               reference guides
  plans/              designs, status and reviews
  target/             build output: target/develop/moqui, target/release/moqui, ...
```
