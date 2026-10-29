# Running MR Moqui

How the binary finds its runtime and conf, the HTTPS listener and certificates, and production archives. Building is in
the [README](../README.md) and [Packaging](Packaging.md); containers and systemd are in
[podman/README.md](../podman/README.md).

## Runtime and conf

Run from the repo root. Paths default to `MOQUI_RUNTIME=runtime`, or the binary's own directory when it is a runtime
directory (see [runtime/README.md](../runtime/README.md)). Embedded Postgres keeps its data in `runtime/db/embedded`
and unpacks its binaries into `runtime/install/postgres`, or under `MOQUI_INSTALL`. Setting `entity_ds_db_conf` to
`postgres`, `mysql` or `mysql8`, with the other `entity_ds_*` properties, uses that server instead; the examples are in
`runtime/conf/MoquiDevConf.xml`.

With no arguments the binary starts the HTTP server (HTTP on `8080` by default, HTTPS on `8443`, cluster HTTPS on
`8444`) using `runtime/base-component/framework/conf/MoquiDefaultConf.xml` merged with the runtime conf: the file
`MOQUI_CONF` names, relative to the runtime directory or absolute (`./moq run -c FILE` sets it); else the instance's
`runtime/MoquiConf.xml` when present; else `runtime/conf/MoquiDevConf.xml` when present. `MOQUI_CONF` comes from the
environment, not `runtime/.env`, which is read after the conf merge.

Without the `moq` script, `moqui help` lists the commands and `moqui help load` describes one:

```sh
cargo run -p moqui
# or: ./target/debug/moqui, and ./target/debug/moqui load for the default seed, seed-initial, install data
# ./target/debug/moqui --port 8081 --conf conf/MoquiProductionConf.xml
```

`-D create-or-update=true` opts into Java's read-first create-or-update (SELECT, then CREATE or UPDATE). It defaults to
off, keeping the UPDATE-first form.

## HTTPS

Certificates are looked up by the requested domain in `runtime/cert` (git-ignored), in any of these layouts:
nginx-proxy's `<domain>.crt` and `<domain>.key` (`dhparam.pem` files are ignored), certbot's or acme-companion's
`<domain>/fullchain.pem` and `privkey.pem`, or mkcert's `<domain>.pem` and `<domain>-key.pem`. A subdomain falls back to
`_wildcard.<parent>` (mkcert) and then `<parent>` (nginx-proxy), one label up only, and then `default` serves any other
name. A domain with no certificate gets a refused handshake. Files are read again within seconds of a change, so
renewals need no restart.

In dev (`instance_purpose=dev`) an empty directory gets a self-signed `localhost` certificate. Outside dev with no
certificate, the HTTPS listener is not started, or the start fails when the port is set explicitly.
`--https-port N` or `webapp_https_listen_port` sets the port to listen on, and `off` disables it.

`webapp_https_port` is separate: it is the port clients see in generated URLs and https redirects (empty is 443), as in
Java Moqui. To make generated URLs use https, set `webapp_https_enabled=true`. Connecting straight to this listener, also
set `webapp_https_port=8443`. Behind a reverse proxy on 443, leave it empty.

The cluster listener's default port is `8444`, so the public HTTPS listener can have `8443`. Members that do not set
`cluster_https_port` need firewall rules and port mappings for `8444`.

## Production archives

`./moq archive` (with `--release` or `--enterprise` for those packages; [Packaging](Packaging.md)) writes a zip, or with
`--tgz` a gzipped tar, that expands to one directory: the contents of `runtime/` (the framework component is inside it)
without its logs, scratch files, saves and unpacked Postgres, with the binary and `runtime/conf/MoquiProductionConf.xml`
as `MoquiConf.xml` in it. The binary uses its own directory as the runtime when that has `base-component/`, and merges
the `MoquiConf.xml` there, so it runs as `<dir>/moqui` from anywhere; `--no-data` leaves out the database and secrets.

By hand: copy the binary into the runtime directory, with a `MoquiConf.xml` there or
`MOQUI_CONF=conf/MoquiProductionConf.xml`. From a checkout, `./moq run-production` runs the Default binary with the
production conf, or another with `--release` or `--enterprise`. `moqui version` prints a binary's package and features.

Podman and systemd are in [podman/README.md](../podman/README.md). One image runs alone, on embedded Postgres, or with
the external databases this tree supports: Postgres, Postgres and OpenSearch, or MySQL and OpenSearch.
[`systemd/moqui.service`](../systemd/moqui.service) runs the same binary with no container. The server terminates HTTPS
itself, so there is no proxy container.
