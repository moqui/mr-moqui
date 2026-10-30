# Podman and systemd

MR Moqui runs as one process. It can use the embedded Postgres it ships, and it terminates HTTPS itself from certificates in the runtime `cert/` directory (the old nginx-proxy file names still work). These files are the Podman image and four stacks, plus a systemd unit that runs the binary with no container. Choosing a deployment, its properties, upgrades and clusters are in [docs/Run-Deploy.md](../docs/Run-Deploy.md).

There is no nginx-proxy container. Nothing here publishes port 80 or 443. HTTP is 8080 and HTTPS is 8443, so a rootless Podman can bind them. Put a certificate in the data volume's `cert/` directory, then set `webapp_https_enabled=true` and `webapp_https_port=8443`. Until a certificate is there the HTTPS port is not listening; production does not mint one, and forcing the listen port with an empty `cert/` makes startup fail.

## Image

The image is the Maximal package: embedded Postgres and the MySQL driver, no dev tools. Debian 13 supplies glibc (the binary needs 2.38 or newer) and the libraries embedded Postgres loads, including `libxml2.so.2`. Chromium is not in the image. PDF stays unavailable until `pdf_chromium_path` points at one.

```sh
podman/build.sh            # ./moq build --release, then the image localhost/moqui:latest
podman/build.sh --nobuild  # image only, from an existing target/release/moqui
```

The image copies `runtime/base-component` and whatever is checked out under `runtime/component` (no `.git` and no `target/`). Add components before building the image.

The container starts as root so it can chown a new data volume, then drops to uid 1000 before the binary. Embedded Postgres will not run as root. One volume, mounted at `/var/lib/moqui`, holds `db`, `log`, `cert`, `tmp`, `install`, `txlog`, and the generated secrets file. Those paths are symlinks into `/opt/moqui`, which is the runtime directory.

## Compose

`podman compose` needs a provider on `PATH`. On this layout that provider is Docker Compose, and it talks to the Podman API socket rather than to the `podman` command. Start the socket once per login (rootless):

```sh
systemctl --user start podman.socket
```

A system-wide Podman uses `sudo systemctl start podman.socket` instead. Each compose file sets the project name `moqui`, so run one stack at a time (two stacks would share the network and the `moqui-data` volume). Image names are fully qualified. There is no `container_name` and no `links`; containers reach each other by service name.

| File | Containers |
|---|---|
| `moqui-compose.yml` | moqui only. Embedded Postgres, search schema on that database. |
| `moqui-postgres-compose.yml` | moqui and Postgres 18. Search is the `search` schema on that server. |
| `moqui-postgres-opensearch-compose.yml` | moqui, Postgres 18, and OpenSearch 3.4.0. |
| `moqui-mysql-opensearch-compose.yml` | moqui, MySQL 9.5, and OpenSearch 3.4.0. |

```sh
cp podman/env.example podman/.env   # then change the passwords
podman compose -f podman/moqui-compose.yml up -d
podman exec moqui-moqui-1 curl -fsS http://127.0.0.1:8080/health
```

`/health` answers `{"ready":true}` only for loopback and for addresses in `webapp_status_ips` (default `127.0.0.1`). The check above runs inside the container, so it is loopback. A curl from the host to the published port is seen as the Podman bridge address and gets 404 until that address is added to `webapp_status_ips`.

`podman/.env` supplies `MOQUI_DB_NAME`, `MOQUI_DB_USER`, `MOQUI_DB_PASSWORD`, and `MOQUI_DB_ROOT_PASSWORD`. The defaults are `moqui` / `moquiroot`, for a local try. Postgres `5432` and MySQL `3306` publish on `127.0.0.1` only. OpenSearch is not published.

An empty `entity_ds_sslmode` means verified TLS. The database containers do not serve a certificate this client can verify, so the external stacks set `entity_ds_sslmode=disable` on the compose network. For a real server with TLS, set `verify-full` and `entity_ds_sslrootcert` to its CA file.

OpenSearch's client checks certificates and has no setting for a private CA. The image's HTTPS certificate is self-signed, so these stacks turn the security plugin off and use `http://opensearch:9200` inside the compose network. `opensearch_run=false` keeps the server from launching a copy under `runtime/opensearch`. OpenSearch runs as uid 1000 and refuses to start as root, so it cannot chown a new volume. Docker Compose, the usual `podman compose` provider, also drops Podman's `:U` volume flag, so the files do not use it. A one-shot `opensearch-init` container (Debian, as root) chowns the data volume to 1000 before OpenSearch starts. The host also needs the same sysctl Docker requires, or the OpenSearch process refuses to start:

```sh
sudo sysctl -w vm.max_map_count=262144
```

`bootstrap.memory_lock` is off. Rootless Podman usually cannot grant `IPC_LOCK`.

The first start creates the tables and, on an empty database, loads the seed, seed-initial and install data (`entity_empty_db_load`). To load more later, such as the data of a component added to the image, use a one-off container while the server is stopped (a second process cannot open an embedded data directory the server already holds):

```sh
podman compose -f podman/moqui-compose.yml run --rm moqui load types=seed,seed-initial,install
```

`compose run` goes through the entrypoint, so the load runs as uid 1000. `compose exec` does not: it is root unless you pass `-u moqui`.

Switching from the embedded stack to an external database does not move the embedded data. The external database starts empty. `moqui-data` still holds logs, certificates, and generated secrets.

## systemd, no container

`systemd/moqui.service` runs `/opt/moqui/moqui`. That directory is an unpacked `./moq archive` (or a checkout laid out the same way): the binary, `base-component/`, and `MoquiConf.xml`. The process notifies systemd when the listener is bound (`READY=1`) and when it is stopping (`STOPPING=1`), so the unit is `Type=notify`. An in-app restart execs the binary in the same PID and sends `RELOADING=1` instead, so the unit stays up. No `NOTIFY_SOCKET` means those messages are skipped, which is how `./moq run` behaves.

```sh
sudo systemd-sysusers systemd/moqui.sysusers
sudo install -d -o moqui -g moqui /opt/moqui
# unpack the archive into /opt/moqui, or copy a built runtime there
sudo cp systemd/moqui.service /etc/systemd/system/moqui.service
sudo systemctl daemon-reload
sudo systemctl enable --now moqui
```

`/etc/moqui/environment` is optional. Put `MOQUI_CONF` or `MOQUI_RUNTIME` there when they must exist before the process starts. The binary reads `/opt/moqui/.env` on its own after the conf merge, and a `MOQUI_CONF` line in that file does not choose the conf, so the unit does not load `.env` as its environment file.

For a database on the same machine, the examples in `systemd/drop-in/` add `After=` and `Wants=`. Copy one to `/etc/systemd/system/moqui.service.d/`. A remote database needs neither.

## Quadlet

`podman/quadlet/moqui.container` is the single-container stack as a systemd service. `Notify=true` uses the same `READY=1` message, and `Pull=never` keeps it on the local image. The external stacks stay compose files, so the environment is written down once.

```sh
mkdir -p ~/.config/containers/systemd
cp podman/quadlet/moqui.container podman/quadlet/moqui-data.volume ~/.config/containers/systemd/
systemctl --user daemon-reload
systemctl --user start moqui.service
```

Rootless user services need a lingering user (`loginctl enable-linger`) if they should run without a login. A system-wide copy goes in `/etc/containers/systemd/` and is `systemctl start moqui.service` without `--user`.
