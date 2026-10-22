# License Review

Reviewed 2026-10-11. The graph is the workspace in this repository, resolved as a Linux `x86_64-unknown-linux-gnu` crate graph, plus the theseus Postgres 18.6.0 archive those plans pin. `moqui-db` was added after that resolve. It depends only on `moqui-lib` and adds no third-party crate. This review reads the published license terms. It is not a legal opinion.

The framework license stays what Moqui Java uses. `LICENSE.md` is CC0 1.0 plus a patent grant adapted from Apache-2.0. CC0 section 4(a) waives copyright and withholds patents, so the grant is part of the license, not a preface. That grant covers Moqui's own code. It does not relicense a dependency, and a dependency's license does not become CC0 because the binary is ours.

## What "linked" means here

GPL-style copyleft treats a static link as one combined work. Rust's default is a static link: every normal crate dependency is an rlib inside the `moqui` binary. A component written in Rust that depends on `moqui-lib`, `moqui-db`, `moqui-core`, or `moqui-host` is in that same binary. That is the case this review is for. A later GPL, AGPL, or SSPL crate anywhere in that graph would cover the customized binary, including the customer's Rust.

Four other relationships are not that link.

- A system library loaded dynamically by the `moqui` binary. On GNU Linux that is glibc (`LGPL-2.1`, the usual system-library case). On musl it is the MIT libc, often static. This is every Rust Linux binary, and it does not relicense the program.
- A child process. The postmaster is started and spoken to over a socket. Chromium is started and spoken to over the DevTools Protocol. The Rust code does not link either one.
- Bytes of that child stored inside the binary and extracted at startup. That is distribution of the child, plus aggregation in one file. It is not a static link of the Rust code. The license of each file in the archive still applies to that file.
- A file the browser loads, and JavaScript, templates, and XML that QuickJS or the screen engine interprets. Those are separate works. Shipping them does not link them into the binary. Their license does not become the binary's license, and the binary's license does not become theirs.

The rule that follows:

| Where it lives | Accept | Keep out |
|---|---|---|
| Static graph of the `moqui` binary | MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Unlicense, Zlib, BSL-1.0, PostgreSQL, Unicode-3.0, CDLA-Permissive-2.0, CC0-1.0, MIT-0, and `AND` combinations of those. `Apache-2.0 WITH LLVM-exception` is an extra grant on top of Apache and is accepted by taking the Apache or MIT alternative. | GPL, AGPL, LGPL, SSPL, EUPL, OSL, MPL, EPL, CDDL, BUSL, Elastic License 2.0, CC-BY-SA, and a proprietary library. |
| Payload we embed and exec | Same permissive set, file by file. | A GPL library inside the archive, even when only a tool we never run is linked to it. |
| Browser file we serve | Permissive, as its own file. LGPL only when the file stays separate and replaceable and the LGPL notice ships with it. A dual license is taken on the permissive side. | GPL and AGPL. A strong-copyleft library bundled into `moqui-html.js` or a qvt bundle. |
| Code we interpret | Whatever license that source file carries. Framework JavaScript we write is CC0. | Nothing about the engine changes. |

Weak copyleft (MPL, EPL, LGPL) does not by itself force a customer's Rust files onto that license. It still does not belong in the default static graph. LGPL static linking wants a relink path that an rlib does not give. MPL and EPL want source of the covered files for as long as we ship them. `pdf-servo` is the one proposed MPL link, and it stays off.

## Result

The default graph is clean. Resolved on 2026-10-11 for Linux gnu, `cargo tree -p moqui` is 387 crate names: the five workspace crates and 382 third-party names. None is GPL, LGPL, AGPL, MPL, EPL, CDDL, or SSPL. A customer can link Rust into a customized Moqui and keep that Rust proprietary or CC0. MIT, BSD, ISC, PostgreSQL, Zlib, Unicode, and CDLA-Permissive still require their notices to travel with the binary. Apache-2.0 still requires its license and any `NOTICE`. `LICENSE.md` remains the framework license. A `THIRD-PARTY-NOTICES` file is how the binary meets the rest.

The one copyleft artifact in the default distribution is `bin/psql` inside the theseus Postgres 18.6.0 archive. It is the only file in that archive whose `NEEDED` entry is `libreadline.so.8` (GPL-3.0-or-later). The postmaster does not link readline. `libreadline` is not in the 18.6.0 archive; `psql` expects the host copy. Moqui never runs `psql`. Strip `psql` out of the archive before `postgresql_embedded` embeds it. Check the next theseus release the same way: the current Linux Dockerfiles pass `--with-readline`, and `scripts/bundle-runtime.sh` copies every non-libc `NEEDED` library into the install. A later archive can start shipping `libreadline.so` itself. The musl and Windows archives were not unpacked. Their build scripts pass the same `--with-readline`.

`mysql` 28's `rustls-tls` feature statically links `ring` 0.17.14 beside aws-lc, through the deprecated `webpki` 0.22 crate. `ring` is Apache-2.0 AND ISC. `webpki`'s crates.io SPDX field is empty because the crate uses `license-file`. The file is an ISC grant, and `third-party/chromium/LICENSE` is BSD-3-Clause. Permissive, and worth a `cargo-deny` clarification so an empty SPDX field does not look like an unknown license. Dropping `ring` waits on a `mysql` release that uses `rustls-webpki`. `russh` does not add another `ring`. Its `ring`, `des`, and `dsa` features are off. `des` 0.9.0 is in the lockfile as an optional dependency of `ssh-cipher` and is not in `cargo tree -p moqui`.

`postgresql_embedded` 0.21 statically links `sqlx` 0.9 (MIT OR Apache-2.0). The entity engine still has no SQLx dependency and no `query!` macros. The lifecycle crate uses `sqlx` for its own setup statements. That is allowed.

Compared with Moqui Java, the in-process copyleft goes away. H2 2.4.240 is MPL-2.0 OR EPL-1.0 and is not carried. The Jakarta APIs on the Java classpath are EPL and are not carried. `getMySqlJdbc` in the Java `build.gradle` downloads MySQL Connector/J into `runtime/lib`. Connector/J is GPL-2.0 with the Universal FOSS Exception. The Rust client is the `mysql` crate, MIT OR Apache-2.0, and it does not link `libmysqlclient` (GPL-2.0 with a FOSS exception) or MariaDB Connector/C (LGPL-2.1). A proprietary component deployed in-process is the case that exception was always awkward for. The Rust static link is stricter in the other direction: one GPL crate would cover the customer's Rust, which the Java classpath would also have done, and which `cargo-deny` is there to prevent.

## Custom Rust, scripts, and templates

A Rust component that calls Moqui is a static link. The licenses in the default graph allow it. The notices above still ship with the combined binary. The customer's own license can be proprietary or CC0. Moqui's patent grant covers Moqui's code. Apache dependencies carry their own patent grants, which end if the licensee brings patent litigation over that dependency. That is a termination clause, not a copyleft clause.

