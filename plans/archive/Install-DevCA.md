# Shared runtime and development HTTPS

> Archived 2026-11-25: built, and verified on macOS and on Linux. The Linux run found two certificate defects, fixed the
> same day: the missing authority key identifier and the leaf that BoringSSL gRPC clients refused. What it left is in
> [Remaining-Work](../Remaining-Work.md#framework-internals) and [Testing-Needed](../Testing-Needed.md#build-and-release).
> Guides: [Run and deploy](../../docs/Run-Deploy.md) (where runtime files come from, the dev certificates),
> [Monitoring](../../docs/tools/Monitoring.md) (OTLP in dev), [Test client](../../docs/dev/JS-Test-Client.md).

## Development listeners

Stock development serves HTTPS and OTLP/gRPC on port 8443, cluster traffic on 8444 and SSH on loopback port 2222 in SSH-enabled builds. A numeric `--http-port` reopens HTTP. Test harnesses that request an explicit loopback address and the desktop private node continue to use HTTP.

Production keeps the HTTP fallback of 8080 and a dedicated gRPC port of 4317 when a service uses it. The webroot settings are `http-bind`, `http-listen-port`, `grpc-listen-port` and `grpc-listen-bind`. HTTP and HTTPS advertise their own URL ports.

## Per-user runtime

macOS and Linux use `~/.mr-moqui`, an absolute `MOQUI_SHARED_RUNTIME` override, or `off`. A checkout reads its own files first, then the binary's embedded files, then the shared `components/` and `conf/` for files neither has. Its top-level conf, keys, secrets, and database remain checkout-local.

`moqui install [--from DIR] [--with-data] [--force]` creates the installed runtime. `moqui --installed` selects it from any working directory. Checkout-local development works on Windows. Per-user installation and trust commands report the operating-system limitation.

## Certificate lifecycle

`moqui dev-cert` prepares the localhost chain. `./moq trust-dev-cert` establishes the shared issuer and trusts the public root. Each checkout receives a separate server key and leaf. A new issuer or a leaf within 30 days of expiry triggers renewal. Existing unmanaged localhost and default identities require an explicit backup and reset.

The root key is discarded after signing a constrained intermediate. Only its intermediate key persists. The restrictions cover localhost DNS names, IPv4 loopback and IPv6 loopback. There are two leaves with one key, each valid for up to 397 days with the server authentication usage and the authority key identifier. `localhost` names only `localhost` and is served when the client asks for it. `default` adds the loopback addresses and is served when the client sends no name, as for `https://127.0.0.1`. The served chain contains the leaf and intermediate. Clients and exporters can load the public root from `runtime/cert/dev-ca/ca.crt`. Installations and archives exclude the signing key even with data.

For deployment and trust commands, see [Run and deploy](../../docs/Run-Deploy.md). For exporters, see [Monitoring](../../docs/tools/Monitoring.md). For browser specs, see [Test client](../../docs/dev/JS-Test-Client.md).

## macOS verification

Two independent checkouts served distinct localhost leaves under one root trusted in the login keychain. Ordinary curl returned a ready health response, and Chromium opened the login page with a fresh profile. Each checkout passed an authenticated WebSocket check and accepted a trace export over gRPC on 8443. Socket inspection showed only 8443, 8444 and loopback 2222 on the server process. The remote TUI authenticated on 8443 and displayed the title of a live notification. With the documented Android loopback HTTP settings, the mobile login and client configuration endpoints both returned 200 on 8080.

A disposable installed runtime started from an empty directory and passed an HTTPS health check. A checkout also served with unusable top-level configuration, environment, database and server identities present in its disposable shared directory. Component discovery loaded a shared-only addon and selected the checkout copy when its manifest was overridden locally.

## Linux verification

On 2026-11-25 the run used Ubuntu 26.04.1 LTS (kernel 7.0.0, x86-64), Google Chrome 158.0.8127.36, snap Firefox 159.0.1,
OpenSSL 3.5.5 and Python 3.14.4. It ran on a release build of master `6c7821559` with the fix below. The trust command
named `~/.pki/nssdb`, the snap Firefox profile and `/usr/local/share/ca-certificates`.

- **Old certificates.** The checkout still had a self-signed `localhost` and `default` pair. `./moq trust-dev-cert`
  refused it and named `moqui dev-cert --replace`, which moved the pair into `cert/replaced-<time>`. The refusal had
  already made the shared issuer.
- **Trust.** Two runs gave the same root, `0F846A7E…23EED87D`, with `C,,` trust in both NSS databases. The new leaf was
  valid 397 days and had the server authentication usage.
- **Listeners.** The moqui process listened on 8443, 8444 and loopback 2222 only. The ready report said HTTP and
  gRPC were off.
- **Clients.** curl with `--cacert` and headless Chrome with a fresh profile loaded `/health` and the login page. The
  remote TUI logged in on 8443. A sign-in from a new client sent the "New sign-in to your account" notice, and the
  TUI showed it live from `/notws`.
- **OTLP failed at first.** Python's OTLP/HTTP exporter refused the chain: Python 3.13 and later verify strictly, and
  the leaf and intermediate had no authority key identifier. The grpcio exporter refused it too: BoringSSL cannot
  check an IP address against the intermediate's IP name constraints, and refused even `localhost`. Chains made with
  openssl showed the same: a leaf naming only DNS passes BoringSSL, and removing the IP constraints would let a leaked
  key sign for any address. The fix (`crates/moqui-host/src/dev_issuer.rs`) adds the authority key identifier to every
  certificate. It also splits the leaves: `localhost` names only the DNS name, and `default` adds 127.0.0.1 and ::1
  for clients that send no SNI.
- **After the fix.** `moqui dev-cert --rotate` and the trust command gave the new root `CCE92F6C…41A19AB7` and removed
  the old one from both NSS databases. curl, Python and Chrome then verified by `localhost` and by `127.0.0.1`. A
  span and a log record went over OTLP/gRPC and OTLP/HTTP to `https://localhost:8443` with
  `OTEL_EXPORTER_OTLP_CERTIFICATE` and an ingest key, and the receiver accepted them. Both spans showed in
  Monitor › Traces. Monitor › Logs showed no rows for any service, the server's own included (Remaining-Work).
  `https://[::1]:8443` was refused because the dev listener binds `0.0.0.0`.
- **A second checkout.** A worktree run with the same binary served the same root and its own leaves. curl, Chrome,
  the TUI and gRPC worked there with no trust step.
- **Installed runtime.** `moqui install` went into a disposable `MOQUI_SHARED_RUNTIME`, then `moqui --installed`
  started from an empty directory (with `MOQUI_CONF=conf/MoquiDevConf.xml` for a dev instance). It logged
  `Runtime: installed at …` and its own CA, and answered health over HTTPS. The empty directory stayed empty. The
  checkout then started on its own runtime, `runtime/.env` and leaf.
- **System trust.** `--system` added the anchor (and Java's store took it). After that, plain curl and Python's
  default context verified by name and by address. Chrome and Firefox loaded both URLs without a warning.
- **Untrust.** `--system --untrust`, run twice, removed the anchor and the root from both NSS databases. The second
  run changed nothing. `--untrust` twice then found nothing to remove. Fresh curl, Python and Chrome
  (`ERR_CERT_AUTHORITY_INVALID`) refused, and so did Firefox after a restart.
  `update-ca-certificates` left the anchor's two `/etc/ssl/certs` links dangling. `trust.sh` now refreshes with
  `--fresh` on removal, and a moq test checks it with a stubbed store; the real store has not been run (Testing-Needed).
- **Afterwards.** The user stores were trusted again, and the system store left untrusted.
