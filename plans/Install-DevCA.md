# Install and the dev CA: dev over HTTPS only, and the shared ~/.mr-moqui runtime

Plan; nothing built.

## Why

A dev start opens three network listeners where one would do:

- **Plain HTTP on 8080**, on every address.
- **HTTPS on 8443.**
- **The gRPC listener on 4317.** The dev conf turns the OTLP receiver on, and that opens 4317 and also serves gRPC
  on 8443.

**Goal:** the default dev conf opens HTTPS and the cluster port only. Every dev tool reaches `https://localhost:8443`
with no certificate warning and no `--insecure`. 4317 stays where gRPC is on today: the default (production) conf's
default, and `MoquiAuthorityConf.xml`.

**One trust for every checkout.** Trusting the dev certificate once must cover every checkout and worktree. So the
dev CA lives in a new shared directory, `~/.mr-moqui/`, which this plan also introduces as a general feature:

- laid out like `runtime/`, which is also the root of a `./moq archive` zip;
- a layer between the start directory's runtime and the binary's embedded files;
- filled by `moqui install`;
- used as *the* runtime with `moqui --installed`.

This is a deliberate difference from how a deb install might spread files over `~/.local`, `~/.config` and the
rest: one standard runtime layout, not split.

## Where things stand

- **Runtime resolution is in one place:** `runtime_framework_dirs()`, `default_runtime_dir()` and
  `runtime_dir_in()` (`crates/moqui-host/src/lib.rs:1047-1072`). The order is `MOQUI_RUNTIME`, then
  `<cwd>/runtime`, then `<cwd>` when it is a runtime (an unpacked archive), else a new `<cwd>/runtime`. `moq`
  exports `MOQUI_RUNTIME`.
- **Embedded files are vfs overlays.** `crates/moqui-lib/src/vfs.rs` has one `Overlay` per mount, and the disk comes
  first. `embedded_files::mount()` (`crates/moqui-host/src/embedded_files.rs:50`) mounts one per set, and
  `Set::dir_in()` (`crates/moqui-embedded/src/lib.rs:153`) maps the sets to their directories.
- **No home-directory crate is used.** `tui_remote.rs:138` builds `~/.config/moqui/...` by hand from `$HOME` or XDG.
- **The HTTP listener cannot be off.** `http-port` is a u16 or empty (`moqui_core::web::effective_http_port`).
  HTTPS and gRPC take their bind IP from the HTTP address (`lib.rs` ~1701, `tls::plan`,
  `grpc_listen::bind_address_by`).
- **The gRPC listener's settings are split** between `telemetry/receiver@grpc-bind`/`@grpc-port` and
  `pass-authority@grpc-host`/`@grpc-port`. The PASS authority shares the receiver's port when both are on
  (`grpc_listen.rs:42-71`).
- **The dev certificate is a self-signed leaf, not a CA** (`tls.rs:787 ensure_dev_cert`).
- **reqwest 0.13 checks certificates through `rustls_platform_verifier`.** `ClientBuilder::tls_certs_merge` adds
  extra roots on top of the platform verifier (`Verifier::new_with_extra_roots`).
- **The hand-built rustls sockets trust only `webpki_roots`:** `tui_remote.rs:362` (the push socket) and
  `test_client/raw.rs:56` (raw and ws).
- **There is no shared reqwest builder.** Each tool builds its own: `test_client/mod.rs:378 http_client`,
  `load_cmd.rs:382`, `test_client/mod.rs:1125 ClientHost::for_server_run`, `tui_remote.rs:78`,
  `devtools_cmd.rs:251`, `devtools_remote.rs:90`, `moqui-client/src/http.rs:35`, `moqui-client/src/socket.rs:245`.
- **Two servers stay plain HTTP on loopback:**
  - `--serve` test servers bind plain HTTP on `127.0.0.1:0` through `ListenBind::Address`
    (`test_cmd/serve.rs:198`).
  - The Desktop node binds its own loopback HTTP (`moqui-desktop/src/node/mod.rs:313`).

## Build order

One commit per phase, each with focused tests; `./moq test-framework` once at the end.

### Phase 1: listener settings; no 4317 in dev

**Conf**, in `MoquiDefaultConf.xml`, the XSD, and `conf_schema.rs` as LISTENER:

- **`webapp[webroot]@http-listen-port=""`** is the port the HTTP listener binds, as `https-listen-port` is for HTTPS.
  - Empty means `http-port` (8080 when that is empty); `off` means no HTTP listener; `0` means an ephemeral port.
  - `http-port` goes back to being only the port in generated URLs.
  - `http-bind` stays the address of every web listener, even with HTTP off.