A component that is JavaScript, a template, or XML is not linked. QuickJS-NG is MIT and is compiled into the binary. Running a script on it does not make the script MIT, and a GPL script a customer writes does not make the engine GPL. The same split holds for the browser: Vue and Quasar run in the page, and Moqui's server binary does not link them. This is the room the Java distribution does not have, where component classes are loaded into the same JVM as the framework and its drivers.

Framework JavaScript we write (macro modules, `moqui-html.js`, `MoquiLib`) is CC0, with the patent grant. Third-party browser files keep the license in the browser table.

## Manifest rules

These are the feature flags the resolve used. The unsafe defaults are called out so the first `Cargo.toml` does not inherit them.

| Crate | Set | Leave off |
|---|---|---|
| `postgresql_embedded` 0.21 | `default-features = false`, `blocking`, `theseus`, `tls-rustls-aws-lc-rs`. `POSTGRESQL_VERSION` pinned to 18.6.0. | Default features, which are `tls-native-tls` and `theseus`. `zonky`. `tls-rustls-ring`. |
| `postgres` 0.19 | `with-jiff-0_2` | `with-chrono-0_4`, `with-time-0_3` |
| `mysql` 28 | `default-features = false`, `minimal-rust`, `derive`, `buffer-pool`, `rustls-tls`, `rust_decimal` | `minimal` (system zlib through `flate2`). `native-tls`. `rustls-tls-ring`. `chrono`. `time`. |
| `tokio-postgres-rustls` 0.14 | `aws-lc-rs`, `webpki-roots` | `ring`. Defaults are empty, so the crypto feature has to be named. |
| `reqwest` 0.13 | `default-features = false`, `blocking`, `rustls`, `json`, `multipart`, `charset` | `native-tls`, `native-tls-vendored`. On 0.13, `default-tls` is already rustls with aws-lc. Name `rustls` anyway. A later reqwest that flips `default-tls` back to native-tls must not be taken on defaults. |
| `gix` 0.89 (added 2026-10-18; `merge` 2026-10-21) | `default-features = false`, `sha1`, `parallel` (shared handles are `Arc`, so a repository is `Send + Sync`), `blocking-network-client`, `blocking-http-transport-reqwest-rust-tls`, `merge` (`merge#GitRefs`: gix-merge's three-way tree and blob merge; brings `gix-merge` 0.22 MIT OR Apache-2.0 and `gix-imara-diff` 0.3.1 Apache-2.0, the rest of its graph already linked) | The other defaults (worktree, status, blame and the rest), `blocking-http-transport-curl*` (libcurl), `*-native-tls`. Its zlib is `gix-zlib` on `zlib-rs`; its HTTP is the workspace `reqwest` 0.13 on rustls with aws-lc. No new TLS or C dependency. |
| `similar` 3.2 (added 2026-10-18) | `default-features = false`, `text`, `inline` | `unicode`, `bytes`, `serde`, `wasm32_web_time`. No required dependencies. |
| `yrs` 0.28 (added 2026-10-21) | `default-features = false` | `sync` (its async sync protocol: the host carries the messages itself), `weak`, `small-client`. Brings `arc-swap`, `async-lock`, `dashmap`, `fastrand`, `smallstr`, `smallvec`, `thiserror`, all MIT or Apache-2.0/MIT. |
| `lettre` 0.11 | `default-features = false`, `smtp-transport`, `builder`, `hostname`, `rustls`, `aws-lc-rs`, `webpki-roots` | Defaults, which include `native-tls`. The feature named `rustls-tls` enables `ring`, so that name stays off. `boring-tls`. `tokio1` stays off, so SMTP is blocking. |
| `mail-parser` 0.11 | `default-features = false` | `full_encoding`/`encoding_rs`, `serde`, `rkyv`. |
| `imap` 2.4 | `default-features = false` | `tls`, which is `native-tls` and is the default. 2.4.1 has no rustls feature. `rustls-connector` is a dev-dependency and is not linked. The TCP stream is wrapped with `rustls` in moqui-core. Depends on `chrono` 0.4, `nom` 5, and `imap-proto` 0.10 (MIT OR Apache-2.0). |
| `rustls` 0.23 | `default-features = false`, `std`, `tls12`, `aws_lc_rs` | `ring`. This direct dependency is the IMAP and POP3 stream. `aws_lc_rs` is the crypto provider passed to `ClientConfig`, not a process-global default. POP3 has no crate: the client is in moqui-core on the same stream. Published POP3 clients are OpenSSL, MPL-2.0, LGPL, or Tokio. |
| `rcgen` 0.14 | `default-features = false`, `aws_lc_rs` | `ring`, `pem`, `x509-parser`, `fips`. Defaults include `ring` and `pem`. The pair is a self-signed cluster certificate, base64 DER, linked from moqui-core only. `time` is certificate validity only. Moqui clocks stay `jiff`. aws-lc is the crypto provider. No OpenSSL. |
| `russh` 0.64 | `default-features = false`, `aws-lc-rs`, `flate2`, `rsa` | `ring`, `des`, `dsa`. Defaults are off so a later default cannot turn `ring` on. `rsa` stays because servers still present RSA host keys. `flate2` is SSH compression, on the `miniz_oxide` backend already enabled. |
| `russh-sftp` 3 | no features | `async-trait`. The crate has no crypto feature. |
| `huskarl` 0.11 | default features, which are `default-jws-verifier-platform` | `authorization-flow-loopback`, `metrics`, `experimental-oidc-key-binding`. `huskarl-reqwest`, `huskarl-redis`, and `huskarl-google-cloud` are not dependencies. JOSE is `huskarl-crypto-native` (RustCrypto). TLS stays on `aws-lc-rs`. P-521 is not included. |
| `rust_xlsxwriter` 0.99 | `default-features = false` | `chrono`, `jiff`, `serde`, `polars`, `constant_memory`, `zlib`. Dates are written as Excel serial numbers. Its `zip` dependency asks for `deflate`. |
| `calamine` 0.36 | `default-features = false` | `chrono`/`dates`. Dates are read as Excel serial numbers and made `jiff` values. Its `zip` dependency asks for `deflate`. |
| `zip` 8 (through the two above) | `deflate`, which both crates name: `deflate-zopfli` and `deflate-flate2-zlib-rs` | The other defaults (`aes-crypto`, `bzip2`, `zstd`, `lzma`, `xz`, `ppmd`, `deflate64`, `time`) stay off. `deflate` turns `flate2`'s `zlib-rs` backend on for the whole graph. `zlib-rs` is pure Rust under the Zlib license. `libz-sys` stays out. |
| `image` 0.25 | `default-features = false`, `bmp`, `gif`, `jpeg`, `png`, `tiff`, `webp` | `rayon`, `avif` (and `dav1d`), `exr`, `dds`, `ff`, `hdr`, `ico`, `pnm`, `qoi`, `tga`, `serde`. JPEG decoding is `zune-jpeg`. |
| `ipp` 7 | `default-features = false` | `async-client-rustls` (the default), `async-client`, `client`, and every TLS feature. The message encoding is used without the crate's clients; the host posts it with the workspace `reqwest`. |
| `axum` 0.8 | `macros`, `ws` | |
| `tower-http` 0.6 | `default-features = false`, `compression-gzip`, `fs`, `set-header` | |
| `tokio-tungstenite` 0.29 | `default-features = false`, `connect` | TLS features. The PDF browser's DevTools socket is loopback `ws://`. Already in the graph under axum's `ws`. `chromiumoxide` was dropped (phase 12): its untagged-enum message parsing fails under `serde_json` `arbitrary_precision`. |
| `p256` 0.14 | `default-features = false`, `ecdsa`, `pkcs8`, `pem`, `std` | Apple's SSO client secret (an ES256 JWT). The same `p256` `huskarl-crypto-native` and `russh` link. |
| `argon2` 0.6 | `password-hash` | |
| `croner` 4 | `default-features = false`, `jiff` | The default `chrono` feature. |
| `totp-rs` 6 | `default-features = false`, `std`, `otpauth`, `gen_secret` | `qr` |
| `rquickjs` 0.14 | defaults | |
| `rquickjs` 0.14 on `wasm32-unknown-emscripten` | defaults plus `bindgen` (moqui-core target dependency) | No bindings ship for this target, so `rquickjs-sys` generates them at build with `bindgen` (BSD-3-Clause, build only, not linked) against the emsdk sysroot. |
| `rusqlite` 0.38 | `bundled` (moqui-browser only) | `0.40`: `servo-net` (behind `pdf-servo`) pins 0.38, and two `libsqlite3-sys` versions cannot both link `sqlite3`. No `bundled-sqlcipher`, `loadable_extension`, or `serde_json` feature. |
| `pulldown-cmark` | the Rust crate | `comrak` is BSD-2-Clause and is an acceptable alternate. Re-check it if a C regex backend is turned on. |
| `pdf-servo` | absent from the default graph | The `servo` crate 0.7.0 is MPL-2.0. See below. |

`oxc_parser` 0.139.0 is named in `[workspace.dependencies]` and is not a dependency of any crate. It is absent from `cargo tree -p moqui` until phase 6 links it. The 387-name count above does not include it. Its published `rust-version` is 1.94.0. Its license is MIT.

`native-tls` and the `openssl` crate are not in the resolved graph. `openssl-probe` is. It only finds a certificate directory. It does not link libssl. `libz-sys` is not in the graph: `mysql` is on `minimal-rust`, and `flate2` 1.1 is on `rust_backend`. Since phase 12 the `zip` `deflate` feature also turns on `flate2`'s `zlib-rs`, a pure-Rust port of zlib under the Zlib license, which then backs every `flate2` user.

## Servo, Chromium, OpenSearch

`pdf-servo` stays off, and it is a license hold as well as a print-parity hold. Servo is MPL-2.0, file by file. MPL would not relicense the rest of the binary or the customer's Rust. It would require source of the MPL files for as long as that feature ships (a pointer at the upstream revision is enough while we do not patch them). Servo's own book says the bundled FreeType is dual-licensed and the embedder chooses the FreeType License or the GPL. The choice is the FreeType License. The GPL alternative is not available for this build. Turn the feature on only after `cargo-deny` on that feature shows no GPL or AGPL, and any LGPL library is a dynamic system library the user can replace. Until that report exists, the default binary drives an external browser over the DevTools socket.

Chromium is not vendored and not downloaded. No browser fetcher is linked. The operator installs Chromium. The browser project is BSD-3-Clause and carries a large third-party notice of its own, which belongs to whoever ships that binary. Moqui does not. Google Chrome remains a developer substitute and is not the deployment binary. A packager who later vendors Chromium picks up Chromium's notices, including LGPL pieces such as some media libraries, for that separate process. Those terms do not flow back into the Rust link.

OpenSearch is a separate process, Apache-2.0, and it is not a crate in this graph. Elasticsearch is not a substitute. Current Elasticsearch server licenses (Elastic License 2.0, and SSPL on older lines) are not accepted, including as a child process we would download or embed.

## Later drivers and engines

A sync `DbDriver` added for SQL Server, Oracle, DB2, Derby, or HSQLDB is reviewed before it is linked. unixODBC is LGPL-2.1. FreeTDS is LGPL. The Oracle client libraries are proprietary. `libmysqlclient` and MariaDB Connector/C stay out. SQLite's public-domain license would be acceptable. SQLite is not a target.

V8, if it ever replaces QuickJS, is BSD-3-Clause for the engine. That swap gets its own pass over V8's third-party tree before it is linked. It is not in this graph.

Bitronix is not linked. The Java fork `org.moqui:btm` is annotated Apache-2.0 in the framework `build.gradle`. Upstream Bitronix was LGPL. The transaction manager is a behavior reimplementation against `DbDriver`. It is not a translation of LGPL source.

An inbound SMTP server is not in this graph. It is reviewed when it is proposed. The tool components are in the framework (`plans/archive/Office-Image-Print.md`): `rust_xlsxwriter` and `calamine` (with `zip`) for `moqui-poi`, `image` for `moqui-image`, and `ipp` for `moqui-cups`. cups4j is LGPL-3.0 and libcups is C; neither is linked. The client speaks IPP to the CUPS server over HTTP. `russh` 0.64.1 and `russh-sftp` 3.0.1 are linked from `moqui-host`; the SFTP client, the SSH server, and the SSH cluster channel are implemented (`plans/archive/Ssh.md`). `libssh` stays out. It is LGPL. `ssh2` and `libssh2` stay out. They are a C client with no server. `remotefs-ssh` is reviewed only if it is proposed. `huskarl` 0.11.6 is linked from `moqui-host` and runs the SSO login (`plans/archive/Sso.md`). SAML is not in the graph. `samael` and `openidconnect` stay out. `saml-rs` is reviewed when it is proposed.

## Direct crates

Versions are what `cargo` locked on 2026-10-11 for the feature set above. "Static" means compiled into the `moqui` binary.

| Crate | Version | License | Role | Linkage | Verdict |
|---|---|---|---|---|---|
| `rquickjs` | 0.14.0 | MIT | Script and template engine | Static. The C engine is QuickJS-NG, also MIT, compiled in through `rquickjs-sys`. | OK. Retain the MIT notice. |
| `postgresql_embedded` | 0.21.0 | (Apache-2.0 OR MIT) AND PostgreSQL | Start and stop the postmaster | Static for the crate. The server archive is a payload. | OK. Strip `psql` from the payload. Keep `COPYRIGHT`. |
| `postgresql_archive`, `postgresql_commands` | 0.21.0 | (Apache-2.0 OR MIT) AND PostgreSQL | Download, bundle, and invoke the archive | Static | OK, same payload rule. |
| `sqlx`, `sqlx-core`, `sqlx-postgres` | 0.9.0 | MIT OR Apache-2.0 | Used inside `postgresql_embedded` | Static | OK. Not the entity SQL path. |
| `postgres`, `tokio-postgres`, `postgres-types`, `postgres-protocol` | 0.19.14 / 0.7.18 | MIT OR Apache-2.0 | Sync Postgres client. Pure Rust, no libpq. | Static | OK |
| `tokio-postgres-rustls` | 0.14.0 | MIT | External Postgres TLS | Static | OK with `aws-lc-rs` |
| `mysql`, `mysql_common` | 28.0.3 / 0.37.3 | MIT OR Apache-2.0 | MySQL and MariaDB client. Pure Rust, no libmysqlclient. | Static | OK. Pulls `ring` via `webpki`. Permissive. |
| `axum`, `tower`, `tower-http` | 0.8.9 / 0.5.3 / 0.6.11 | MIT | HTTP server, WebSocket, compression, ETag | Static. `ws` pulls `tokio-tungstenite` 0.29 (MIT) and `tungstenite` (MIT OR Apache-2.0). | OK |
| `tokio`, `hyper` | 1.x / 1.x | MIT | Runtimes inside the host (axum, and the edge runtime for PDF, SSH and SSO) and the blocking clients | Static | OK. Core stays Tokio-free; the crates still land in the binary through host and the clients. |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | Blocking HTTP client | Static | OK on the `rustls` feature |
| `lettre` | 0.11.23 | MIT | Outbound SMTP | Static | OK with the feature row above |
| `imap` | 2.4.1 | Apache-2.0 OR MIT | Blocking IMAP client. POP3 is in-tree, not a crate. | Static | OK with `default-features = false`. The `tls` feature is `native-tls` and stays off. 2.4.1 has no rustls feature. No `ring` from this crate. |
| `mail-parser` | 0.11.9 | Apache-2.0 OR MIT | Inbound MIME for `poll#EmailServer`: nested multiparts, RFC 2047 words, charsets, folded headers | Static | OK with `default-features = false`. Its one dependency is `hashify` 0.2 (Apache-2.0 OR MIT), a proc-macro that is not linked. `full_encoding` (`encoding_rs`, multi-byte CJK charsets) stays off; the single-byte charsets are built in. |
| `russh` | 0.64.1 | Apache-2.0 | SSH client and server: SFTP client, the built-in server when `ssh_port` is set, the SSH cluster channel | Static | OK with `aws-lc-rs`, `flate2`, and `rsa`. `ring`, `des`, and `dsa` off. Depends on `rsa` 0.10.0-rc.18 and `ssh-key` 0.7.0-rc.11, both MIT OR Apache-2.0. |
| `russh-sftp` | 3.0.1 | Apache-2.0 | SFTP v3. Does not depend on `russh`. | Static | OK. `async-trait` off. |
| `huskarl` | 0.11.6 | MIT OR Apache-2.0 | OIDC and OAuth 2 relying party (`moqui-sso` in the framework) | Static | OK. Default feature is the native JWS verifier. `rsa` 0.10.0-rc.18, shared with `russh`. `huskarl-crypto-webcrypto` is locked for wasm and is absent from the Linux tree. |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | SFTP timestamps via `russh-sftp`, and IMAP `INTERNALDATE` via `imap` | Static | OK. Not a Moqui clock. Time stays `jiff`. |
| `tokio-tungstenite` | 0.29.0 | MIT | DevTools socket for PDF | Static. Does not contain Chromium. | OK. Already linked under axum `ws`. |
| `futures-util`, `http`, `bytes`, `form_urlencoded` | 0.3 / 1.x / 1.x / 1.x | MIT OR Apache-2.0 | Async streams (PDF, SSH cluster), `huskarl`'s request types, the SSO callback query | Static | OK. Already in the graph; named directly in `moqui-host`. |
| `encoding_rs` | 0.8.42 | (Apache-2.0 OR MIT) AND BSD-3-Clause | SFTP charsets (`remoteCharset`) in `moqui-core` | Static | OK. Already in the graph under reqwest. The BSD part is the WHATWG index data. |
| `rustls` | 0.23.45 | Apache-2.0 OR ISC OR MIT | TLS | Static | OK |
| `rcgen` | 0.14.10 | MIT OR Apache-2.0 | Self-signed cluster certificate | Static, moqui-core only | OK with `aws_lc_rs`. `ring`, `pem`, `x509-parser`, and `fips` stay off. `x509-parser` is an optional dependency and is not linked. |
| `time` | 0.3.55 | MIT OR Apache-2.0 | `not_before` and `not_after` for that certificate. `rcgen` turns its default features off. | Static | OK. Not a Moqui clock. Clocks stay `jiff`. Pulls `deranged` 0.5.8, `num-conv` 0.2.2, `powerfmt` 0.2.1, and `time-core` 0.1.9, all MIT OR Apache-2.0. |
| `yasna` | 0.6.0 | MIT OR Apache-2.0 | DER encoding used by `rcgen` | Static | OK |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | Certificate DER type used by `rcgen` and `rustls` | Static | OK |
| `aws-lc-rs` | 1.18.1 | ISC AND (Apache-2.0 OR ISC) | TLS crypto | Static | OK |
| `aws-lc-sys` | 0.45.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | aws-lc C sources | Static C, compiled at build | OK. The `AND` means every one of those notice obligations applies. None of them is copyleft. |
| `ring` | 0.17.14 | Apache-2.0 AND ISC | Pulled only by `mysql` → `webpki` 0.22 | Static | OK. Not the chosen TLS provider. |
| `webpki` | 0.22.4 | ISC, plus BSD-3-Clause for `third-party/chromium` | Old certificate verifier used by `mysql` | Static | OK. SPDX field is empty; clarify in `cargo-deny`. |
| `webpki-roots` | 1.0.9 | CDLA-Permissive-2.0 | CA bundle data | Static data | OK. CDLA-Permissive is not the share-alike CDLA. |
| `rustls-platform-verifier` | 0.7.1 | MIT OR Apache-2.0 | OS trust store, via reqwest | Static | OK |
| `jiff` | 0.2.38 | Unlicense OR MIT | Timestamps and IANA zones | Static. The bundled tzdb is public domain. | OK |
| `croner` | 4.0.1 | MIT | Quartz cron | Static | OK |
| `rust_decimal` | 1.x | MIT | Decimals | Static | OK |
| `argon2`, `password-hash` | 0.6.0 / 0.6.1 | MIT OR Apache-2.0 | Password hashes | Static | OK |
| `hmac`, `sha1`, `sha2`, `blake2`, `digest` | RustCrypto | MIT OR Apache-2.0 | Signer and TOTP | Static | OK. Further signer crates stay in RustCrypto or aws-lc. |
| `rsa` | 0.10.0-rc.18, RustCrypto | MIT OR Apache-2.0 | `SimpleSigner` in `moqui-lib` (`SHA1withRSA` for QZ Tray) | Static | OK. It is the same `rsa` that `russh` and `huskarl` already resolve, so the graph has one. A direct `sha1` 0.11 (MIT OR Apache-2.0, renamed `sha1-digest-011`) feeds it, because `rsa` is on `digest` 0.11 and the other `sha1` is 0.10. RUSTSEC-2023-0071 (Marvin timing in private-key operations) is already ignored in `deny.toml`. The signer uses the randomized signing path, which blinds the private-key operation, and it signs print job strings that the browser already holds. The ignore stays until a patched `rsa` is released. |
| `totp-rs` | 6.0.0 | MIT | TOTP | Static | OK with `qr` off |
| `serde`, `serde_json` | 1.x | MIT OR Apache-2.0 | JSON | Static | OK |
| `csv`, `csv-core` | 1.4.0 / 0.1.13 | Unlicense OR MIT | CSV | Static | OK |
| `pulldown-cmark` | 0.13.4 | MIT | Markdown | Static | OK |
| `tracing` | 0.1.44 | MIT | Log and metrics instrumentation | Static | OK |
| `indexmap` | 2.x | Apache-2.0 OR MIT | Map used in place of `LiteStringMap` | Static | OK |
| `regex` | 1.x | MIT OR Apache-2.0 | Transitive | Static | OK |
| `flate2`, `miniz_oxide`, `zlib-rs` | 1.1.10 / 0.9.1 / 0.6.8 | MIT OR Apache-2.0 / MIT OR Zlib OR Apache-2.0 / Zlib | Compression. Pure Rust. `flate2` is on `rust_backend`, and `zlib-rs` since `zip` `deflate` (phase 12). | Static | OK. `libz-sys` is not enabled. |
| `ammonia` | 4.2.3 | MIT OR Apache-2.0 | HTML cleaning for `allow-html="safe"`, with its default allow-list | Static. Pulls `html5ever` 0.40 and `markup5ever` (MIT OR Apache-2.0), `url`, `maplit`, `tendril`, `string_cache`, `phf`. Also pulls `cssparser` 0.38 and `dtoa-short` 0.3.5, both **MPL-2.0**. | OK with a narrow exception. MPL-2.0 is file-level copyleft: we do not modify those crates, so the duty is to keep their license notice and offer their source, which the crates.io release is. `deny.toml` allows MPL-2.0 for those two crates by name and no other. |
| `rust_xlsxwriter` | 0.99.1 | MIT OR Apache-2.0 | The `xlsx` render mode (`moqui-poi`) | Static | OK with `default-features = false`. |
| `calamine` | 0.36.1 | MIT | `read#Spreadsheet` and `load#SpreadsheetEntity`: xlsx, xlsm, xlsb, xls, ods | Static. Pulls `quick-xml` 0.41 (MIT), `codepage` (Apache-2.0 OR MIT), `atoi_simd`, `fast-float2`, `debug_unsafe` (MIT OR Apache-2.0), and `encoding_rs`. | OK with `default-features = false`. |
| `zip` | 8.6.0 | MIT | The xlsx container, read and written | Static. `deflate` pulls `zopfli` 0.8 (Apache-2.0), `flate2` with `zlib-rs` 0.6 (Zlib), and `typed-path` (MIT OR Apache-2.0). | OK |
| `image` | 0.25.10 | MIT OR Apache-2.0 | `ImageServices` (`moqui-image`) | Static. Pulls `png`, `gif`, `image-webp`, `fdeflate`, `weezl`, `half` (MIT OR Apache-2.0), `tiff`, `fax`, `color_quant` (MIT), `zune-core` and `zune-jpeg` (MIT OR Apache-2.0 OR Zlib), `moxcms` and `pxfm` (BSD-3-Clause OR Apache-2.0), `bytemuck` (Zlib OR Apache-2.0 OR MIT), `byteorder-lite` (Unlicense OR MIT). | OK. Take Apache-2.0 for `moxcms` and `pxfm`. |
| `ipp` | 7.0.0 | MIT OR Apache-2.0 | IPP messages for network printing (`moqui-cups`) | Static, moqui-host. Pulls `enum-as-inner` and `enum-primitive-derive` (proc-macros, not linked), `num-traits`, `thiserror`, `bytes`, `http`. | OK with `default-features = false`. |
| `gix` and its `gix-*` crates | 0.89.0 (`gix-object` 0.66, `gix-odb` 0.86, `gix-transport` 0.61, `gix-merge` 0.22, ...; `gix-imara-diff` 0.3.1 is Apache-2.0) | MIT OR Apache-2.0 | `git://` resources (`moqui-core` feature `git`, `docs/Revision-Management.md`) | Static, not in the browser build. Pulls `bstr`, `heapless`, `hash32`, `defmt` (MIT OR Apache-2.0), `clru`, `faster-hex`, `nonempty`, `prodash` (MIT), `unicode-bom` (Apache-2.0). | OK. `cargo deny check` bans and licenses pass; `libgit2`/`git2` is not used (GPL-2.0 with a linking exception, and OpenSSL) |
| `similar` | 3.2.0 | Apache-2.0 | Line diffs of resource versions | Static, also in the browser build | OK |
| `yrs` | 0.28.0 | MIT | The Yjs CRDT of co-edited documents (`moqui-core` `collab`), wire-compatible with the browser's Yjs | Static | OK. Retain the MIT notice. |
| `servo` | 0.7.0 | MPL-2.0 | In-process PDF alternate | Would be static | Hold. Off by default. |
| `rusqlite` | 0.38.0 | MIT | SQLite `DbDriver` of the browser build (`crates/moqui-browser`, `plans/archive/Wasm.md`) | Static in the emscripten module. Natively it is only in `moqui-browser` tests and stub; the `moqui` binary does not link it. Pulls `hashlink` 0.11, `fallible-iterator` 0.3, `fallible-streaming-iterator` 0.1 (MIT OR Apache-2.0 / MIT). | OK |
| `libsqlite3-sys` | 0.36.0 | MIT; the bundled SQLite amalgamation is public domain | Compiles SQLite with the target C compiler (emcc for the browser) | Static, same scope as `rusqlite` | OK. Public domain needs no notice; keep the MIT notice for the crate. |
| `revue` | 3.10.1 | MIT | Terminal widgets for `crates/moqui-tui` | Not in the `moqui` binary. `moqui-tui` only. Pin `=3.10.1`, `default-features = false`, feature `markdown`. Rust 1.95. `async` and `config` stay off, so `tokio` and `reqwest` are absent from `cargo tree -p moqui-tui`. Pulls `pulldown-cmark` 0.13.4 (already in this table) and `anyhow` 1.0.104, `bitflags` 2.13.2, `thiserror` 2.0.21, `libc` 0.2.190, which the `moqui` graph already has. | OK. Added 2026-10-20. `cargo deny check` on this lock: licenses, bans, and sources passed. Advisories failed on `ammonia` 4.2.3 (RUSTSEC-2026-0373), `rustls` 0.23.45 (RUSTSEC-2026-0346, 0347, 0348, 0349), and `zip` 8.6.0 (RUSTSEC-2026-0367, 0368, 0369). Those three are already in the `moqui` graph and stay at the locked versions. |
| `revue-macros` | 3.10.2 | MIT | Proc-macro for Revue. The `revue` pin `=3.10.1` resolved this crate to the later 3.10.2 patch. | Compile time. Not linked. Absent from `cargo tree -p moqui`. | OK |
| `crossterm` | 0.29.0 | MIT | Terminal backend Revue uses | Absent from `cargo tree -p moqui`. Transitive through `revue`. | OK |
| `unic-emoji-char` | 0.9.0 | MIT/Apache-2.0 | Emoji width data for Revue's width table | Absent from `cargo tree -p moqui`. Transitive through `revue`. | OK. Take MIT or Apache-2.0. |
| `signal-hook` | 0.4.5 and 0.3.18 | MIT OR Apache-2.0. The 0.3.18 SPDX field is spelled Apache-2.0/MIT. | Signal listeners inside Revue (0.4.5) and crossterm (0.3.18) | Absent from `cargo tree -p moqui`. Two versions, which `deny.toml` allows. | OK |
| `textwrap` | 0.16.4 | MIT | Line wrapping. Already linked through `oxc_parser` 0.139 (`oxc-miette`). Revue pulls the same version. | Static in the `moqui` binary. Also in `moqui-tui`. | OK |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | Display width. Already linked through `textwrap` / `oxc-miette`. Revue pulls the same version. `moqui-tui` names `0.2` as a dev-dependency for the width comparison. | Static in the `moqui` binary. | OK |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | Grapheme breaks. Already linked through `rquickjs` (`convert_case`) and `oxc-miette`. Revue pulls the same version. | Static in the `moqui` binary. | OK |

`cc`, `cmake`, `pkg-config`, and `find-msvc-tools` run at build to compile aws-lc and are not linked into the binary. Proc-macro crates (`serde_derive`, `sqlx-macros`, `tokio-macros`, and the rest) run at compile time and are not linked either. The code they expand is.

## Other licenses in the static graph

324 of the 382 third-party names are MIT, Apache-2.0, or MIT OR Apache-2.0, including the `MIT/Apache-2.0` and `Apache-2.0 / MIT` spellings. The five workspace crates are CC0-1.0. The other 58 third-party names are below. Those counts are from before the phase 12 tool components. The 37 names those components added (`cargo tree -p moqui`, against the phase 12 SSO commit) are in the direct crates rows above, and the ones that are not MIT or Apache-2.0 are marked phase 12 below. All of them are permissive. Unicode-3.0 is the current Unicode license (attribution, no share-alike). The ICU data crates carry it, pulled in by URL and text handling. `idna` itself is MIT OR Apache-2.0.

| License | Crates |
|---|---|
| Unicode-3.0 | `icu_collections`, `icu_locale_core`, `icu_normalizer`, `icu_normalizer_data`, `icu_properties`, `icu_properties_data`, `icu_provider`, `litemap`, `potential_utf`, `tinystr`, `writeable`, `yoke`, `yoke-derive`, `zerofrom`, `zerofrom-derive`, `zerotrie`, `zerovec`, `zerovec-derive` |
| (MIT OR Apache-2.0) AND Unicode-3.0 | `unicode-ident` |
| Unlicense OR MIT | `aho-corasick`, `byteorder`, `jiff`, `jiff-core`, `memchr`, `termcolor` |
| Unlicense OR MIT (`csv` spelling) | `csv`, `csv-core` |
| Apache-2.0 OR ISC OR MIT | `hyper-rustls`, `rustls`, `rustls-native-certs`, `rustls-pemfile` |
| (Apache-2.0 OR MIT) AND PostgreSQL | `postgresql_embedded`, `postgresql_archive`, `postgresql_commands` |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | `linux-raw-sys`, `rustix`. Take Apache-2.0 or MIT. |
| ISC | `rustls-webpki`, `untrusted` |
| Zlib | `zlib-rs` (phase 12, through `zip`) |
| Zlib OR Apache-2.0 OR MIT, MIT OR Apache-2.0 OR Zlib | `bytemuck`, `zune-core`, `zune-jpeg` (phase 12, through `image`) |
| BSD-3-Clause OR Apache-2.0 | `moxcms`, `pxfm` (phase 12, through `image`). Take Apache-2.0. |
| Unlicense OR MIT | `byteorder-lite` (phase 12, through `image`) |
| ISC AND (Apache-2.0 OR ISC) | `aws-lc-rs` |
| ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | `aws-lc-sys` |
| Apache-2.0 AND ISC | `ring` |
| BSD-3-Clause | `curve25519-dalek`, `ed25519-dalek`, `subtle` |
| MIT AND BSD-3-Clause | `matchit` |
| (Apache-2.0 OR MIT) AND BSD-3-Clause | `encoding_rs` |
| BSD-2-Clause OR Apache-2.0 OR MIT | `zerocopy` |
| Zlib | `foldhash` |
| Zlib OR Apache-2.0 OR MIT | `tinyvec` |
| MIT OR Zlib OR Apache-2.0 | `miniz_oxide` |
| 0BSD | `quoted_printable` |
| 0BSD OR MIT OR Apache-2.0 | `adler2` |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | `constant_time_eq`, `dunce` |
| Apache-2.0 OR BSL-1.0 | `ryu` |
| Apache-2.0 OR BSL-1.0 OR MIT | `whoami` |
| CDLA-Permissive-2.0 | `webpki-roots` |
| ISC plus BSD-3-Clause, SPDX empty | `webpki` 0.22.4 |

## Embedded Postgres payload

Archive inspected: `postgresql-18.6.0-x86_64-unknown-linux-gnu.tar.gz` from theseus-rs/postgresql-binaries, 12 MB compressed. `readelf -d` on every ELF under `bin/` and `lib/`. The archive contains `COPYRIGHT` (the PostgreSQL license) and theseus's own short permissive `LICENSE`. It does not contain `libreadline`, `libssl`, `libcrypto`, `libxml2`, `libxslt`, `libzstd`, `liblz4`, `libz`, or `libgssapi_krb5`. The postmaster's `RUNPATH` is `$ORIGIN/../lib`, then the system path.

| File | Linked libraries | License of that code | What Moqui does |
|---|---|---|---|
| `bin/postgres` | `libzstd`, `liblz4`, `libxml2`, `libssl`, `libcrypto`, `libgssapi_krb5`, `libz`, `libm`, `libc` | Server: PostgreSQL license. zstd: BSD-3-Clause (upstream is BSD-3-Clause OR GPL-2.0; use BSD-3-Clause). lz4: BSD-2-Clause. libxml2: MIT. OpenSSL 3: Apache-2.0. MIT Kerberos: the MIT license. zlib: Zlib. glibc: LGPL-2.1 as the system libc. | Exec this. It does not link readline. |
| `lib/pg_trgm.so`, `lib/plpgsql.so` | `libc` only | PostgreSQL license | Loaded. `pg_trgm` is the search extension. plpgsql is already in the server. |
| `bin/psql` | `libpq`, `libreadline.so.8`, `libm`, `libc` | `psql` is PostgreSQL code combined with GPL-3.0-or-later readline | In the archive. Never run. Strip it before bundling. |
| `lib/plpython3.so` and the three `*_plpython3.so` | `libpython3.11.so.1.0`, `libc`. `RUNPATH` is `/usr/lib/x86_64-linux-gnu`. | PostgreSQL license, dynamic dependency on CPython (PSF-2.0) when the extension is loaded | Present. Not loaded. Python is permissive. A slim archive can drop them. |
| `lib/pgxml.so` | `libxslt`, `libxml2`, `libc` | PostgreSQL license. libxslt and libxml2 are MIT. | Loaded only if that extension is created. |
| `lib/uuid-ossp.so` | `libossp-uuid` | PostgreSQL license plus the OSSP uuid BSD-style license | Loaded only if that extension is created. |
| `lib/pgcrypto.so` | `libcrypto`, `libz`, `libc` | PostgreSQL license, OpenSSL Apache-2.0 when loaded | Not required by the framework. |

The postmaster's dynamic libraries are the host's copies in this 18.6.0 archive. A theseus release that runs `bundle-runtime.sh` will copy those permissive libraries into the archive and will also copy `libreadline` while `psql` remains. The strip, or a theseus build with `--without-readline`, is what keeps the payload clean across that change. Notices for OpenSSL, ICU (the build enables ICU; the 18.6.0 gnu postmaster's `NEEDED` list did not include `libicu`), Kerberos, and zlib travel with the archive once those `.so` files are inside it. Theseus already has `collect-notices.sh` for that.

Full-text dictionaries and `pg_trgm` ship as part of the PostgreSQL license. No extra license.

## Browser files

These are served as their own files from `runtime/base-component/webroot/`. They are not compiled into the binary. A dual license is taken on the permissive side, and the notices file says so.

| File | License | How it is delivered | Verdict |
|---|---|---|---|
| Vue 3.5.44 (`vue.global.prod.js`) | MIT | Script tag. Server-merged `qvt`, and a server-static shell that can load a merged fragment. | OK |
| Vue 3.5.44 runtime (`vue.runtime.global.prod.js`) | MIT | Script tag. A page whose qvt output, including fragments, is entirely server-static. Same package as the row above. | OK |
| `@vue/compiler-dom` 3.5.44, Node build, bundled for QuickJS (`vue/compiler-dom.cjs.js`) | MIT, with `@babel/parser` (MIT), `entities` (BSD-2-Clause), `estree-walker` (MIT) and `@vue/*` (MIT) inside the bundle | Read by the server and run in QuickJS. Not sent to browsers. Built by `libs/refresh.mjs` with esbuild 0.28.2 (MIT, build tool only). | OK. Their notices are in `libs/NOTICES.md`. |
| Quasar 2.35.1 (`quasar.umd.prod.js`), including `QEditor` | MIT | Script tag | OK. This is the replacement for CKEditor. |
| Font Awesome 4.7.0 (npm `font-awesome`) | Font: SIL OFL-1.1. CSS: MIT. Documentation: CC-BY-3.0. | The font and the CSS. Do not ship the docs. | OK. OFL is a font license, not a software copyleft. Stay on 4.7. Font Awesome 5+ changes the `fa fa-*` classes, and the SVG/JS icons are CC-BY-4.0. |
| Material Icons (Google) 1.13.14 (npm `material-icons`, filled style only) | Apache-2.0 | Font and CSS | OK. Pictogrammers' Material Design Icons, if substituted, are Apache-2.0 for the icons and MIT for the code. |
| Material Symbols Outlined (Google) 0.47.6 (npm `@material-symbols/font-400`, the outlined woff2 and `outlined.css` only; added 2026-10-18) | Apache-2.0 | Font and CSS. Quasar's `material-symbols-outlined` icon set (`quasar/icon-set/`, MIT, same package as Quasar) names its icons | OK. Same license and publisher as Material Icons. The npm package repackages Google's font (`google/material-design-icons`, Apache-2.0) and carries the Apache-2.0 text as its `LICENSE`, which `libs/NOTICES.md` prints. |
| Chart.js 4.5.1 | MIT | Lazy script | OK |
| Mermaid 11.17.2, `dist/mermaid.min.js` | MIT for the package | Lazy script | OK for the MIT package. Checked on the pinned build: it does not embed elkjs (the `elk` strings are layout option names). The bundle carries DOMPurify (Apache-2.0 OR MPL-2.0) and the other npm dependencies of mermaid (`d3` ISC and BSD-3-Clause, `cytoscape`, `dayjs`, `katex`, `roughjs`, `stylis`, `uuid` and the rest MIT). `mermaid.init` is gone in 11, use `mermaid.run`. Original note: elkjs is EPL-2.0. If a build needs it, that file stays separate and its source notice ships. It still does not enter the Rust link. |
| Marked 18.1.1 | MIT | Assist panel. Minified by esbuild from `lib/marked.umd.js`, banner kept. | OK |
| DOMPurify 3.4.17 (`purify.min.js`) | MPL-2.0 OR Apache-2.0 | Assist panel, its own file | OK under Apache-2.0. Election recorded: we take Apache-2.0, and `libs/NOTICES.md` carries both texts. |
| highlight.js 11.12.0 | BSD-3-Clause | Assist panel and `m-code` (the `code-view` element). Core plus bash, css, diff, groovy, java, javascript, json, kotlin, markdown, plaintext, python, rust, sql, typescript, xml, yaml, bundled by esbuild, and two themes. Kotlin added Oct 21, 2026 for code-view. | OK |
| Tiptap 3.31.4 (`tiptap/tiptap.min.js`, added 2026-10-19; collaboration added 2026-10-21): `@tiptap/core`, `starter-kit` and the extensions it includes, `extension-link`, `extension-image`, `extension-mention`, `extension-placeholder`, `extension-table`, `extension-table-row`, `extension-table-cell`, `extension-table-header`, `extension-collaboration`, `extension-collaboration-caret`, `extensions`, `suggestion` and `pm`, with the `prosemirror-*` packages (including `prosemirror-tables`), `@tiptap/y-tiptap` 3.0.9 (Tiptap's y-prosemirror, which the collaboration extensions import), `yjs` 13.6.33, `y-protocols` 1.0.7, `lib0` 0.2.119, `isomorphic.js` 0.2.5, `@floating-ui/*`, `linkifyjs`, `orderedmap`, `rope-sequence` and `w3c-keyname` inside the bundle | MIT, every package (checked on the pinned tree; Tiptap's paid Pro extensions and its cloud collaboration (Hocuspocus, Tiptap Cloud) are not dependencies: the Yjs updates go over the framework's own notification socket) | Lazy script, loaded only when an `m-rich-text` or `m-coedit` editor mounts. Bundled by `libs/refresh.mjs` with esbuild 0.28.2 (an IIFE exposing the global `Tiptap`), every version pinned and published two weeks or more before it was pinned. 587 KB, 187 KB gzipped (462 KB and 147 KB before Yjs). | OK. Each license text is in `libs/NOTICES.md` once, with the packages it covers. |
| OpenUI Lang core 0.2.18 (npm `@openuidev/lang-core`, `openui/lang-core.min.js`, added 2026-10-22), with the parts of `zod` 4.6.5 it uses inside the bundle | MIT, both | The Assist canvas, lazy (loaded with `js/WebrootAssistCanvas.js` on the first canvas); the server runs the same file in QuickJS to parse `write_ui` programs. Bundled by `libs/refresh.mjs` with esbuild 0.28.2 (an IIFE exposing `OpenUILang`) after two reviewed patches of `libs/patches/` (telemetry off: no PostHog call and no `ci-info` import; a failed Query is not fetched again until its arguments change), 42 KB. | OK. Both license texts are in `libs/NOTICES.md`. |
| Tom Select 2.6.2 (`tom-select.complete.min.js`) | Apache-2.0 | html mode, lazy | OK |
| SortableJS 1.15.7 | MIT | html column picker, lazy | OK |
| xterm.js 6.0.0, @xterm/addon-fit 0.11.0 | MIT | the browser terminal (Terminal screen), lazy | OK |
| PDF.js 6.4.299 (npm `pdfjs-dist`, `build/pdf.min.mjs` and `build/pdf.worker.min.mjs` only; added 2026-10-21) | Apache-2.0 | Read by the server and imported by the PDF engine's Chromium page to preview a PDF in Files (its first page and its text). Not sent to browsers by any screen, though served under `/libs/pdfjs/` as every vendored file is. Pinned 18 days after its release; has the CVE-2024-4367 fix (4.2.67 and later). | OK. Not shipped: `standard_fonts` (the Foxit fonts are under PDFium's BSD license, but the Liberation fonts are GPL-2.0 with a font exception, which the deny list below refuses), `cmaps` (Adobe, BSD-3-Clause), `iccs` (CC0) and `wasm` (OpenJPEG BSD-2-Clause, PDFium's JBIG2 BSD, qcms MIT). So the standard 14 fonts come from the system, and a JPEG 2000 or JBIG2 image and a CJK font without its cmap are not drawn. Shipping the Foxit fonts, cmaps or wasm needs a row of its own. |
| Mustache.js | MIT | Not used. The fancy renderer is an in-tree compiler of `if`, `list`, and `assign` to one cached JavaScript function. | Not a dependency. |
| QZ Tray `qz-tray.js` 2.3.0 (npm `qz-tray`, the file header says `@license LGPL-2.1-only`) | LGPL-2.1-only | Separate script, only when printing is enabled (user preference `qz.print.enabled`). Served unmodified from `/libs/qz-tray/qz-tray.js`, never minified, concatenated, or edited, so a customer can replace it. The LGPL text is `libs/licenses/LGPL-2.1.txt` and `libs/NOTICES.md` links the source (`https://github.com/qzind/tray`, tag v2.3.0). Flagged: the only copyleft file in `libs/`. | Acceptable on the browser rule. Do not concatenate it into `moqui-html.js` or a qvt bundle. Ship the LGPL text and a link to the source. |
| QZ Tray desktop application | LGPL-2.1, and the qz.io builds add a premium signing certificate | Operator installs it | Do not vendor the desktop binary. Same shape as Chromium. |
| `moqui-html.js`, `MoquiLib`, macro modules | CC0 plus the patent grant | Ours | OK |

CKEditor 4 after 4.22.1 is commercial. CKEditor 4.22.1 was the last MPL OR LGPL OR GPL release. CKEditor 5 is GPL-2.0-or-later. Both stay out. The architecture proposal already replaced them with `QEditor`.

jQuery, Moment, Bootstrap 3, Select2, and the rest of `CombinedBase` are removed. Nothing in that stack was a copyleft problem (they are MIT). Removing them does not change the license result.

## Rejected or not carried

| Item | License | Why it is absent |
|---|---|---|
| MySQL Connector/J, `libmysqlclient` | GPL-2.0 with the Universal FOSS Exception | Replaced by the `mysql` crate. |
| MariaDB Connector/C | LGPL-2.1 | Not used. |
| H2 | MPL-2.0 OR EPL-1.0 | Replaced by embedded Postgres. |
| Jakarta servlet, transaction, and mail APIs | EPL | The servlet stack is not carried. |
| GNU readline | GPL-3.0-or-later | Not linked by the postmaster or by Rust. `psql` is the file that references it, and `psql` is stripped from the payload. |
| `libssh` | LGPL-2.1 | Not used. `russh` is the SSH crate. |
| `ssh2`, `libssh2` | `libssh2` is BSD-3-Clause | A C client with no server. Not used. |
| Elasticsearch server | Elastic License 2.0 or SSPL | OpenSearch is the HTTP backend, and it is not embedded. |
| CKEditor 5 | GPL-2.0-or-later | Replaced by Quasar `QEditor`. |
| `chromiumoxide_fetcher`, vendored Chromium, Google Chrome as a deployment binary | Chrome is proprietary. Chromium is BSD plus a large notice. | The operator installs Chromium. |
| `servo` in the default binary | MPL-2.0, and FreeType's GPL alternative | Feature off until a separate deny report. FreeType, if ever linked, is taken under the FreeType License. |
| Bitronix source | Upstream LGPL. The `org.moqui:btm` fork is annotated Apache-2.0. | Behavior is reimplemented. The source is not translated. |
| Shiro, FreeMarker, Groovy, Jetty, SnakeYAML, Jackson | Apache-2.0 | Not linked. Permissive, and still not carried. |
| `native-tls`, vendored OpenSSL inside the Rust binary | OpenSSL 3 is Apache-2.0 | Kept out so the binary has one TLS stack. Permissive either way. The postmaster may still load the host `libssl.so.3`. |
| `comrak`'s C regex backend | `comrak` itself is BSD-2-Clause | Not in the graph. |

## Enforcement once the workspace exists

`cargo deny check` runs in CI against the workspace, on the default features and again with each optional feature that can see a different graph (`pdf-servo` in particular).

- Allow the permissive set in the policy table at the top.
- Deny GPL-2.0, GPL-3.0, AGPL, LGPL-2.0, LGPL-2.1, LGPL-3.0, MPL-2.0, EPL-1.0, EPL-2.0, CDDL, SSPL, EUPL, BUSL, and the Elastic License, including as an `OR` branch that is the only branch on offer. An `OR` with a permissive alternative is accepted by recording which side we take (DOMPurify's Apache-2.0).
- Clarify `webpki` 0.22.4 as ISC AND BSD-3-Clause so the empty SPDX field is not a silent exception.
- Private ignore rules are comments that name the crate, the license, and the reason. A blanket allow is not a policy.
- On every bump of `POSTGRESQL_VERSION`, unpack that target's archive and fail if any ELF `NEEDED` entry is `libreadline` or if `libreadline` is a file in the archive. The same check covers a new `.so` whose package copyright is GPL or AGPL. `scripts/pg-payload.sh check` is that check, and it prints the `NEEDED` set to read for a new GPL or AGPL library. `scripts/pg-payload.sh strip` takes `psql` out of the cached archive before the build embeds it. On the 18.6.0 gnu archive the extension libraries also `NEED` host `libpython3.11`, `libossp-uuid`, `libxslt`, `libecpg` and `libpgtypes` (plpython, uuid-ossp, xml2 and ecpg). None of them is a file in the archive.
- The binary build writes `THIRD-PARTY-NOTICES` from the crate graph plus the Postgres `COPYRIGHT` and, once theseus bundles host libraries, `dependency-notices`. `LICENSE.md` is not edited to claim those bytes are CC0.

The resolve behind the tables is a throwaway package with the feature flags in the manifest section, `cargo metadata --filter-platform x86_64-unknown-linux-gnu`, crates.io license fields, the `webpki` 0.22.4 `LICENSE` file, and `readelf -d` on the theseus 18.6.0 gnu archive. Repeating that resolve is how a later lockfile is checked against this document.
