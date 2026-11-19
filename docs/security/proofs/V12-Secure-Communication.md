# V12 Secure Communication

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V12 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V12.1 General TLS Security Guidance

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-12.1.1 | 1 | Only TLS 1.2 and 1.3, newest preferred | Framework | every server and client rustls config takes `with_safe_default_protocol_versions` (`tls.rs` `server_config`, `cluster_edge/mod.rs`, `edge_link/accept.rs`, `pg_tls.rs`); rustls has nothing below 1.2 and prefers 1.3; no OpenSSL or native-tls in `Cargo.lock` | to write: black-box |
| v5.0.0-12.1.2 | 2 | Recommended cipher suites, strongest preferred | Framework | `aws_lc_rs::default_provider()` suites only: AES-GCM and ChaCha20-Poly1305 with ECDHE (`tls.rs` `server_config`) | to write: black-box |
| v5.0.0-12.1.3 | 2 | mTLS client certificates checked before identity use | Framework | cluster listener: client certificate mandatory, SHA-256 pin of `cluster-member@tls-cert` and handshake signature checked (`moqui-host/src/tls_pin.rs` `verify_client_cert`); edge link: key must match an active `EdgeClient` (`edge_link/accept.rs`) (map); validity dates not checked; related: SEC-CLU-81 (member key admitted on first use, Low, open) | `a_member_without_the_cluster_certificate_is_refused` (`moqui-host/src/cluster_edge/tests.rs`) |

## V12.2 HTTPS Communication with External Facing Services

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-12.2.1 | 1 | TLS for all external HTTP, no clear-text fallback | Configuration | plain HTTP listener always binds `webapp-list/webapp[webroot]@http-port` (8080); set `webapp-list/webapp[webroot]@https-enabled` (screens redirect, `screen/render.rs` `redirect_to_https`) with the HTTPS listener (`@https-listen-port`, certificate in `runtime/cert`) or a TLS proxy, and do not publish the HTTP port; HSTS only on https screen answers (`screen-secure`) | - |
| v5.0.0-12.2.2 | 1 | Publicly trusted certificates on public services | Configuration | ACME on System › Certificates (`acme@directory-url`, `org.moqui.admin.CertificateServices`) or an operator certificate in `runtime/cert`; a self-signed one is made only on `instance@purpose` `dev` (`tls.rs`) | - |

## V12.3 General Service to Service Communication Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-12.3.1 | 2 | Encrypted transport on every inbound and outbound link | Configuration | clear text by default: MX STARTTLS opportunistic unless `mail-delivery@mx-require-tls` or MTA-STS (`moqui-mail/src/mx.rs` `tls_required`), `search-facade/cluster@url` `http://127.0.0.1:9200`, Postfix relay, SIP UDP and TCP, a MySQL conf connection with no mode (SEC-DAT-97), a script's `grpc://` PASS endpoint (SEC-OUT-83, open); TLS by default: datasources (empty `sslmode` is `verify-full`), cluster, edge link | - |
| v5.0.0-12.3.2 | 2 | TLS clients check server certificates | Configuration | `RestClient` checks chain and name, no `danger_accept_invalid_certs` (`moqui-lib/src/http.rs`); datasource default `verify-full`; not by default: MX STARTTLS takes any certificate unless `mail-delivery@mx-require-tls` or MTA-STS (`mx.rs`); Postgres `require` takes any, `verify-ca` any public name (SEC-DAT-93) | - |
| v5.0.0-12.3.3 | 2 | TLS between internal HTTP services, no fallback | Configuration | cluster listener TLS only (`cluster_edge/mod.rs`); edge link TLS; plain only to loopback for Docker (SEC-OUT-08 fixed) and for PASS gRPC without a certificate (`pass-authority@grpc-insecure`); OpenSearch is http unless `search-facade/cluster@url` is https | PASS: Transport "a transport in clear text to another host is refused unless pass@backend-insecure says so" (mr-pass `test/Transport.spec.js`) |
| v5.0.0-12.3.4 | 2 | Internal TLS trusts only named CAs or pins | Gap | met: cluster certificate pinned by SHA-256, edge link key pinned (`tls_pin.rs`); not met: Postgres `verify-ca` without `sslrootcert` trusts every public root for any name (SEC-DAT-93), a MySQL datasource ignores `sslrootcert` and a MySQL conf connection adds it beside the public roots (SEC-DAT-97) | - |