- **The dedicated gRPC listener gets its settings on the webapp:**
  - `grpc-listen-port="4317"`, or `off` for none;
  - `grpc-listen-bind=""`, the `http-bind` address when empty.

  It opens only when a gRPC service is on, as now. The services are always served on HTTPS over HTTP/2.
- **Remove the old gRPC settings** (no migrations before 0.1.0):
  - `telemetry/receiver@grpc-bind`/`@grpc-port` and `pass-authority@grpc-host`/`@grpc-port`;
  - the "shares its port" warning (`grpc_listen.rs:55-64`) and `pass_address`;
  - `PORT_PROPERTY` and `HOST_PROPERTY` in `pass_authority/mod.rs` and `pass_authority_off.rs`.

  `pass_allowed` keeps its TLS, loopback or insecure rule for the dedicated port.
- **`runtime/conf/MoquiDevConf.xml`:**
  - `grpc-listen-port="off"`: the receiver stays on, served at 8443.
  - `https-enabled="true"` and `https-port="8443"`, so generated URLs are `https://localhost:8443`
    (`factory.rs:789 webapp_root_url_for`, CI `public_base_url`).
  - Until Phase 6, HTTP on 8080 stays on but bound to loopback only; HTTPS keeps `http-bind`'s address.
- **`MoquiAuthorityConf.xml` is unchanged** and inherits 4317.

**Code:**

- **The web listeners** (`lib.rs listen_address` ~1765; `http.rs listen` and `record_web_listeners`, 187-293):
  - take the bind IP from `http-bind` whatever the HTTP port is;
  - skip the HTTP bind for `off`, and say `HTTP off (http-listen-port)` in the ready report;
  - `ListenBind::Address` (tests, `--serve`) always binds HTTP at its address.
- **The port check** (`port_check.rs:126-197 targets`): HTTP is optional when off, and HTTPS and the cluster port are
  no longer gated on an HTTP address.
- **`grpc_listen.rs` and `monitor/grpc.rs:42-62`** read the new webapp settings. The gating of gRPC on HTTPS
  (`http.rs:481-494`) does not change.
- **`test_cmd/serve.rs OTHER_PORTS`** (41-45) forces `grpc-listen-port` to 0 when the conf sets it.
- **`-p`/`--http-port` accept `off`:** `usage.rs` (`parse_port`), `scripts/moq/build.sh:72-93` and `core.sh:399`.

**Tests:**

- `grpc_listen_tests.rs` and `port_check_tests.rs`;
- `tests/it/monitor_grpc.rs`: with the dedicated port off, the receiver is still served on HTTPS (the existing
  "sender can use the HTTPS port" test);
- a host test that `http-listen-port=off` binds only HTTPS.

### Phase 2: the shared directory `~/.mr-moqui/`

**New module `crates/moqui-lib/src/shared_runtime.rs`.** It is in moqui-lib so moqui-client can use it too.

- **`shared_dir() -> Option<PathBuf>`:**
  - `MOQUI_SHARED_RUNTIME` when set: an absolute path, or `off`;
  - else `$HOME/.mr-moqui` on Linux and macOS (`cfg(all(unix, not(target_os = "android"), not(target_os = "ios")))`);
  - else `None`.

  It counts as present when it is a directory. It is created with mode 0700, since it holds the CA key and may hold
  a database.
- **Layout:** a runtime, the same as an archive root: `components/`, `conf/`, `cert/`, `db/`, `log/`,
  `MoquiConf.xml`, `.env` and the `moqui` binary.

**Layering, when not `--installed`:** the runtime's disk, then the shared directory's disk, then the embedded files.

- **Only the definition trees layer:** `components/` (with framework, tools and webroot) and `conf/`.
- **Never used by another runtime:** a shared `MoquiConf.xml`, `.env`, `db/`, `log/` and server certificates. A
  shared install's production `MoquiConf.xml` and its secrets must not reach a dev checkout. The one other use across
  runtimes is `cert/dev-ca/` (Phase 3).
- **`vfs.rs`** gets two new overlays: `DirOverlay` reads a directory on disk; `ChainOverlay` takes the first overlay
  that has a file, and merges directories.
- **`embedded_files::mount()`:**
  - Each mount point gets `Chain[Dir(shared/<same path>), Builtin]`.
  - `components/` and `conf/` are mounted whenever the shared directory has them, even where the binary embeds
    nothing.
  - `embedded diff` and `check()` (`embedded_files.rs:127`) say which layer a file came from.

**`--installed`** is a global option, taken off the arguments at the top of `dispatch()` (`lib.rs:806`) and set as
a process property, as `MOQUI_CONF` is.

- The runtime *is* `shared_dir()`. The start directory and `MOQUI_RUNTIME` are ignored, with a warning when
  `MOQUI_RUNTIME` is set. There is no shared layer then.
