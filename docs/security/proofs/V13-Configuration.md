# V13 Configuration

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V13 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V13.1 Configuration Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-13.1.1 | 2 | Every communication need and user-given target documented | Framework | `docs/security/Attack-Surface-Outbound.md` (each request the server makes, who supplies its target), `Attack-Surface-Process.md` and the area maps (listeners); partial: Outbound's open questions name clients not read | n/a: a document |

## V13.2 Backend Communication Configuration

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-13.2.1 | 2 | Backend links authenticated without static shared credentials | Gap | databases and OpenSearch take a static password (`moqui-drivers/src/pgwire.rs` `authenticate`; no client certificate for a database); the OpenSearch child has no sign-in (SEC-OUT-88, open); met for the cluster and edge link (certificates) | - |
| v5.0.0-13.2.2 | 2 | Backend accounts hold least privilege | Gap | every pool of the embedded cluster is its bootstrap superuser (`moqui-drivers/src/embedded.rs` `driver_for`; SEC-TLS-86, Medium, open); systemd unit sandboxed and binary not writable (SEC-OUT-13 fixed) | - |
| v5.0.0-13.2.3 | 2 | No default credentials between services | Framework | each generated-secret attribute still at `CHANGEME` gets 32 CSPRNG bytes at start (`conf_env.rs` `generate_secrets`); embedded `pgpass` random (`postgresql_embedded` settings, 16 alphanumerics); datasource examples with `moqui` passwords are comments only; compose default gone (SEC-OUT-07 fixed) | `generated_secrets_are_32_random_bytes_and_secret_names_are_known` (`context/tests.rs`); PASS: to write: in-process |
| v5.0.0-13.2.4 | 2 | Allowlist of external systems the app may reach | Configuration | operator egress firewall; the framework has a deny list of internal addresses (`moqui-lib/src/net.rs` `internal_address`) and per-feature lists only (`email-client@allowed-hosts`, `llm-facade/a2a/push@allow-hosts`); of the named lists only `email-client@allowed-hosts` restricts destinations; `llm-facade/a2a/push@allow-hosts` lifts the https and internal-address checks for its hosts | `host_rejected_by_allowed_hosts_does_not_touch_the_sink`, `host_suffix_matches_on_a_dot_boundary` (moqui-mail); `context/resource/tests.rs` `an_http_location_reaches_an_internal_address_only_off_production` (in-process) |
| v5.0.0-13.2.5 | 2 | Server allowlist of request and load targets | Configuration | operator egress firewall; an `http(s)://` resource location is any host, guarded on production only (`context/resource.rs` `http_get`) (map); the data loader takes any host path (map) | `context/resource/tests.rs` `an_http_location_reaches_an_internal_address_only_off_production` (in-process) |

## V13.3 Secret Management

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-13.3.1 | 2 | Secrets in a vault, not in source or artifacts | Gap | secrets in `runtime/.env` beside the data, no vault, KMS or HSM (`Attack-Surface-Data.md` Known gaps; `plans/Sealed-Secrets.md` not built); confs ship `CHANGEME` only; `./moq archive` includes `.env` unless `--no-data` (`scripts/moq/archive.sh`) | - |
| v5.0.0-13.3.2 | 2 | Least-privilege access to secrets | Gap | met: `.env` mode 0600 (`moqui-lib/src/env.rs` `update_dotenv_file`), scripts get no secret conf value (`script/host/globals.rs` `get_conf_value`, `context/identity.rs` `script_refused_property`), admins read only whether set; not met: Tools auto forms show encrypted fields decrypted (SEC-DAT-92, Low, open) | - |

## V13.4 Unintended Information Leakage

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-13.4.1 | 1 | No source-control metadata reachable | Configuration | deploy with `./moq archive` or the binary, which leave out `.git` (`crates/moqui-embedded/archive-excludes.txt`); a checkout keeps `.git` readable by server scripts; never served: a name starting with `.` is refused (`screen/static_files.rs` `safe_segments`; `traversal_hidden_and_odd_names_are_refused`) | V13Configuration "v5.0.0-13.4.1: no source-control metadata is served: /.git and the like are not found"; `traversal_hidden_and_odd_names_are_refused` (`screen/static_files.rs`) |
| v5.0.0-13.4.2 | 2 | Debug features off in production | Framework | `instance@purpose` defaults to `production` (`MoquiDefaultConf.xml`); dev components load only on `dev` (`context/component.rs`) (map) and the dev tools ask `devtools::is_dev_instance`; a 500 answers an error id (`web/mod.rs` `error_text`); related: SEC-DAT-95 (database DETAIL text on production, Low, open) | V16Logging "v5.0.0-16.5.1: the internal error page shows no stack trace and escapes its message"; to write: black-box |
| v5.0.0-13.4.3 | 2 | No directory listings served | Framework | a static file is sent only for a regular file (`screen/static_files.rs` `file_under`), no index pages; DAV `PROPFIND` and `/resources` list for signed-in users by design (map) | V13Configuration "v5.0.0-13.4.3: a static directory's path lists nothing; its files are served" |
| v5.0.0-13.4.4 | 2 | HTTP TRACE refused in production | Configuration | refused 405 only by web protection's `requests` methods list (`moqui-host/src/guard.rs`), applied when `webapp-list/webapp[webroot]/web-protection@enabled` is true or `@trusted-proxies` is set; by default TRACE reaches the screen chain (likely; not run) | V13Configuration "v5.0.0-13.4.4: a TRACE request is refused, not answered with the page or the request" (a deployment's; skips in the fixtures profile) |
| v5.0.0-13.4.5 | 2 | Internal API docs and monitoring not exposed | Framework | `/health`, `/metrics`, `/status` only for `webapp-list/webapp[webroot]@status-ips` or loopback (`moqui-host/src/http.rs` `status_allowed`; SEC-WEB-11 fixed); REST schemas need `REST_SCHEMA`; RAML and Swagger not built; `/devstatic` dev only | V13Configuration "v5.0.0-13.4.5: the entity REST schema is served only with REST_SCHEMA: #who"; SecurityAccess "A01 /status from loopback is JSON ..." (the loopback side); to write: black-box (from an address off `@status-ips`, `/status`, `/health` and `/metrics` are refused) |
