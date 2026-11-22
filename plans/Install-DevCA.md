# Shared runtime and development HTTPS

## Development listeners

Stock development serves HTTPS and OTLP/gRPC on port 8443, cluster traffic on 8444 and SSH on loopback port 2222 in SSH-enabled builds. A numeric `--http-port` reopens HTTP. Test harnesses that request an explicit loopback address and the desktop private node continue to use HTTP.

Production keeps the HTTP fallback of 8080 and a dedicated gRPC port of 4317 when a service uses it. The webroot settings are `http-bind`, `http-listen-port`, `grpc-listen-port` and `grpc-listen-bind`. HTTP and HTTPS advertise their own URL ports.

## Per-user runtime

macOS and Linux use `~/.mr-moqui`, an absolute `MOQUI_SHARED_RUNTIME` override, or `off`. A checkout reads its own files first, then shared definitions under `components/` and `conf/`, then embedded definitions. Its top-level conf, keys, secrets, and database remain checkout-local.

`moqui install [--from DIR] [--with-data] [--force]` creates the installed runtime. `moqui --installed` selects it from any working directory. Checkout-local development works on Windows. Per-user installation and trust commands report the operating-system limitation.

## Certificate lifecycle

`moqui dev-cert` prepares the localhost chain. `./moq trust-dev-cert` establishes the shared issuer and trusts the public root. Each checkout receives a separate server key and leaf. A new issuer or a leaf within 30 days of expiry triggers renewal. Existing unmanaged localhost and default identities require an explicit backup and reset.

The root key is discarded after signing a constrained intermediate. Only its intermediate key persists. The restrictions cover localhost DNS names, IPv4 loopback and IPv6 loopback. The leaf is valid for up to 397 days and carries the server authentication usage. The served chain contains the leaf and intermediate. Clients and exporters can load the public root from `runtime/cert/dev-ca/ca.crt`. Installations and archives exclude the signing key even with data.

For deployment and trust commands, see [Run and deploy](../docs/Run-Deploy.md). For exporters, see [Monitoring](../docs/tools/Monitoring.md). For browser specs, see [Test client](../docs/dev/JS-Test-Client.md).
## macOS verification

Two independent checkouts served distinct localhost leaves under one root trusted in the login keychain. Ordinary curl returned a ready health response, and Chromium opened the login page with a fresh profile. Each checkout passed an authenticated WebSocket check and accepted a trace export over gRPC on 8443. Socket inspection showed only 8443, 8444 and loopback 2222 on the server process. The remote TUI authenticated on 8443 and displayed the title of a live notification. With the documented Android loopback HTTP settings, the mobile login and client configuration endpoints both returned 200 on 8080.

A disposable installed runtime started from an empty directory and passed an HTTPS health check. A checkout also served with unusable top-level configuration, environment, database and server identities present in its disposable shared directory. Component discovery loaded a shared-only addon and selected the checkout copy when its manifest was overridden locally.

## Linux verification on a separate machine

Live Linux verification is pending.

Use two fresh checkouts under the same account. Record the distro, browser versions, root fingerprint and each store named by the trust command.

1. Run `./moq trust-dev-cert` in the first checkout. Run it again and compare the SHA-256 fingerprint of `runtime/cert/dev-ca/ca.crt`. Open the dev site in Chromium and an existing Firefox profile.
2. Start `./moq run` and inspect `ss -ltnp`. Expect HTTPS on 8443, cluster on 8444, and loopback SSH on 2222 in an SSH-enabled build. Check that the Moqui process has closed 8080 and 4317.
3. Request `curl --cacert runtime/cert/dev-ca/ca.crt https://localhost:8443/health`. Run `./moq trust-dev-cert --system` and repeat with ordinary curl. Log in through `moqui tui --server https://localhost:8443`, trigger a notification and confirm delivery.
4. Configure an OTel sender with protocol `grpc`, endpoint `https://localhost:8443` and the absolute path to the public root in `OTEL_EXPORTER_OTLP_CERTIFICATE`. Use a configured ingest credential and confirm the exported signals in Monitoring.
5. Stop the first server and start the second checkout. Confirm both public roots match and the two leaf fingerprints differ. Repeat the browser, curl and gRPC checks without another trust operation.
6. Install the runtime, change to an empty directory and start `moqui --installed`. Verify startup reports the installed runtime. Stop it and confirm a checkout still uses its local top-level conf, database, secrets and TLS keys.
7. Run `./moq trust-dev-cert --system --untrust` and `./moq trust-dev-cert --untrust`. Repeat both operations and verify each store reports its result. Check that the browsers and system curl cease trusting the root after restarting any cached connections.