- It is an error when the shared directory is not a runtime (`layout::is_runtime`): "run `moqui install` first".
- It feeds `runtime_framework_dirs()`.

**`moqui install [--from <dir>] [--with-data] [--force]`** is a new arm in `dispatch()`, with its usage page in
`usage.rs` (`command_help`, `command_usage`, `is_known_command`).

- **With no `--from`:** write the binary's embedded files (reusing the writer of `embedded extract` in
  `embedded_files.rs`), and copy `current_exe()` to `~/.mr-moqui/moqui`.
- **`--from <runtime directory or unpacked archive>`:** copy it, leaving out what `./moq archive` leaves out:
  - always: log, tmp, install, .git and save;
  - db, txlog, .env and cert unless `--with-data`.

  Share the exclusion rules with `archive-excludes.txt` and `moqui_embedded::select` rather than keeping a third list.
- **It never overwrites a differing file without `--force`.** It lists them, as `embedded diff` does, and prints
  what to add to PATH.

**Docs:**

- `docs/Run-Deploy.md` gets a "Shared runtime (~/.mr-moqui)" section: the layering order, `--installed`,
  `install`, and the deliberate difference from an XDG or deb split.
- `usage.rs` OVERVIEW, Environment: `MOQUI_SHARED_RUNTIME`.

**Test isolation:** `moqui test`, the `test-client --serve` children and the cargo tests set
`MOQUI_SHARED_RUNTIME=off`, so a developer's `~/.mr-moqui` never changes a result.

**Tests:**

- runtime resolution in `lib.rs` (`runtime_dir_in`, `--installed`);
- the vfs chain and directory overlays;
- `usage_tests.rs`;
- an install round trip into a temporary `MOQUI_SHARED_RUNTIME`.

### Phase 3: the dev CA and one client trust helper

**`tls.rs ensure_dev_cert` (787):**

- **The CA is `cert/dev-ca/ca.crt` and `ca.key`.** The scan in `tls.rs:475` skips `dev-ca/`, so the CA is never
  served.
- **Which CA it uses, in order:**
  1. `~/.mr-moqui/cert/dev-ca/` when present: it signs with the shared CA and copies only `ca.crt` into the runtime;
  2. else the runtime's own `cert/dev-ca/`;
  3. else it generates a new CA there.
- **The CA itself:** 10 years, `CA:true`, keyCertSign, and **name constraints** that permit only `localhost`,
  `.localhost`, 127.0.0.0/8 and ::1, so a leaked key cannot mint certificates for real sites.
- **The leaf**, `localhost` and `default` as now (SANs localhost, 127.0.0.1, ::1):
  - signed by the CA, EKU serverAuth;
  - valid for 397 days (macOS refuses TLS leaves of more than 825 days);
  - issued again within 30 days of expiry, or when it is not signed by the current CA.

  Keys are written with `write_private`.

**New `crates/moqui-lib/src/dev_ca.rs`:**

- **`for_url(url) -> Option<CertificateDer>`** answers only for a loopback host (`localhost`, 127/8, ::1). It reads
  `<MOQUI_RUNTIME or ./runtime>/cert/dev-ca/ca.crt`, then `~/.mr-moqui/cert/dev-ca/ca.crt`.
- **`reqwest(builder, url)`** uses `tls_certs_merge`, which keeps the platform verifier.
- **`rustls_roots(url)`** gives the webpki roots plus the CA, for the hand-built sockets.
- **Used in:**
  - `test_client/mod.rs:378 http_client` and `raw.rs:56`;
  - `load_cmd.rs:382` and `test_client/mod.rs:1125`;
  - `tui_remote.rs:78`, and its push socket at 362, which then no longer trusts only `webpki_roots`;
  - `devtools_cmd.rs:251` and `devtools_remote.rs:90`;
  - `moqui-client/src/http.rs:35` and `socket.rs:245`, in debug builds only, as its rule for http to loopback is.
- **Browser specs** (`test_client/browser.rs:774 browser_args`): for a loopback server, pass
  `--ignore-certificate-errors-spki-list=<SPKI sha256 of the dev CA>`. `--insecure` keeps
  `--ignore-certificate-errors`.

**Tests:**

- `tls_tests.rs`: the leaf verifies against the CA with rustls/webpki, the name constraint refuses another name, and
  the shared CA is preferred;
- `dev_ca`: loopback only;
- a test-client raw and ws test against an HTTPS test server.

### Phase 4: `./moq trust-dev-cert`

A new task in `scripts/moq/`, in the `moq` task table, `./moq help` and `completion.sh`:

1. **Make sure `runtime/cert/dev-ca/` exists.** A new small `moqui dev-cert` command runs `ensure_dev_cert`, so it
   needs no server start and no openssl.
2. **Copy `ca.crt` and `ca.key` to `~/.mr-moqui/cert/dev-ca/`** (directory 0700, key 0600). From then on, dev
   certificates in every checkout are signed by it. Remove a runtime's old self-signed `localhost` and `default` pair
   so the next start issues new ones.
3. **Trust it for the user, without sudo:**
   - Linux, when `certutil` exists: NSS, `~/.pki/nssdb` for Chrome and Chromium and each Firefox profile's
     `cert9.db`;
   - macOS: `security add-trusted-cert -r trustRoot` into the login keychain.

   It prints the system-store commands for curl and OTel senders (Debian `update-ca-certificates`, Fedora
   `trust anchor`). `--system` runs them with sudo, and `--untrust` removes the CA everywhere.

### Phase 5: tools and docs default to https://localhost:8443

**Tools:**

- `test_client/mod.rs:73 DEFAULT_SERVER` becomes `https://localhost:8443`.
- In `scripts/moq/test.sh:14-40`, `-p N` becomes `https://localhost:N`. Update the help text in `test.sh:653-687`
  and `archive.sh:61`.
- `tui_cmd.rs:186-191 served_on` probes the HTTPS listen port and suggests `--server https://localhost:8443`.
- `moqui-desktop node/mod.rs:388 served_on_runtime_port` probes HTTPS too.
- dev-aide `GroupServers.spec.js:85` baseUrl.
- `scripts/android-smoke.sh` and `docs/Build-Test.md:507` start the server with `--http-port 8080` for
  `adb reverse`.

**Docs:**

- the README quick start;
- `docs/Run-Deploy.md`:
  - the Ports table: HTTP off in dev, 4317 on by default where gRPC is on;
  - HTTP/2 gRPC;
  - Certificates: the dev CA and `trust-dev-cert`;
  - the SSO loopback example at ~1964, moved to https;
- `docs/tools/Monitoring.md:151-156`: send to 8443, with `OTEL_EXPORTER_OTLP_CERTIFICATE`;
- `docs/security/Attack-Surface-Routes.md` rows 18 and 20: the setting names;
- `docs/dev/JS-Test-Client.md`, `docs/tools/Load-Runner.md`, `docs/ui/TUI.md`, `docs/ui/Mobile-Client.md`;
- the curl examples in `webroot/screen/webroot/rest.xml`;
- the `MoquiDefaultConf.xml` comments.

The podman healthchecks (`podman/Containerfile:50`, `quadlet/moqui.container:22`) stay on HTTP, since production
keeps 8080.

### Phase 6: dev conf with no 8080

In `runtime/conf/MoquiDevConf.xml`, set `http-listen-port="off"` and drop the interim loopback bind. The production
default is unchanged: `http-listen-port=""` (8080) and `grpc-listen-port="4317"`.

## Verification

**Per phase:**

- the focused cargo tests named above;
- `cargo lint` and `cargo lint-min`;
- `scripts/check-packages.sh` when a dependency or feature changes (rcgen name constraints, any new crate).

**At the end:** `./moq test-framework` (the pre-commit skill). No acceptance runs.

**By hand:**

- `./moq run`: the ready report lists HTTPS and Cluster only, with no 8080 and no 4317 (`ss -ltnp`).
- After `./moq trust-dev-cert`:
  - the browser opens `https://localhost:8443` with no warning;
  - `curl https://localhost:8443/health` works;
  - `./moq test-client framework` runs with no `--insecure`;
  - `moqui tui --server https://localhost:8443` works, push included;
  - an OTLP/gRPC send to 8443 works.
- A second worktree's fresh `runtime/cert` is signed by the shared CA and trusted with no new step.
- After `moqui install`, `moqui --installed` from an empty directory runs from `~/.mr-moqui`.
- A checkout sees a component placed in `~/.mr-moqui/components/addon/X`.

Commit each component repository touched (dev-aide) on its own.

## Open points

- **Nested vfs mounts.** How vfs chooses among nested mounts (`components/` around `components/base/framework`),
  so a deeper Builtin mount does not hide the shared tree.
- **Chromium's SPKI flag.** Whether `--ignore-certificate-errors-spki-list` matches a certificate in the chain (the
  CA) or only the leaf. If only the leaf, pass the leaf's SPKI.
- **Mobile.** Trust of the dev CA on Android (emulator or debug trust config) is deferred, so mobile dev keeps
  `--http-port 8080` with `adb reverse`.
- **Commands with their own runtime.** The commands that work out their runtime themselves (`devtools_cmd.rs:507`,
  `test_cmd`, `embedded extract <dir>`) each either honor `--installed` or refuse it.
