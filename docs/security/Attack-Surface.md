# Attack surface of mr-moqui

This is a map of the HTTP routes, screens, REST, WebSockets, and the other listeners in this repository. It is for operators, security researchers, and review of this repository. It records what can be reached. It is not a vulnerability report, and it is not a WAF. Proof of the controls lives in [Proof-Tests.md](Proof-Tests.md).

Application components (Mantle, Marble, IronStone, and the mr-boss screens beyond the framework's token-route mount) add their own screens and services. Those are out of scope here, except where the framework mounts a route they plug into.

To report an issue that should stay private until it is fixed, write to [moqui-board@googlegroups.com](mailto:moqui-board@googlegroups.com). The policy is in [SECURITY.md](../../SECURITY.md).

## How to read this

A request goes from the client, through a reverse proxy or TLS, to hyper and axum in `crates/moqui-host/src/http.rs`, then screen dispatch, artifact authorization, and on to the database, search, mail, and jobs.

This server terminates TLS itself with rustls (`crates/moqui-host/src/tls.rs`) when the HTTPS listener is started. It is still meant to run behind a proxy. `webapp_https_enabled` defaults to false and is the flag generated URLs use when they pick https. Starting the listener is the separate rule in [HTTP container](#http-container).

**Defaults** are `runtime/base-component/framework/conf/MoquiDefaultConf.xml`. **On** and **off** mean those defaults. Rows say what untrusted input can reach, and what to keep in mind during a review. Designed admin tools are privileged on purpose.

`./moq run` merges `runtime/conf/MoquiDevConf.xml` on top of that file. The dev file sets `instance_purpose` to `dev`, `webapp_allow_origins` to `*`, and `ssh_port` to `2222`, and it sets `tarpit-enabled` false for screens, screen transitions, and services. It does not declare an H2 console. The Jackrabbit repository element in it is commented out. The tables below stay on the default conf unless a row says otherwise.

## HTTP authentication and authorization

A servlet request is signed in from the session cookie, HTTP Basic, an `api_key` or `login_key` header, or `authUsername` and `authPassword` (`docs/interfaces/Web-Scripts.md`). `/notws` accepts the session cookie and, failing that, a `login_key` header. `/termws` accepts the session cookie only. Notification sockets revalidate the original session or hashed login key every 60 seconds and close on logout, a changed user, key revocation or expiry, or a failed credential lookup. Revalidation does not log in again or write login history. Protocol Ping/Pong traffic cannot extend authentication.

A node the desktop app runs in its own process may have no HTTP or cluster port (`moqui_host::start_embedded_with`, `crates/moqui-host/src/in_process.rs`). Its window's requests and sockets reach the router in process, through the same checks as a listener's, from the loopback address, as a sidecar window's do over loopback HTTP. The app keeps the node's session cookie for the window and sets `Host`, `Cookie` and the forwarded and edge link headers itself, never from the page (`crates/moqui-desktop/src/node/scheme.rs`). Nothing outside the app's process reaches that path.

Screens carry `require-authentication`. The parser maps `false`, `anonymous-view`, and `anonymous-all` to those modes, and it maps a missing attribute to required. A render requires a user when any screen in the chain is required. An anonymous login counts as a user.

Artifact authorization runs as each screen, transition, service, REST path, entity, git repository, and LLM call is pushed. Inheritable allow and always records cover children on that stack unless a more specific deny wins.

Seed grants, from `runtime/base-component/tools/data/ToolsSecurityData.xml`:

| Group | Artifact | Grant |
| --- | --- | --- |
| `TOOLS_APP` | `component://tools/screen/Tools.xml`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `SYSTEM_APP` | `component://tools/screen/System.xml`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API` | `/moqui`, `AT_REST_PATH`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API_GIT` | `/moqui/git`, `inheritAuthz` Y | `ALL_USERS`, `AUTHZT_ALLOW`, `AUTHZA_ALL`. The services still check `AT_GIT_REPOSITORY` |

`user-facade` `authz` sets the privileged groups to `ADMIN`, `ADMIN_ADV` and `SYSMSG_RECEIVE`. Creating, updating, or deleting membership in those groups is allowed only when the caller is already a member. Seed and install, and a call with authorization disabled, skip that check. `SYSMSG_RECEIVE` is there because its members call `receive#IncomingSystemMessage` as a remote system, and a party screen that adds memberships by posted `userGroupId` would otherwise let a user join it. The sealed permissions are `GROOVY_SHELL_WEB`, `SQL_RUNNER_WEB`, `SSH_SHELL`, `SERVICE_LOAD_RUNNER`, `ADMIN_LOGIN_AS`, `ADMIN_PASSWORD`, `SearchRemote`, `DashboardRemote`, `REST_SCHEMA`, `LlmGatewayUse`, `COMPONENT_DEPLOY`, `DATABASE_ADMIN`, and `DATABASE_EXTERNAL`. A `UserGroupPermission` row for one of those can be written only by a caller who already has that permission.

From `runtime/base-component/tools/data/MonitoringSecurityData.xml` ([Monitoring](../tools/Monitoring.md#security)), all granted to `ADMIN` only:

| Group | Artifacts |
| --- | --- |
| `MONITOR_APP` | `component://tools/screen/Monitor.xml` (`inheritAuthz` Y), the `org.moqui.report.dataset.otel_*` datasets, `org.moqui.report.dataset.monitor_annotations`, `org.moqui.monitor.MonitorServices.*` |
| `DASHBOARDS_APP` | `component://tools/screen/Dashboards.xml` and `/moqui/reports` (`inheritAuthz` Y), `org.moqui.report.ReportServices.*` |
| `REPORT_DATASETS_ADMIN` | every `org.moqui.report.dataset.*`: unrestricted dataset access |
| `REPORT_CONTACT_ADMIN` | `org.moqui.report.contact.webhook`: alert webhooks |
| `REPORT_ALERT_EVALUATION` | `org.moqui.report.alert.evaluateAll`: the alert job evaluating every owner's rules |

A report query is authorized on its dataset (`org.moqui.report.dataset.<id>`, plus `DataDocumentUserGroup` for a DataDocument) as the viewer, or as the owner for alerts and scheduled reports, with server-side row filters. The permissions `MONITOR_INGEST` (the owner of an ingest key) and `REPORT_SAVE_LOCATION` (scheduled report files on the server) are seeded on `ADMIN`.

The proxy permissions are `SearchRemote` and `DashboardRemote`. Both are seeded on `ADMIN`.

`AgentTypeData.xml` defines `AgentUse`, `AgentAdmin`, `AgentAudit`, `LlmGatewayUse`, `A2aServerUse`, `McpServerUse`, and `AgentSkillAuthor`. The `ADMIN` grant rows in that file are `AgentUse`, `AgentAdmin`, and `AgentSkillAuthor`. `A2aServerUse`, `McpServerUse`, and `LlmGatewayUse` are not among those rows. `AgentAudit` is not among them either. `SecurityTypeData.xml` grants `SQL_RUNNER_WEB`, `SSH_SHELL`, `SERVICE_LOAD_RUNNER`, and `ADMIN_LOGIN_AS` to `ADMIN_ADV`, and `ADMIN_PASSWORD` to `ADMIN`. The obsolete `GROOVY_SHELL_WEB` permission is no longer seeded or used by a screen; its name remains sealed for pre-existing database rows.

## Cross-cutting controls

These apply unless a row says otherwise.

### Session cookie

The session row is `moqui.server.HttpSession`. The cookie is `moqui.session={id}.{mac}`, then `HttpOnly`, `SameSite=Lax`, and `Path=/`. `Secure` is added only when the request is https: the TLS listener, or `X-Forwarded-Proto: https` from a peer `ClientAddressing` believes. A client that is not that peer cannot turn the flag on by sending the header. The visitor cookie is `moqui.visitor`, with `Max-Age` of one year. The session cookie itself has no `Max-Age`.

The MAC is HMAC-SHA256 of the session id under `session_cookie_key`, base64 URL-safe without padding. A bad MAC is treated as no session. The session id and the session token are 20 random bytes, hex, 40 characters. `session-config` is `timeout="60"` (minutes), `flush="eventual"`, `max="10000"`.

`session_cookie_key` ships as `CHANGEME` with `is-secret="true"`. Startup generates a value into `runtime/.env` when the property still ends in `CHANGEME`. See [Secrets](#secrets).

### CSRF

`webapp_require_session_token` is `true`. POST, PUT, PATCH, and DELETE on a transition that runs actions must send `moquiSessionToken`, or a header `moquisessiontoken`, `sessiontoken`, or `x-csrf-token`. The compare is constant time. A missing or wrong token is 401 `Session token is missing or is not valid`. A transition may set `require-session-token="false"`. Header authentication skips the check: Basic, `api_key`, `login_key`, or `authUsername`. A new session skips it too. Responses set `moquiSessionToken` and `X-CSRF-Token` when the cookie is new or the token changed.

### Login lockout and passwords

`login` is `max-failures="3"` `disable-minutes="5"` `history-store="true"`. The account is disabled when `successiveFailedLogins + 1` is greater than `max-failures`, so the fourth failure starts the five-minute window. That window is not a substitute for an edge limit on the login URL. The per-client login rate limit in [Web protection](#web-protection) is off until per-client rules are on.

New password hashes are argon2id. Legacy hashes still verify, including the conf type `SHA-256`. The password element also sets `min-length="8"`, `min-digits="1"`, `min-others="1"`, `history-limit="5"`, `change-weeks="104"`, and `email-expire-hours="48"`. A login key is hashed with `login-key` `encrypt-hash-type`, which is SHA-256, and it is not hashed with argon2id. `login-key` `expire-hours` is 144.

A sensitive action can ask for a recent sign-in (`check#RecentAuth`, five minutes): a password login, a second factor's code,
an SSO login with a recent `auth_time` or a re-authentication counts; a key, an app password or an internal login does
not, and another user's login in the session drops it. `reauthenticate#User`
checks the signed-in user's password and code under the same lockout rules as the login form ([Recent sign-in](Recent-Auth.md)).
`/ReAuth/check`, `/ReAuth/password` and `/ReAuth/sendCode` are 401 without a user; the posts need the CSRF token.

`update#Password` is `authenticate="anonymous-all"` and `allow-remote="false"`. `reset#Password` is `authenticate="anonymous-all"` and `allow-remote="true"` (`UserServices.xml`). `create#InitialAdminAccount` is `authenticate="anonymous-all"`. It returns an error when more than one `UserAccount` row exists (the seed row `_NA_` is the one that may already be there), then creates the account and adds it to `ADMIN` with authorization disabled.

### Tarpit

Screens, transitions, services, REST paths, LLM calls, and `AT_OTHER` have the tarpit on. `AT_ENTITY` and `AT_GIT_REPOSITORY` have authorization on and the tarpit off. Counts are per user. The dev conf turns the tarpit off for screens, transitions, and services.

| Type | Authz | Tarpit |
| --- | --- | --- |
| `AT_XML_SCREEN`, `AT_XML_SCREEN_TRANS`, `AT_SERVICE` | on | on |
| `AT_ENTITY` | on | off |
| `AT_LLM` | on | on |
| `AT_GIT_REPOSITORY` | on | off |
| `AT_REST_PATH`, `AT_OTHER` | on | on |

`AT_LLM` seed counts, all for `ALL_USERS`, `inheritAuthz` N (`AgentTypeData.xml`): `agent:.*` 30 hits in 60 seconds, then 300 seconds; `gateway:.*` 120 in 60, then 120; `assist:.*` 60 in 60, then 120; `mcp:.*` 120 in 60, then 120.

### HTML and uploads

A service parameter `allow-html` defaults to none, which rejects text that contains `<`. There is no webapp attribute of that name. `safe` cleans with ammonia and allows the span attributes `data-mention-id`, `data-record-entity`, and `data-record-id`. `any` leaves the markup.

`webapp_upload_executable_allow` is `false`. An upload is refused unless that setting is `true`. The check matches a `#!` shebang and the magics `MZ`, ELF, a Java class file, and four Mach-O values. A ZIP (`PK`) is not treated as a program. `webapp_max_upload` is 2147483648 bytes. A file over 256 KiB is written under `runtime/tmp/upload`.

A body that is not multipart stops at `webapp_max_body`. This repository has no `default-property` of that name (`MoquiDefaultConf.xml` only interpolates `${webapp_max_body}`, and `MoquiDevConf.xml` does not set it). The code fallback is 10 MiB (`crates/moqui-core/src/web/mod.rs`). A runtime override in the environment or in `runtime/.env` was not confirmed.

Screen JSON removes credential fields at any depth, case-insensitive: `password`, `oldPassword`, `newPassword`, `newPasswordVerify`, `currentPassword`, `resetPassword`, `authPassword`, `api_key`, `login_key`, `passwordSalt`, `passwordHashType`, `passwordBase64`, `passwordVerify`.

### Response headers

Screen renders send `Content-Security-Policy: frame-ancestors 'none'; form-action 'self';`, `X-Frame-Options: sameorigin`, `X-XSS-Protection: 1; mode=block`, and `X-Content-Type-Options: nosniff`. `Strict-Transport-Security: max-age=31536000` is on type `screen-secure`. A stricter CSP in the conf comment is commented out. The Assist sandbox frame uses its own policy, under [Assist](#assist).

### CORS

`webapp_handle_cors` is `true`. `webapp_allow_origins` is empty. An origin is its scheme, host, and port, all three. The server's own origin is allowed: https over TLS or from a believed proxy, with the host and port of `Host` or of a believed proxy's forwarded host and port, and the webapp URLs from the conf. A foreign origin is 401 `Origin not allowed`, before the servlet. An `allow-origins` entry that includes a scheme is matched the same way. A bare domain allows that host and the hosts under it on any scheme and port. `*` allows every origin. The dev conf sets `*`.

An `OPTIONS` preflight is 204 with an empty body, before the servlet chain (`crates/moqui-core/src/screen/mod.rs`). Preflight and actual responses send `Access-Control-Allow-Credentials: true`. Preflight also sends the conf allow-headers (including `api_key`, `login_key`, `moquiSessionToken`, `X-CSRF-Token`, `Authorization`, `A2A-Version`, `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`), allow-methods `GET, PUT, POST, PATCH, DELETE, OPTIONS`, and `Access-Control-Max-Age: 3600`.

`/notws`, `/termws`, and the Assist event stream use the same origin list and answer 403 `origin not allowed`.

### Client IP

`webapp_client_ip_header` and `webapp_trusted_proxies` are empty, so `X-Forwarded-For` is not trusted. `ClientAddressing` believes a peer only when a header is configured. If a header is set and the trusted-proxy list is still empty, the first hop is believed from any peer, and `X-Forwarded-Proto` uses that same gate. The comment above the property still names Jetty's `ForwardedRequestCustomizer`. The listener uses `ClientAddressing`.

On an edge node (`node_role=edge`) and an all-in-one node (`node_role=all-in-one`) a header with an empty trusted-proxy list is ignored, with an error at startup: the edge node faces the internet and passes the client address to its local nodes ([Edge node](../Run-Deploy.md#a-local-server-with-an-edge-node)).

`/health` and `/metrics` use the TCP peer, not this header. `/status` uses `ec.user.getClientIp()`, which can differ.

### Field encryption

`entity_ds_crypt_pass` ships as `MoquiDefaultPassword:CHANGEME` with `is-secret="true"`. It is the `entity-facade` `crypt-pass`. The algorithm is `PBEWithHmacSHA256AndAES_128`, salt `20201202`, 10 iterations. The same secret encrypts an OAuth refresh token and a client secret stored on `EmailOAuthProvider`, the SSO client secrets (`OauthFlow.secret`, `OidcFlow.secret`), the SAML keystore and private key passwords, and a calendar publish link's token (`CalendarShare.publishToken`, still found by value since the encryption is deterministic). Alternate decrypt still tries `PBEWithMD5AndDES` with the current pass and with `entity_ds_crypt_pass_old`, and the AES algorithm with the old pass. The old pass ships as the same `CHANGEME` sentinel and is not replaced by the generator.

`entity_ds_user` and `entity_ds_password` default to `sa` and `sa`. Those are the old datasource properties. The embedded Postgres client does not connect with them.

### Web protection

`web-protection` always applies connection limits, request limits, and overload. Per-client rules (rate limit, ban, path rules, user-agent rules) are on when `webapp_trusted_proxies` is set, or when `webapp_web_protection` is `true`. Both default to empty, so per-client rules are off on a default install. Loopback (`127.0.0.0/8`, `::1`) is exempt when those rules are on. The mr-boss limit on `POST /r/*/submit` is one of those per-client rules.

| Control | Default |
| --- | --- |
| Methods | `GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS` |
| Max URI | 8192 bytes |
| Max header bytes / headers | 65536 (the code forces at least 8 KiB) / 100 |
| Header timeout / body timeout | 30 s / 60 s (code defaults) |
| Upload idle / request timeout | 60 s / 300 s |
| Connections / shutdown | 1024 / 30 s (code defaults) |
| Overload | queue timeout 10 s, `Retry-After` 5 |
| Rate limit `all` | 50 requests / 1 s, burst 500 |
| Rate limit `login` | `POST` on `/Login/**`, `/rest/login`, `/rest/login/**`, `/SecondFactor/**`: 10 / 60 s, burst 10 |
| Ban `auth-failure` | 20 events in 600 s, ban 900 s |
| Ban `abuse` | `rate-limited` and `denied`, 300 in 60 s, ban 300 s |
| Path deny `scanners` | `/.env`, `/.git/**`, `/wp-admin/**`, `/wp-login.php`, `**/*.php`, `**/*.asp`, `**/*.aspx` |
| User-agent deny | `sqlmap`, `nikto`, `masscan`, `zgrab` |

## HTTP container

The listener is hyper and axum. `webapp_http_bind` empty binds every address. `webapp_http_port` empty is port 8080. The plain HTTP listener is HTTP/1.1. The HTTPS listener offers `h2` and `http/1.1` by ALPN (`webapp_http2=false` offers only `http/1.1`). An HTTP/2 connection takes at most `http2-max-streams` (100) open streams, a request head of at most `max-header-bytes`, h2's bounds on streams a client resets before they are taken (20) and on streams reset for its protocol errors (1,024), PINGs every 30 seconds, and is closed with a GOAWAY after `header-timeout` with no request, or when web protection refuses its client with a closing refusal (HTTP/1.1's `Connection: close`). An HTTP/2 request is given `Host` from `:authority`; a WebSocket over extended CONNECT is checked by web protection as the GET of an HTTP/1.1 upgrade. Over HTTP/2 a `content-type: application/grpc` call goes to the gRPC services that are on (OTLP ingest, PASS authority; the rows below) before web protection's per-request rules and the router, as on their own port, except a call for an edge node's terminate host, which is forwarded as any request. There is no separate servlet descriptor: filters and servlets are the webroot `<webapp>` children in `MoquiDefaultConf.xml`, run by the screen chain. The session is the database row above.

`webapp_https_enabled` is `false`. `webapp_https_listen_port` empty is 8443 when the listener is used. The value `off` starts none. `plan` does not start HTTPS when `runtime/cert` has no usable certificate, unless the port was set explicitly (that case is an error with no certificate). `instance_purpose=dev` tries to generate a development certificate first. `webapp_https_port` is the port clients see in generated URLs, which can differ behind a proxy.

The request deadline is `request-timeout`, code default 300 seconds, and the answer is 504. Exempt paths are `/notws`, `/termws`, paths starting with `/git/`, `/dav/`, and `/llm/v1`, and multipart uploads.

Default Cargo features are `maximal` and `devtools` (`crates/moqui-host/Cargo.toml`). `maximal` includes `enterprise`, which includes `git`, `llm`, `tui`, and `ssh`. `lite` includes `llm` and `tui`. `git` and `ssh` are on the `dev`, `enterprise`, and `maximal` sets. A default `cargo build` therefore compiles `/git`, `/termws`, and the LLM routes. `inbound-smtp` is on Enterprise and Maximal. `embedded-postgres` is on Dev, Lite, and Maximal. Enterprise leaves that feature out and uses an external database.

`<terminal max-sessions="16" max-sessions-per-user="4"/>` limits terminal sessions. The browser socket is `/termws`. The screen is `/qapps/Terminal`, which requires authentication. SSH is a separate listener, under [SSH and the TUI](#ssh-and-the-tui).

## Host routes and servlets

Registration order is `listen` in `crates/moqui-host/src/http.rs`. Anything that does not match a host route falls through to screen dispatch.

| Surface | Path | Default | Authn | Authz | Considerations |
| --- | --- | --- | --- | --- | --- |
| Health | `GET /health` | on | TCP peer in `webapp_status_ips` (default `127.0.0.1`) plus `127.0.0.1`, `::1`, and `0:0:0:0:0:0:0:1`; otherwise 404 | that IP list | JSON `{"ready":true}` or `{"ready":false}` with 503 when pool stats are missing. The client-ip header is not used. |
| Metrics | `GET /metrics` | on | same IP check | same list | Plain text pool, guard, and (when a search replica exists) replica-lag gauges, and the telemetry queue, export, drop and failure counters. 503 when pool stats are missing. No request span is written for it or for `/health`. |
| OTLP ingest | `POST /v1/traces`, `/v1/logs`, `/v1/metrics`, `/v1/annotations` | on in builds with `telemetry` unless `telemetry/receiver` `enabled="false"` | an ingest key as `Authorization: Bearer` or `api_key`; not a login credential | the key's owner must hold `MONITOR_INGEST` on every request; key revocation, expiry and the owner's enabled state are checked each time | Body limit 4 MiB, 600 requests per minute per key, 8 at once (`telemetry/receiver`). Redaction runs before indexing. The source recorded is the key's, never the sender's claim. No request span is written for these paths ([Monitoring](../tools/Monitoring.md)). |
| Recovery reports | `POST /v1/recovery` | as OTLP ingest (`telemetry/receiver`) | an ingest key as `Authorization: Bearer`, and the action's callback token in the body | the key's owner must hold `MONITOR_INGEST`; the token is random per action, sent only in the first request to its controller, and stored only as a SHA-256 hash | JSON only. A report can move only an active action to running, succeeded or failed; the message is redacted. It never starts an action. No request span is written ([Monitoring](../tools/Monitoring.md#recovery)). |
| Recovery webhooks (outbound) | `POST` to a `RecoveryAdapter` URL, and `GET` its `statusUrl` | off until an adapter of type `webhook` is stored and a policy chooses it, with approval or `telemetry/recovery` `allow-automatic="true"` | none from MR Moqui beyond the body's HMAC-SHA256 signature (`X-Moqui-Signature`) when `secretProperty` names a key | storing an adapter needs `MONITOR_RECOVER_ADMIN`; URLs carry no credentials and headers and secrets come from server properties | Private and loopback addresses are refused unless the adapter's `allowInternal` is `Y`. 10 s timeout, at most five attempts with the same idempotency key, then the action's deadline. |
| Instance drain | `/health` answers 503 with `draining` | the `isolate-instance` recovery action, through `moqui.monitor.InstanceDrain` | as `/health` (`status-ips` and loopback) | running or reverting the action needs `MONITOR_RECOVER` (or an automatic policy) | A drained instance still serves the requests that reach it; only a load balancer that reads `/health` stops sending more. |
| Browser RUM | `POST /rum/traces`, `/rum/logs`, `/rum/replay` (webroot `rum.xml`) | off; `telemetry/browser` `enabled="true"` (replay also `replay-enabled="true"`) | the session cookie of a signed-in user, else 401 | the session's CSRF token as for any post; no permission to send, since a user reports only on their own pages | The server replaces the page's identity with the session's user and visit and its own service name, keeps only CLIENT and INTERNAL spans (at most 512 a post), and drops attributes that name a user, visit, source or service. 256 KiB a post, 120 posts a minute per user and visit; a replay is held to its minutes and bytes and to the session that began it. Replays are masked in the browser and read back only with `MONITOR_REPLAY`, never as a report dataset or through Assist; each view is logged. No request or artifact span is written for these paths. |
| OTLP/gRPC ingest | the trace, logs and metrics collector services on `telemetry/receiver` `grpc-port` (4317), and on the HTTPS listener over HTTP/2 | off; built with `grpc` (every package), opened by `grpc-enabled="true"` | the same ingest key, as `authorization: Bearer` or `api_key` metadata | as OTLP ingest | The host's gRPC listener at `grpc-bind` (the HTTP bind by default), shared with the PASS authority's gRPC; TLS with the HTTPS certificates else h2c. The edge guard admits each connection. The same body limit (as tonic's decoding limit), rate, concurrency, redaction and queue as OTLP/HTTP. |
| PASS authority gRPC | `moqui.pass.v1.PassAuthority` (`Call`, `Download`, `Upload`) on the host's gRPC listener: the OTLP receiver's port when that is on, else `pass_authority_grpc_host`:`pass_authority_grpc_port` (4317); and on the HTTPS listener over HTTP/2, always TLS | off; built with `edge` (Enterprise, Maximal), opened by `pass_authority_grpc_enabled=true`; with no certificate for TLS it is left off (an error logged) unless the address is loopback or `pass_authority_grpc_insecure=true` | a registered PASS client: `x-pass-client` (or the call's `client_id`) and `x-pass-client-secret` metadata, checked by the framework's `check#Client` (the REST servlet's check, a digest comparison) on the request pool before the authority service runs, and for `Upload` on its first part, before the file is read; a missing or wrong one is `UNAUTHENTICATED`. Wrong secrets are counted per address: after 10 in 10 minutes its calls are `RESOURCE_EXHAUSTED` until the window ends, and only the first and the tenth are logged | the authority service (`pass_authority_service`, Mantle's `handle#Call`) as the REST servlet: only routed operations, each input field checked against the operation's schema, the client's sites, a delegation checked per call (the visitor's identity), and a write with no delegation refused unless it is an account action, an opt-out or a bearer link's token write. An account action (not the SSO back channel, which keeps its state in handoff rows) binds the authority session its handle names: `p1.<session id>.<MAC>` under a key derived from `session_cookie_key` for this use only, so a handle is never a `moqui.session` cookie, and a session made for one client and site, so another client's or site's handle gets a new session. Once a delegation is issued the session's user is cleared, so a handle carries only a sign-in in progress (the second factor) | No CSRF: it has no browser session. The visitor's address is data for the account rate limits only. A request message is at most 2 MiB (an upload's file goes in 64 KiB parts, at most 10 MiB), an answer at most 16 MiB; a download streams the authority's file in 64 KiB chunks. The edge guard admits each connection. A refusal is the REST API's error envelope with its status ([plan](../../plans/Edge-Node.md) §5.5). Over REST the frontend keeps the authority's session cookie in its own server-side session, still signed in after the delegation is issued (as before E4); only gRPC clears it. |
| Call guests | `GET /callws` | on with the rtc feature and `/notws` | none at the upgrade (origin checked as for `/notws`); the first frame must be `{rtc: 'guestAuth', credential}` within 10 s, checked with `get#GuestState` | that guest's own call and leg only | WebSocket of a call link's guest page. It takes `rtc` frames of that call and leg only, and no topic, notification, co-editing or other call's frame. The credential is checked again every 30 s; one that no longer works ends the call for the page and closes the socket. 128 KiB a frame. |
| Chat bridge webhooks | `POST /channels/{chatBridgeId}/webhook` | off; `channels_webhooks_enabled=true` registers it | none from a session or cookie: the bridge's adapter checks the network's signature over the raw body before it parses anything (SMS: Telnyx's Ed25519 signature of the timestamp and body under the bridge's public key, aws-lc-rs); a missing or wrong signature, or a timestamp more than 300 s off, is 401 and counts as a refused login for web protection | a signed request acts only as that bridge's network: `ingest#Event` with `transportVerified`, which still refuses an item naming a local user, an unmapped room and a malformed item, and only for a number whose messaging bridge it is | The body is read first, at most `channels_webhook_max_bytes` (256 KiB; a larger declared body is 413 before the read). A signature already worked within the window is answered 200 `duplicate` and not worked again, one still being worked 409; neither counts for web protection. The signatures are held in the distributed replay cache the system-message HMAC check uses (`moqui.security.hmac.replay`), so a copy that reaches another node is caught there as far as that cache reaches (the node's own until a distributed cache is configured, as for the HMAC check); the event and message ids dedupe on any node. `transportVerified` holds only inside the bridge's own webhook (the context is marked), never from a parameter. An unknown bridge (or an id that is not one), one this build has no adapter for, and a disabled one to anyone but its network get the one answer of an unsigned request (401), so bridge ids cannot be probed; a disabled bridge answers its network, by its signature, 503. Files a text names are fetched by the adapter from public addresses only, each fetch reading no further than `maxInboundMediaBytes`, and kept with a safe content type (a page or SVG is kept as bytes). Worked on a request worker; the provider's retries are no-ops ([SMS](../interfaces/SMS.md)). |
| Chat bridge media | `GET /channels/{chatBridgeId}/media/{token}` | off, with the webhook | the token: the attachment id and an expiry, HMAC-SHA-256 under `channels_media_key` (generated) | only a file of a message with a delivery to that bridge, until the expiry (`channels_media_minutes`, 60) | For the provider to fetch an MMS picture that goes out. Sent as an attachment with `nosniff` and `no-store`; any other token, bridge or file is 404. |
| Notifications | `GET /notws` | on unless the endpoint `enabled` is `false`. Conf endpoint is enabled, timeout 3600000 | origin, then session-cookie MAC, else `login_key` | the signed-in user's notification topics | WebSocket. An origin outside the CORS list is 403. Call signaling frames (`rtc`, the rtc feature) bind the socket to a leg only through `CallServices.bind#Connection` as the socket's user (the leg must be theirs and in the call); a frame of a leg the socket did not bind is refused (`notJoined`), each carries the claimed connection generation, which the media node fences, and an SDP or candidate grants nothing. At most 4 bound legs a socket, 128 KiB a frame. |
| Terminal | `GET /termws` | on when `tui` is compiled and the endpoint is not `enabled="false"`. Conf endpoint is enabled | origin, then session cookie | runs as that user; screens stay authorized; no extra permission | WebSocket bytes and one JSON resize. Counts against the terminal session limits. |
| XMPP WebSocket | `GET /xmpp-websocket` (RFC 7395, subprotocol `xmpp`) | off; with the `xmpp` feature, `xmpp_domain` and `xmpp_websocket_enabled` `true` | SASL inside the stream (no cookie, so no origin check): see [XMPP](#xmpp) | as the XMPP listeners | Exempt from the request deadline. Stanzas up to 256 KiB, 32 deep. A client (as a trusted proxy names it) blocked for failed sign-ins, or with 20 XMPP connections open, gets 429. |
| XMPP uploads | `PUT`, `GET`, `OPTIONS /xmpp-upload/{token}/{name}` | while the XMPP server runs | the token in the URL is the capability (XEP-0363), compared by its hash | none beyond it | A slot takes one PUT of the size it was given, within `xmpp_upload_max_bytes` and an hour. The token, the declared length and type are checked before any of the body is read; a second PUT while one is coming is 409 at once; the body is spooled as it arrives (no request worker or database connection held for a slow client) and never read past the slot's size (413), within `xmpp_upload_seconds` (300; 408), then stored with the slot's row locked so PUTs to two nodes cannot both write (the other gets 409); a PUT cut off leaves no file; served as an attachment (images, audio and video inline, not SVG) with `nosniff` and `Content-Security-Policy: sandbox`; CORS open. A chat file sent to an XMPP client gets such a URL too, derived from the file and `xmpp_upload_secret`, so anyone the URL is passed to can read that file. Exempt from the request deadline. |
| Git | `ANY /git/{*rest}` | on when `git` is compiled | see [Git](#git) | `AT_GIT_REPOSITORY` | Smart HTTP. Exempt from the request deadline. |
| LLM routes | `/a2a`, `/mcp`, `/llm/v1`, Assist streams, public cards | compiled with `llm`; several answer 404 while their flag is false | see [LLM, A2A, MCP, Assist, git, and DAV](#llm-a2a-mcp-assist-git-and-dav) | see that section | Registration is not gated again by the conf flag. A protocol that is off answers 404. |
| DAV | `ANY /dav`, `/dav/`, `/dav/{*rest}` | route always registered; server default enabled | see [DAV](#dav) | the signed-in user's own path | Exempt from the request deadline. |
| Well-known | `/.well-known/caldav`, `/.well-known/carddav` | always registered | OPTIONS and the redirect need no login | 301 to `/dav/` | Path only. |
| Screen fallback | any other path | on | per screen | per screen | Also the servlet chain: `/fop`, `/search`, `/dashboard`. |
| Status | `/status` | screen transition | client IP in the same loopback list as `/health`, compared with `getClientIp()` | that list | `getStatusMap()` with no argument, so CPU, OS, framework version, and datasource fields are omitted. |
| FOP | `/fop/*` | servlet on | same as the target screen | that screen | Renders the remaining path as `xsl-fo` and sends PDF. `filename` (else `saveFilename`) is the download name. A login redirect of the target is the screen response. |
| Search proxy | `/search/*` | servlet on; 404 unless the search backend is OpenSearch | session cookie, Basic, `api_key`, `login_key`, or `authUsername` / `authPassword` | `SearchRemote` | No user: 401 `WWW-Authenticate: Basic realm="Moqui"`, body `Login required`. Missing permission: 403. |
| Dashboard proxy | `/dashboard/*` | servlet on; 404 when `proxyTo` has no host, and 404 unless OpenSearch | same as `/search` | `DashboardRemote` | `http://${kibana_host}:${kibana_port}`. Default host `127.0.0.1`, port `5601`, so the servlet is mounted. An empty `kibana_host` turns the target off. |

`search_backend` defaults to `postgres`, so both proxies answer 404 on a default install after authentication. Each proxy's `max-body` is `100m`. Hop-by-hop headers are stripped, and so are `host`, `authorization`, `cookie`, and `content-length`. The client's `Authorization` and cookies are not forwarded. Upstream Basic is the servlet's user and password init-params (`elasticsearch_user` / `elasticsearch_password` for search; the dashboard init-params for Kibana). The proxy timeout is 120 seconds. HTTP request spans are recorded by the factory-scoped telemetry middleware.

There is no axum route for `/groovysh`, `/rpc`, `/elastic`, `/kibana`, `/htmlr`, `/vapps`, or `/h2`. `/fop`, `/search`, and `/dashboard` are servlet patterns on the fallback. `/status` is a screen transition. The conf has no `/groovysh` endpoint, and the obsolete Groovy Shell screen has been removed. The browser's Terminal UI runs the applications; the privileged SSH admin shell provides the JavaScript executor ([SSH and the TUI](#ssh-and-the-tui)).

## Screen tree

The root screen `component://webroot/screen/webroot.xml` sets `require-authentication="false"` so subscreens decide. The default subscreen item is `qapps`. There is no `vapps` item. `toolstatic` points at the tools screen of that name, which is `anonymous-view` and is the Swagger UI static mount. Schema documents are the REST transitions, which require a user and `REST_SCHEMA`.

Because the root screen does not require a user, static files served as that screen's content are reachable without a login. The shell references `/js` and `/libs`, and the root names `/favicon.ico`. A full listing of those files was not made.

`qapps` is `require-authentication="false"` with extra path allowed, and its pre-actions redirect to `/Login` when there is no `userId`. The Assist drawer flag is `ec.llm.isEnabled()` and `hasPermission('AgentUse')`. `terminalUrl` is `/qapps/Terminal` only when `webapp_terminal_endpoint` is set.

### Public and weakly authenticated root paths

| Path | Authn in the screen | What it does |
| --- | --- | --- |
| `/status` | IP list, via `getClientIp()` | Process stats, sensitive fields omitted. See the host table. |
| `/menuData` | the script does not check `userId`; `getMenuData` does | `getMenuData` is 401 `Authentication required` with no username, 404 for an unknown screen, and 403 when view is not permitted (`screen/menu.rs`, in-process). The transition script reads `sri.getScreenUrlInfo().extraPathNameList` first. An anonymous `GET /menuData/apps/tools` in the client suite was HTTP 500 (`cannot read property 'extraPathNameList' of undefined`) before that check. The HTTP proof only requires that the body is not the Tools menu. |
| `/shellData` | same split | 401 `Authentication required` when there is no user id (`screen/shell.rs`). |
| `/shellSearch` | same split | 401 `Authentication required` when there is no username. Parameters `text`, `groups`, `limit`. |
| `/extItems` | same split | The declared actions and panels of the screen at the path (`screen/ext.rs`): 401 with no username, 404 for an unknown screen, 403 when view is not permitted. A target the user may not view, and an item whose provider hook the user has none for, is left out; the target checks access again when called. |
| `/WebPush/status`, `/WebPush/store`, `/WebPush/delete` | none in the screen; `store` and `delete` need a user | The browser's Web Push subscription of the session's user only (`service/web_push.rs`): `store` is 401 without a user and checks the endpoint is https and the keys a P-256 point and 16 bytes; an endpoint stored for another user moves to this one. `delete` touches only this user's row, with no error for another's. Posts need the session's CSRF token. The endpoint is a capability and is not logged. |
| `/ChatKeys/device`, `devices`, `rename`, `revoke`, `whoami`, `upload`, `query`, `claim`, `send`, `sync`, `storeKey`, `signingUpload`, `signaturesUpload`, `backupVersion*`, `backupKeys*` | 401 when there is no `userId` | The browser's crypto devices ([Chat keys](../interfaces/Chat-Keys.md); `ChatKeyServices` with authorization disabled, each in its own transaction). A request names a `deviceId`, which must be the session user's and not revoked (another user's is `notFound` with the same parameters). `query`, `claim` and `send` reach only the user's own devices and those of users who share a room or a live call; others are left out. Bounds: 50 devices a user, 200 one-time keys an upload and 500 held, 1,000 users a query, 1,000 devices a claim or send, 64 KB a message, 5,000 waiting a device (more are dropped and logged). The server stores public keys and ciphertext; it does not read message content. Posts need the session's CSRF token. C7: `storeKey` answers a device's store passphrase only to its own signed-in user (derived under the generated secret `chat_store_key_secret`, not stored, never in another answer; a device from before still has its random key as `legacyStoreKey` until its browser moves it to a new device); a cross-signing reset, a new or deleted backup, deleting backed-up keys, a new backup public key and a revoke of a device with keys need a sign-in in the last five minutes (`reauthRequired`, [Recent sign-in](Recent-Auth.md)); a claimer gets at most 10 of a device's one-time keys a minute (then its fallback key), and retries of one send at once queue one copy; the cross-signing and signature uploads write only the caller's keys and signatures (another user's master key only by the caller's user-signing key, in reach), never a silent reset of a master key (`reset` is the user's explicit choice); the key backup endpoints read and write only the caller's own backups (opaque `session_data`, 16 KB each, 1,000 a put, 100,000 a user). The server verifies no signature: trust is the clients'. |
| `/sw.js`, `/manifest.webmanifest` | none | Static files of the PWA: the service worker (served `no-cache`, `Service-Worker-Allowed: /`), which caches only versioned shell assets and never an authenticated answer, and the manifest. |
| `/listViewColumns` | no `userId` check in the transition | `get#ListViewColumns` with authorization disabled. `values` is sliced to 500. The comment calls it status configuration read without authorization. |
| `/statusBar` | no `userId` check in the transition | `get#StatusBar` with authorization disabled. |
| `/shellCounts` | 401 when there is no `userId` | then `get#ShellCounts` with authorization disabled. |
| `/userStatus`, `/setUserStatus`, `/userStatusHeartbeat`, `/userStatuses`, `/chatTyping`, `/chatAssistCard`, `/chatUnread` | 401 when there is no `userId` | Chat and status services with authorization disabled, for the current user. `userStatuses` passes up to 500 ids from `userIds`. |
| `/richTextImage` POST | 401 when there is no `userId` | `upload#RichTextImage` with authorization disabled. Multipart field `contentFile`. The GET transition is `read-only`; its body was not read past the tag. |
| `/email/{emailMessageId}` | none; the id is the capability | `viewEmailMessage()` sends a 1x1 PNG (`image/png`, no charset, `Content-Disposition: inline`) for every id. A known `EmailMessage` whose status is not `ES_VIEWED` is updated to `ES_VIEWED` with `receivedDate` set, through `update#moqui.basic.email.EmailMessage` with authorization disabled. A suffix from the first `.` is removed when that `.` is past the first character (`NOTAREALID.png` marks `NOTAREALID`). A missing message is logged. A failure while marking is logged and the response stays the PNG. |
| `/qapps/clientConfig`, `/qapps/clientDevices`, `/qapps/revokeClientDevice` POST, `/qapps/renewClientDevice` POST | 401 when there is no `userId` | The desktop client's settings (what `/qapps` writes in its page, with no session token), and the user's own device keys and one revoked (`get#UserDeviceKeys`, `revoke#UserDeviceKey` with authorization disabled; a key not the user's, or not a device's, is 400). `renewClientDevice` (`renew#UserDeviceKey`) gives a new key, once, for the device of the request's own `login_key` header, with its name and a full `device-expire-hours`, and ends the old key ten minutes later; a request not signed in by a device key of the user's is 400. The client calls them with its `login_key` header (plans/Mobile-Desktop.md). |
| `/qapps/registerClientPush` POST, `/qapps/unregisterClientPush` POST | 401 when there is no `userId` | The mobile app's FCM registration (`register#PushDevice`, `unregister#PushDevice`, authorization disabled; `service/web_push.rs`): `register` is 400 unless the request was signed in by an unexpired device key of the user's (its `login_key` header), and the row keeps that key's hash, so the key's revoke deletes it and its renewal moves it. A token stored for another user moves to this one; `unregister` deletes only this user's. The token is a capability and is not logged. |
| `/sso/appProviders`, `/sso/appLogin`, `/sso/appCode`, `/sso/appCodeConfirm` POST, `/sso/appToken` POST | none in the screen; see the right column | An installed app's SSO sign-in (RFC 8252, `service/sso_app.rs`): the AuthFlows offered; a start with an S256 challenge, a state and a redirect URI that is one of `webapp_app_redirect_uris` exactly (400 otherwise), kept in the session 15 minutes; after the provider's callback signs the browser in, a page asking the user to let the app in, whose confirmation needs the session token and gives a single-use code (hashed, a minute) to the redirect URI; `appToken` (no session token) takes the code with its PKCE verifier and redirect URI once, for a device key, logging no session in or out. A failure after the redirect URI was checked goes back to it with `error=`. |
| `/resources` | 401 when there is no `userId` | `ElFinderServices.run#Command` with authorization on: `ResourceCommandServices` (`SecurityTypeData.xml`) grants it to `ADMIN` only. Reads and writes the `resourceRoot` it is given (`component://`, `file:` included); in production only `dbresource://`, `content://` and `git://` roots take writes. `read-only`, GET or POST; `cmd=file` sends the file, `cmd=upload` takes multipart. |
| `/robots.txt` | none | Disallows `apps`, `qapps`, `rest`, `status`, `menuData`, `shellData`, `shellSearch`, `shellCounts`, the user-status and chat paths, `richTextImage`, `resources`, `llm`, `a2a`, `mcp`. |
| `/favicon.ico` | none | The file under the webroot directory. |

Other root screens that do not require a user: `Login` (`false`), `ChangePassword` (`false`), `SecondFactor` (`anonymous-all`), `echopath` (`false`, standalone, echoes extra path names), the `error` screens (`false`), `emailOAuth` (`false`), `sso` (`false`, compiled with the `sso` feature, which is on enterprise and maximal), `PublicShell` (`false`, chrome for token routes), `assist` (`false`), and `rest` (`false`). `apps` is `false` so its subscreens decide.

`Login` transitions include `login` (`require-session-token="false"`), `logout`, `removePreAuth`, `resetPassword`, `changePassword`, `createInitialAdminAccount`, and `sendOtp`. A user who is already signed in is redirected to `/`. `returnTo` is kept only when `WebUtilities.isSameOriginRedirect` accepts it (`Login.xml`). That helper allows a path that starts with `/`, and an `http` or `https` URL whose host equals the configured host. It refuses `javascript:`, `data:`, `vbscript:`, a leading `//`, a backslash, user info, and control characters. The port is not part of the host compare (`web/mod.rs`). An off-site value is logged and ignored. The initial-admin transition is the service described under passwords: it runs only while no real `UserAccount` exists beyond the seed row.

`apps` transitions: `setPreference`, `setPrefGoLast`, and `getPreferences` are 401 `Authentication required` without a `userId`. `setPreference` and `setPrefGoLast` are 403 for an appearance key (`QUASAR_DARK`, `SHELL_DENSITY`, `THEME_*`) that the `STRT_APPEARANCE_POLICY` of the user's Quasar theme, after its `STRT_APPEARANCE_SERVICE` services, locks; `ec.user.setPreference` itself refuses it, so another caller gets the error too. `l10nCatalog` and `qzSign` are 401 without a `userId`. `colorSchemeCss` is `read-only` and `require-session-token="false"`. The comment says a scheme is not private, so there is no login. It calls `org.moqui.impl.ShellServices.get#ColorSchemeCss` (Rust, `screen/sri/appearance.rs`), which loads `ScreenColorScheme` and colors with authorization disabled. A scheme name must match `^[a-z][a-z0-9-]{0,40}$`. A color value is rejected when it matches `[;{}<>\\"'!]`. `scheme.extraCss` is appended after that check. It is not passed through the name or value checks. The response is `text/css`.

`apps` subscreens that were read: `AppList` and `ScreenTree` and `Terminal` require authentication. `ScreenTreeNested` sets `require-authentication="false"` and is a child of `ScreenTree`. Whether a direct URL's render chain always includes the parent was not traced. See [Open questions](#open-questions).

### Tools and System

`runtime/base-component/tools/MoquiConf.xml` mounts `system` and `tools` under `apps`. The URLs are `/apps/system` and `/apps/tools`, and the same names through the qapps extra path. There is no `/vapps` mount. `Tools.xml` and `System.xml` omit `require-authentication`, so both are required. `ADMIN` has inheritable always-all on each root, from the seed table above.

Tools' SQL, Service Run, Data Load, Data Snapshots, and `qzSign` are designed privileged tools. The elFinder screen was removed (Dev-AIDE replaces it); its commands are `/resources` on webroot. A screen-by-screen re-check of write refusals (the rest of the Java tool table) was not done. The extra permission checks that were read, on top of screen artifact authorization:

| Screen | Check |
| --- | --- |
| `Tools/Sql.xml` (a statement, or a script) | `SQL_RUNNER_WEB` |
| `Tools/Service/ServiceLoadRunner.xml` | `SERVICE_LOAD_RUNNER` |
| `Tools/dashboard.xml` | shows SQL and service load runner links only with their respective permissions |
| `System/Llm.xml` | `AgentAdmin`, or the screen returns an error. The comment says every screen under it needs `AgentAdmin`. |
| `System/Security/UserAccount/UserAccountDetail.xml` | `ADMIN_LOGIN_AS` for login-as, and a different username |

### Component deploy and conf reload

System › Components and System › Conf ([Component-Deploy](../tools/Component-Deploy.md)), with the services in `org.moqui.impl.ComponentServices` (`crates/moqui-devkit/src/deploy/`). Each is `allow-remote="false"`, under the `SYSTEM_APP` screen authorization, and each checks the `ADMIN` group itself.

- **What a load can do.** Loading a component, from a component directory or from git, runs its code on the server: its services, ECA rules, screens, REST APIs, templates and scripts are loaded, its data is loaded with authorization off, its tables are made, and its `MoquiConf.xml` is merged into the running conf. Treat a load as a code deploy. A component with Rust code (a `Cargo.toml`, or `.rs` files in `src/`) is refused: native code loads only at a start, when the binary links it.
- **Deploy from git** (`preview#ComponentDeploy`, `deploy#Component`, `update#Component`) also needs the sealed `COMPONENT_DEPLOY` permission. On a production instance it needs `component_deploy_enabled=true` too, and without that it is refused with a vague error.
- **URLs.** The URL must be `https` or `ssh` (also `user@host:path`), never `file://` or a local path. On a production instance a host that resolves to a loopback, private or link-local address is refused.
- **Fetching.** The database and bare storages fetch in-process (`moqui-git`). A working clone runs the system `git` command, with no prompt and the access token sent as an HTTP header rather than kept in the clone's conf. Both paths are refused in sim mode and by the `RemoteCall` fence. Credentials are stored encrypted (`GitRepository.accessToken`, or the `DeployedComponent` row for a clone).
- **Files written.** Fetched files go to a dot-prefixed staging directory that discovery skips, then into `runtime/deployed-component/<name>` or the clone's component directory. Remove moves the directory to `.removed/` beside it; a working clone with changes asks again first.
- **Restart.** "Deploy and restart", "Update and restart" and System › Conf's Restart Now go through `restart#Server`: a dev instance only, `DEV_AIDE` or `ADMIN`, and a server that can restart itself.
- **Conf reload** (`reload#Conf`, System › Conf, the dev file watcher, and `SIGHUP` when stdin is not a terminal) applies only the units the catalog marks live or apply. Ports, datasources, keys, `instance_purpose` and the other restart-class units wait for a restart. A property that the environment set is not overridden.
- **Cluster.** Only the database storage is fanned out, on the `component-deploy` topic (see Cluster below), and each member writes the tree from the shared database repository. The other storages, and a load from a local directory, are refused while the server has live cluster members.
- **Audit.** Each load, deploy, update and remove writes a `DeployedComponentHistory` row and a WARN on `moqui.monitor.audit`, as does a data load again (its history row only for a component a load recorded) and each conf reload request.

### Token routes

Token routes are not framework-default paths. `runtime/component/mr-boss/MoquiConf.xml` adds them on the `webroot` webapp, and `runtime/base-component/basic-comms/MoquiConf.xml` adds `call`. `screen/mod.rs` sends a match to token-route dispatch before the ordinary screen. The failure status of a bad token was not read.

| Name | URL | Token service | Screen |
| --- | --- | --- | --- |
| `s` | `/s/<token>` | `org.moqui.impl.FileServices.check#PublicShareToken` | `mr-boss` `PublicShare.xml` |
| `u` | `/u/<token>` | `mantle.party.ConsentServices.check#UnsubscribeToken` | `Unsubscribe.xml`, post-transition `oneClick` |
| `cal` | `/cal/<token>` | `ironstone.commons.CalendarServices.check#PublishedCalendarToken` | `PublishedCalendar.xml` |
| `r` | `/r/<token>` | `mantle.request.RequestServices.check#IntakeToken` | `RequestIntake.xml` |
| `call` | `/call/<token>` | `org.moqui.impl.CallServices.check#CallLinkToken` | `basic-comms` `CallGuest.xml` |

mr-boss also adds a per-client rate limit `request-form` on `POST /r/*/submit`: 5 requests in 60 seconds, burst 5. It applies when per-client web protection is on.

`/call/<token>` is a call link's guest page. Its token is 32 random characters; only its hash is stored. The token is
refused once the link is revoked or expired. The page's transitions are `joinGuest`, `guestState` and `leaveGuest`,
without the session token. `joinGuest` takes a name of up to 60 characters and gives a 40-character credential that
lasts 4 hours (hash kept). basic-comms rate-limits it per client (`call-guest`: 10 a minute, burst 5).

- A guest's credential reaches only that call's signaling, on `GET /callws`. It is never in a URL.
- `guestState` shows a waiting guest nothing but their admission. Once admitted, a guest sees the names of those in the
  call. They never see the room, its members or its history.
- A guest leg can negotiate media only once admitted (`bind#GuestConnection`).
- Revoking the link, or removing or denying the guest, ends the credential and closes the guest's legs.
- A link made end-to-end encrypted (W8) takes no guest (`encryptedNoGuests`): a guest has no crypto device. The page says
  so from the token's route context (`encrypted`).

Call captions (W6, [WebRTC](../interfaces/WebRTC.md#captions)) send call audio out to a speech provider, so they are off
unless `call_captions_enabled` is `true`. `start#Captions` and `stop#Captions` (the Calls screen's `startCaptions`,
`stopCaptions`) need a signed-in user with an answered leg in the call; anyone else gets `notFound`. An end-to-end
encrypted call is refused. Every leg, a late joiner's and a guest's too, gets the notice naming the provider before its
audio is captioned. The provider is the `transcribe` model binding, reached through `moqui-llm`'s address guard (a
private address needs `allow-private-network`), counted against the quota of the user who turned captions on, and kept
as an `LlmCall`. `org.moqui.impl.SpeechServices.transcribe#Audio` and `get#SpeechProvider` are `allow-remote="false"`:
`transcribe#Audio` reads any `audioLocation` it is given, so its caller decides what may be sent out. Captions are not
stored unless `call_captions_store` is `true`.

## REST

`rest.xml` is `require-authentication="false"` so each transition decides. It does not grant inheritable admin authorization. Rust bodies for this screen are in `screen/dispatch.rs`. A same-named transition on another screen is not wired there.

| Path | Default | Authn | Authz | Considerations |
| --- | --- | --- | --- | --- |
| `POST /rest/login` | on | `require-session-token="false"`. `username` and `password` required, optional `code` | the login | MFA leaves `moquiAuthcFactorRequired` on the session and returns factor JSON, or completes with the code. |
| `POST /rest/sendOtp` | on | session token required. Needs the pre-auth session. Parameter `factorId` | the send service | |
| `POST /rest/verifyOtp` | on | session token required. 401 if there is no `moquiPreAuthcUsername`. Parameter `code` | the validate service | |
| `/rest/logout` | on | logs out when `ec.user.username` is set | none | Text `successful`, or `no_user`. |
| `/rest/e1`, `/rest/v1` (same as e1, deprecated), `/rest/m1` | on | 401 `Authentication required for entity REST operations` when login failed or there is no username. Empty extra path: 400 | `AT_ENTITY`. Tarpit off for entities | Path, query, JSON body, JSON list body. |
| `GET /rest/entity.json`, `/rest/master.json`, `/rest/service.swagger` | on | 401 `Authentication required for REST schema` | `REST_SCHEMA`, else 403 `Not authorized for REST schema`. Seeded on `ADMIN` | Path selects the entity or API root. |
| `GET` `entity.raml`, `master.raml`, `service.raml` | transition exists | the Rust body returns before a schema check | none | 404 text `RAML is not built`. |
| `GET` `entity.swagger`, `master.swagger` | transition exists | same | none | 404 text `Swagger is not built`. |
| `/rest/s1/{root}/...` | on | per resource. Empty `authenticate` or `"true"` requires a user | `AT_REST_PATH` on each path node. Authz and tarpit both on | The tools seed covers `/moqui` for `ADMIN` and `/moqui/git` for `ALL_USERS`. Other roots were not listed. |
| `/rest/sm/{type}/{remote}/{id?}` | on | `require-session-token="false"`, `begin-transaction="false"` | the remote's `messageAuthEnumId` | Only a configured type and remote. Otherwise 400. Reply `{systemMessageIdList}`. |

`/rpc/json` is not a screen and not a host route. There is no `webroot/rpc.xml`, so the path is 404. `/rest/api_key` and `/rest/moquiSessionToken` are comments, not transitions. `/rest/userInfo` sits inside a comment, so it is not live.

Entity REST (`entity/query_rest.rs`): a find of a restricted generic entity is unauthorized. Writes of identity-admin entities and of `UserAccount` are refused. Secret-config entities are writable only when the caller is in `ADMIN`. Results pass through `strip_user_account_secrets`. The secret names are `currentPassword`, `resetPassword`, `passwordSalt`, `passwordHashType`, and `passwordBase64`. `pageNoLimit` is removed from the parameters. `entity-facade` `rest-max-page-size` is 1000. The page size applied after `pageNoLimit` is removed was not read.

System-message receive:

| `messageAuthEnumId` | What is required |
| --- | --- |
| empty or `SmatLogin` | a logged-in user, then authorization of the receive service. No user is 401 or 403 |
| `SmatHmacSha256` | header HMAC-SHA256, Base64, then anonymous login |
| `SmatHmacSha512` | header hex HMAC-SHA512 of the body (either case, an optional `sha512=` prefix: Authorize.Net's `X-ANET-Signature`), then anonymous login. No timestamp or replay window; the receive service keeps each event once (`store#ReceivedSystemMessage`) |
| `SmatHmacSha256Timestamp` | `t` / `v1`. Skew 10 seconds, max age 300 seconds. Replay cache `moqui.security.hmac.replay`, type `distributed`, live 300 seconds; a key is given back when the receive service fails (the message is not kept), so the provider's retry of that delivery is taken |
| `SmatUrlToken` | exactly one query parameter (`authHeaderName` names it, else `token`: a remote switched from an HMAC scheme must change or clear its `authHeaderName`; the token must be URL-safe, as `+` decodes to a space) equal to `sharedSecret`, compared in constant time (Shippo's self-generated webhook URL token), then anonymous login. No signature over the body and no replay window; the token is in the URL, so it can reach a proxy's access log. An empty secret refuses every request |
| `SmatNone` | anonymous login, no check |

## LLM, A2A, MCP, Assist, git, and DAV

Routes exist when the `llm` feature is compiled. While a protocol is off, its handlers answer 404. `llm-facade` `enabled` is `true`. That element is the model facade. It is not the A2A server, the MCP server, or the gateway. Assist answers 503 when the facade is disabled; `bootstrap` still answers (`docs/ai/Assist.md`).

A user with `EXTERNAL_USER` never holds `A2aServerUse` or `AgentUse`. The Assist contract says that permission also blocks the other agent permissions, so that user starts no run and gets no model call.

### A2A

Off. `a2a_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/a2a` and `GET /.well-known/agent-card.json` answer 404.

From `docs/ai/A2A.md`: a caller needs `A2aServerUse` and `AgentUse`. Credentials are a Bearer login key, an `api_key` or `login_key` header, or Basic with an app password or a login key as the password. The account password is refused (401). Credentials are taken only over TLS, a believed proxy's https, or loopback. Missing credentials are 401 `WWW-Authenticate: Bearer realm="a2a", Basic realm="a2a"`. No permission is 403. The body max is 4 MiB. Waits and streams are at most 256 on a node (503) and 16 per user (429). Every request except the public card needs `A2A-Version: 1.0`. OIDC is not built. An agent is reached only when `exposeA2a="Y"` and the surface includes `a2a`. The seed `assist` agent does not. Writes wait for the human owner.

### MCP

Off. `mcp_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/mcp` and its documents answer 404.

From `docs/ai/MCP.md`: a caller needs `McpServerUse` and `AgentUse`. Credential shapes match A2A, with realm `mcp`. No credentials is 401 JSON-RPC `-31401`. No permission is 403 `-31403`. `server/discover` needs no credentials. The body max is 4 MiB. The transport is `POST /mcp` only; GET and DELETE are 405. The seed agent `mcp` serves `mcp` with `exposeMcp="Y"`. The MCP client cannot approve a write. Public when on, and cached for an hour: `GET /mcp/server-card`, `/mcp/setup`, `/.well-known/mcp.json`, `/.well-known/ai-catalog.json`.

### Gateway

Off. `llm_gateway_enabled` is `false`. The live element is `<gateway enabled="${llm_gateway_enabled}" require-client="true" timeout="300"/>`. An `enabled="true"` example above it in the conf is inside a comment.

From `docs/ai/Gateway.md`: Bearer login key or app password, or Basic with those. The account password is refused. TLS, a believed proxy's https, or loopback. Missing or bad credentials are 401 `invalid_api_key`. The caller needs `LlmGatewayUse` and an `LlmApiClient` row. An app password always needs a client. A login key needs one because `require-client` defaults to true. The body max is 16 MiB. The path is exempt from the webapp request deadline. The gateway `timeout` of 300 seconds answers 504. Paths: `GET /models`, `GET /models/{alias}`, `POST /chat/completions`, `POST /responses`, `POST /embeddings`. An unknown alias is 404. No tools run on the server: function-tool definitions pass through, and any other tool type is 400 `unsupported_tool`.

### Assist

The drawer is screen transitions on `assist.xml` (`require-authentication="false"`), not host routes. Each transition calls the Assist service. GET reads: `bootstrap`, `conversations`, `conversation`, `tasks`, `prompts`, `mentions`, `attachment`, `memories`, `skills`, `events`, `read`. POST: `turn`, `cancel`, `retry`, `follow`, `context`, `submitted`, `decide`, `preview`, `answer`, `feedback`, `queued`, `attachments`, `transcribe`, `textAction`, `memory`, `skill`, `canvasState`, `canvasAction`, `canvasAdjust`, `canvasProblems`.

`docs/ai/Assist.md` is the contract for those calls. The browser session cookie is the login. Every call acts for the current user and needs `AgentUse`. Another user's conversation, run, approval, question, hand-off, or message is 404. A call from inside a run is 403. POST requires `X-CSRF-Token` or `moquiSessionToken`, and a missing token is 401 `Session token is missing or is not valid` before the call. The qapps shell includes the drawer when the llm feature is on and the user has `AgentUse`. Per-call tool policy inside the Rust services was not read.

`GET /assist/runs/{run_id}/events` and `.../events.json` authenticate like `/notws` (origin, session cookie, else `login_key`). The stream is for the run's owner. At most 256 streams; one more is 503 with `Retry-After` (`docs/ai/Assist.md`).

`GET /assist/sandbox` is public and has no session. The host must be a plain name, or the answer is 400. Query `v` is a cache buster. `x=1` adds `https:` to `img-src` only. The iframe is `sandbox="allow-scripts"` without `allow-same-origin` (`docs/ai/Assist.md`). The policy is the frame document, separate from the screen CSP:

```
sandbox allow-scripts; default-src 'none';
script-src <origin>/libs/vue/ <origin>/libs/quasar/ <origin>/js/WebrootAssistSandbox.js 'unsafe-eval';
style-src <origin>/libs/ 'unsafe-inline'; font-src <origin>/libs/;
img-src data: <origin>/images/ <origin>/libs/;
connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none';
manifest-src 'none'; form-action 'none'; frame-ancestors 'self' <origin>; base-uri 'none'
```

Response headers from that doc: `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, a `Permissions-Policy` that turns features off, and no `X-Frame-Options`. `'unsafe-eval'` is in this frame policy. It is not in the default screen CSP.

### Git

`ANY /git/{*rest}` when `git` is compiled. `git-server` defaults: `allow-http="false"`, `max-concurrent="4"`, `max-queued="16"`, `max-request="67108864"`, `max-push-size="2147483648"`, `max-object-size="536870912"`, `idle-timeout="120"`, `lfs-max-object-size="5368709120"`, `webhook-private-network="false"`. The route is exempt from the webapp request timeout.

From `docs/interfaces/Git-Hosting.md`: HTTP Basic with the username and the password, or an access token or an app password as the password. An account with a second factor needs the token or the app password. The account password cannot finish that login. `Authorization: Bearer` and the `api_key` header also work. No credentials is 401. Credentials are accepted only over HTTPS (the TLS listener, or `X-Forwarded-Proto: https` from a believed proxy), from loopback, or when `allow-http="true"`.

Authorization is artifact type `AT_GIT_REPOSITORY`, name `repositoryId`. The server refuses to serve when that type's authorization is not enabled. VIEW is clone and fetch. CREATE is a new branch or tag. UPDATE is a fast-forward. DELETE is a force-push, a deleted ref, or settings. No view is 404. View without push is 403. Only `refs/heads/` and `refs/tags/` can be pushed. The default branch cannot be deleted. Seed `GitRepositories` is always-all for `ADMIN`. A `remote` repository is a read-only mirror. LFS is under `/git/<repositoryId>.git/info/lfs`. Webhooks are not sent to loopback, private, or link-local addresses unless `webhook-private-network="true"`.

SSH git is a channel on the SSH server, not this route. See [SSH and the TUI](#ssh-and-the-tui).

### DAV

The routes are always registered. There is no `<dav-server>` element in `MoquiDefaultConf.xml`. The defaults are the Rust fallbacks, and `docs/interfaces/Sync.md` states the same numbers.

| Flag | Default | Effect |
| --- | --- | --- |
| `enabled` | true | `false` is 404 `the DAV server is off` |
| `allow-http` | false | otherwise credentials need a secure request: HTTPS, a believed proxy's https, or loopback |
| `account-password` | false | app password only. `true` also accepts the account password |
| `max-file` | 268435456 (256 MiB) | 413 over that body |
| `sftp-drives` | true | SFTP to drives for a user without `SSH_SFTP` |

OPTIONS is answered with no login: `DAV: 1, 2, 3, calendar-access, addressbook, extended-mkcol`, and `Allow` lists `OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, REPORT`.

Every other method is HTTP Basic. No credentials is 401 `WWW-Authenticate: Basic realm="Moqui", charset="UTF-8"`. Plain HTTP while `allow-http` is false is 403 `credentials over plain HTTP are refused; use HTTPS`. A path whose user segment is not the signed-in username (case-insensitive) is 404. Writes go through the file and adapter services as that user. `docs/interfaces/Sync.md` paths: `/dav/`, `/dav/principals/<user>/`, `/dav/files/<user>/<drive>/<path>`, `/dav/calendars/...`, `/dav/addressbooks/...`. The framework keeps no calendar or contact of its own. An application serves them through adapter services, and Mantle sets those adapters.

`login_user` does not call `finish_login` when a second factor is required. DAV requires that login to succeed and requires a non-empty current user id. The wrapper that maps a second-factor result onto the current user was not read.

## Non-HTTP listeners

`/metrics` is on the public HTTP listener. It is not its own port. `port_check` probes HTTP, HTTPS, cluster, and SSH before the database opens. It does not probe SMTP.

### Mail

There is no open SMTP receiver by default. `smtp_listen_port` empty does not start one. The host default is `127.0.0.1`. There is no default port (the doc example is 2525). Only internet mode (`smtp_internet=true`, or `internet="true"` on the `smtp-server`, which MoquiEdgeConf.xml sets) takes mail from anyone, on port 25 of an edge node; see [Edge node](#edge-node) below. `inbound-smtp` is an Enterprise and Maximal feature. Dev and Lite omit it. If the port is set on a build without the feature, startup logs an error and does not listen.

The listener is the last hop behind an operator-run gateway (`docs/interfaces/Mail-Inbound.md`). There is no SMTP `AUTH` and no client certificate. The allow-list is the gateway. A peer outside `smtp_allowed_addresses` gets `554` at connect and the connection closes. An empty allow-list refuses to start the listener. `proxy-protocol`, `starttls`, and `require-tls` default to false. STARTTLS with no certificate does not start the listener. `dsn` defaults to false. The message cap is 25 MiB (`max-message-size` 26214400), 100 recipients, 20 connections. In internet mode no allow-list is needed: STARTTLS is on by default, each address is limited (`max-connections-per-ip` 5, `connections-per-minute` 30; IPv6 by its /64), and SPF, DKIM, DMARC and ARC are checked after the data within 45 seconds, with the listener's own `Authentication-Results` written and any claiming its authserv-id removed; a DMARC reject policy is refused (550). Rspamd's verdict, when `mail_rspamd_url` is set, fails open ([Mail-Inbound](../interfaces/Mail-Inbound.md)).

A permitted gateway can `RCPT TO` a hosted (`EakHosted`) or shared (`EakShared`) address that has no incoming IMAP or POP server. Other recipients are `550`. After `DATA`, the message is stored in that account's Inbox, or Junk when a configured spam header starts with yes or true. That store runs with authorization disabled, including the app `EmailMessage` ECA when Mantle is loaded. There is no relaying. `VRFY` answers `252`. Empty `smtp_authserv_id` keeps every `Authentication-Results` header, including a forged one, and warns at startup. Out of internet mode the listener does not check SPF, DKIM, or DMARC; the gateway does.

`poll#EmailServer` (job `poll_EmailServer_frequent`) is paused. If unpaused, the cron is every 15 minutes, `emailServerId=SYSTEM`, `authenticate="anonymous-all"`. It connects out to the configured IMAP or POP server. `sync#DueEmailAccounts` (job `sync_EmailAccounts_frequent`) is running, every minute, `authenticate="anonymous-all"`, `allow-remote="false"`. IMAP IDLE is on by default (`idle-enabled="true"`, `idle-max` 8) and dials the account's server. It does not listen. Both jobs use the scheduler's authorization-off path.

OAuth mail sign-in is not a listen port. It stays off until the operator sets a Google or Microsoft client id and secret. The browser session, PKCE, and server-side state (10 minutes, single use) protect the callback `/emailOAuth/callback`. The redirect URI is the deployment's and is never taken from the request. A server with no field encryption refuses to store the refresh token. The ID token signature is not checked. The callback transition's body was not fully read.

### SSH and the TUI

The SSH server is one `TcpListener`. SFTP, git, the admin shell, `apps`, `agent-apps`, and the SSH cluster channel are channels on it. `moqui tui` with no `--serve` and no `--server` opens no port (`docs/ui/TUI.md`).

Off in the default conf: `ssh_port` empty, `ssh_host` `127.0.0.1`. `0` binds an ephemeral port. `./moq run` merges the dev conf, which sets `ssh_port` to `2222`, still on loopback unless `ssh_host` changes. `MoquiProductionConf.xml` does not set `ssh_port`. The `ssh` feature is in Dev, Enterprise, and Maximal. Lite leaves it out. `moqui tui --serve` starts this listener together with HTTP and the cluster listener.

Password login uses `login_user` (lockout counts, `ipAllowed` of the account and its groups). An account that needs a second factor is refused at the SSH password step. An app password is tried first when the secret has that shape. A public key must match an active `UserSshKey` by SHA-256 fingerprint, then `internal_login_user` with no password. Keyboard-interactive (`login-dialog` default true, and only if `tui` is built) asks no questions: the session has no user until the apps login dialog, the same username, password, and second factor as `moqui tui`. That dialog checks `ipAllowed` and closes after 2 minutes without a login or 5 refused logins. Pre-auth timeout is 30 seconds, and at most 16 connections have not logged in. The host key is `ssh_host_key`, generated into `runtime/.env`.

`SSH_SHELL` is seeded on `ADMIN_ADV` and is sealed, so the ordinary permission UI cannot grant it. A login with `SSH_SHELL` gets the admin shell (`help`, `status`, `service`, `entity` up to 100 rows, `cache clear`, `js` with `ec`, `apps`). Those commands run as that user. `service`, `entity`, and `js` also need Tools-app artifact authorization. Without `SSH_SHELL`, or after a dialog login, the session is only `apps`. `agent-apps` needs a key or password login and no pty. It drives the same client with that user's credentials. A dialog login gets no shell commands, no `agent-apps`, no SFTP, and no git. Git over SSH needs `SSH_GIT` (seeded on `ADMIN`) and a key, unless `git-password-auth` (default false). At most `max-sessions` 8 SSH connections. Terminal sessions also count against the terminal limits. Idle time is 10 minutes.

The SSH cluster channel is not a separate port. It is used when a peer's base URL is `ssh://`. The default transport is HTTPS, so the channel is off unless a member advertises SSH, and the SSH server must be listening. The login user is `moqui-cluster` with an Ed25519 key derived from `cluster_tls_key`. Any other key for that user is rejected. The channel carries the same cluster topics. It is not a shell.

### SFTP

SFTP is the SSH `sftp` subsystem. Nothing listens for it unless the SSH server is up. `docs/interfaces/Sync.md` "SFTP to the drives" is this subsystem. The outbound SFTP client dials other hosts.

`sftp-drives` defaults to true. `sftp-root` defaults to `runtime://sftp`. `sftp-max-file` defaults to 256 MiB. The user must already be logged in by key, password, or app password. A dialog login is refused. `SSH_SFTP` (seeded on `ADMIN`) selects the root: one resource location (`runtime://`, `file:`, `component://`, or `dbresource://`), paths normalized against `/`, and `..` that leaves the root is refused. Those calls run with authorization off. The permission checked at login is the gate. A user without `SSH_SFTP` gets the drives that user may open, when `sftp-drives` is true. `sftp-drives="false"` gives that user no SFTP. A write is an upload and a new version. A removal goes to the trash. At most 64 handles per channel.

### Cluster

The cluster listener is separate from the public HTTP and HTTPS listeners. Public routes do not ask for a client certificate.

On whenever the process starts listeners (a normal `moqui` with no command, and `moqui tui --serve`). Empty `cluster_https_port` is **8444**. Empty `cluster_bind_address` is **0.0.0.0**. `0` is ephemeral. Commands that do not serve do not start it. The default is 8444 (`crates/moqui-host/src/cluster_edge/mod.rs`).

A TLS client certificate is mandatory. The verifier accepts only a certificate whose SHA-256 matches this member's `cluster_tls_cert`. The server presents that same certificate. The handshake must finish within 10 seconds. There is no HTTP login and no public app routing. ALPN picks the protocol: `h2` (offered unless `cluster_protocol=rest`) serves only the gRPC `moqui.cluster.v1.Cluster.Post`, a batch of the same messages with a status for each, decoded up to 17 MiB, at most 64 streams per connection, with HTTP/2 PINGs and a connection idle for 60 seconds closed; `http/1.1`, or no ALPN, serves only `POST /cluster/{topic}` for one of the bus topics (`moqui_core::notify::TOPICS`): `entity-cache-invalidate`, `notification-message`, `collab-update`, `session-end`, `rtc-signal`, `runtime-settings` or `component-deploy`. The body must be JSON with `origin_id` and `message_id`, or the answer is 400. An unknown topic or method is 404. The serialized JSON envelope limit is 16 MiB on HTTPS, SSH and Postgres NOTIFY (including payloads kept in `ClusterMessage`). Oversized messages are refused before fan-out. A 4xx response refuses that message without retrying or marking the peer down. At most 64 connections. Cache clears that entity's one-cache. `session-end` drops this process's live copies of the named session ids. Notification and collab messages are published on the local bus and are not fanned out again. `rtc-signal` is sent to one member (`cluster::send_to`, one try from a queue of 1,024 that refuses when full): a call's signaling frames for the node holding its media, its replies for the node holding a leg's socket, the call services' close messages, and the voice services' SIP control (kind `sip`, for the node's SIP engine); a receiver drops one whose `target` is not its instance id (the notify transport reaches every notify member).

Postgres `LISTEN moqui_cluster` is off unless `cluster_transport=notify`. It needs a Postgres entity database. MySQL refuses startup. It is one connection outside the pool, not a new TCP port. The payload is not authenticated on its own. A payload over 7,900 bytes is a `ClusterMessage` row id. A notification missed while `LISTEN` is down is lost. Reconnect then clears entity caches. The default `cluster_transport` is empty, which means the cluster port at `cluster_base_url` (gRPC, or the HTTP/1.1 POST).

### Edge node

Off unless `node_role` is `edge` or `all-in-one` and the HTTPS listener runs, and only in a binary with the `edge` feature (Maximal, Enterprise). It is not a port of its own: the edge link shares the public HTTPS listener and tells its connections apart by the ClientHello, which the listener reads before it picks TLS settings (`crates/moqui-host/src/edge_link/`, [plan](../../plans/Edge-Node.md) §5.1, §5.3, §5.8). A local node opens no port; it dials out.

- **`moqui-edge/1`** (the up and down connections): the edge node presents a certificate from its own link key (`edge_link_key`) and asks for a client certificate without requiring it. After the handshake the key's SHA-256 must match an active `moqui.edge.EdgeClient`; a role byte follows, and a down connection must name the live link generation. The handshake to the role byte has 10 s. On up the local node calls the edge node's `Link` gRPC service; on down the edge node calls the local node, which serves only the link's own gRPC and the PASS authority's (on `/moqui.edge.v1.LocalLink/` and `/moqui.pass.v1.PassAuthority/`, with no client named), forwarded requests and tunnels. A revoked key, a stale generation and a version more than one apart are refused.
- **`moqui-edge-pair/1`** (checking and pairing): the web certificate, which the local node checks by WebPKI. A check connection (no client certificate) can only Probe; a pairing connection presents the new key and can Pair with a one-use code (kept as a hash, an hour by default, at most a day). Each connection stays at most 30 s, with at most 4 calls and 2 streams; Pair is limited to 10 tries per address in 10 minutes. A code does not take over a node that is active with another key.
- **Any other ALPN** gets the web certificate and is never asked for a client certificate.
- **Terminate hosts** (`moqui.edge.EdgeHost`, mode `EdgeHostTerminate`): a request whose `Host` is one goes down its node's link after the edge node's own web protection; only configured names go, and only to their own node. The edge node removes hop-by-hop headers and every forwarding header a client sent (`Forwarded`, `X-Forwarded-*`, `X-Real-IP`, its client-IP header, `x-moqui-edge-*`), then names the client in `x-moqui-edge-client`. The local node believes that header only on the down connection, removes it and the forwarding headers, and runs its own web protection on that client. A client's `CONNECT` is answered 405; only a WebSocket, an HTTP/1.1 upgrade or an HTTP/2 extended CONNECT with `:protocol` `websocket`, becomes an extended CONNECT down the link, and the edge node answers the browser 101 or 200 as its protocol has it. While the link is down the edge node answers 503 with an offline page. The edge node sees the content of these requests.
- **Passthrough hosts** (mode `EdgeHostPassthrough`): a ClientHello whose server name is one goes down the link unopened, with a PROXY v2 header naming the client; the local node ends TLS with its own certificate, so the edge node sees no content. A ClientHello is at most 64 KiB. While the link is down the connection is closed. A local node reads a PROXY header only on a tunnel from its link, never on a socket, and its web protection admits that address before the TLS handshake. A tunnel names a kind (`https`, `ssh`); the local node picks its own listener and the edge node never names an address. At most 768 tunnels and forwarded WebSockets at once per node.
- **Delegated calls, queue and replication** (the link's gRPC, [plan](../../plans/Edge-Node.md) §5.2): the edge node calls `LocalLink.Delegate` and `LocalLink.Deliver` on the down connection, and the local node calls `Link.Deliver` and `Link.Replicate` on the up connection; each is fenced by the link generation, and none is reachable by a forwarded request (the down connection takes gRPC only under `/moqui.edge.v1.LocalLink/` and `/moqui.pass.v1.PassAuthority/`, and only with no client named, which a forwarded request always has). Every message is at most 16 MiB both ways (tonic's decode and encode limits on both servers and clients); a queued item's payload is at most 1 MiB, refused at enqueue, and a replication batch at most 500 records and 4 MiB. On the edge node a delegated call is checked first: with the service's definition there, its authorization and `authenticate`; without one, a caller with no user is refused unless the `delegate` entry has `allow-anonymous="true"`. A delegated call runs on the local node only when the service is in the local node's own `<edge><delegate>` list (a compromised edge node could ask for anything), as the user `_EDGE_` (no password, so no login; group `EDGE_DELEGATE`, `UgtRemoteSystems`), with authorization: the `EDGE_DELEGATE` artifact group, which starts with only the diagnostic `echo#Delegate`, must grant it. No parameter the edge node sends changes that user: the local node drops `authUsername`, `authPassword`, `authUserAccount`, `authTenantId`, any other `auth` + capital name, every `_` name, and an `edgeCaller` the edge node wrote, then sets `edgeCaller` from the call's caller data (never a login). A queued item runs its kind's handler from the receiver's own `queue-kind` conf, also as `_EDGE_` with authorization, once per item id (`moqui.edge.EdgeReceipt`, inserted before the handler runs); a node enqueues only the kinds it lists. Replication writes only `moqui.edge.EdgeReplicaValue` and `EdgeReplicaCursor` rows of the sending node on the edge node, with the copied fields encrypted at rest; what a compromised local node sends there is data for its own node only.
- **PASS authority on the link** ([plan](../../plans/Edge-Node.md) §5.5): an edge node's mr-pass (`pass_backend_url=edge:`) calls `moqui.pass.v1.PassAuthority` on the down connection; a forwarded request never reaches it (the same rule as `LocalLink`). The link's mutual TLS is the client's identity, so no secret is sent: the local node names it `pass_edge_client` (default `edge`) and serves only the sites in `pass_client_<id>_sites`, with its SSO callback `pass_client_<id>_callback`. Everything else is the direct listener's (the row in Host routes): routed operations only, schema-checked input, a delegation per call, session handles bound to the client and site, the visitor's address as rate-limit data. A compromised edge node can do what a registered PASS server of those sites can: any routed operation and account action, as an anonymous visitor or with the delegations of the visitors who sign in through it (it is their frontend, so it sees their passwords and second-factor codes too), and it can name any visitor address for the rate limits. It cannot use a delegation or session handle made for another client or site, and it reaches no operation the routes do not list.
- **Port 25** (the MTA in moqui, E5b; MoquiEdgeConf.xml turns internet mode on, and `smtp_listen_port` in `.env` starts it): the [Mail](#mail) listener's internet mode. `RCPT TO` is answered from the paired nodes' replicated accounts (`mailRecipients`), never by calling a local node, so an unknown address is refused while a node is away, and a node not yet replicated answers 451. A message is queued for its node, not stored on the edge node beyond the queue's retention. There is no relaying: the edge node sends only what a paired node queued (`mailOut`), DKIM-signed with `mail_dkim_key`, to each recipient domain's MX. A VERP bounce address is taken only for a `mailOut` item the edge node holds the receipt of.
- **LMTP from a gateway beside it** (E5a, instead of port 25): the listener out of internet mode with `protocol="lmtp"` and an allow-list of the gateway's addresses (`127.0.0.1,::1`), as in [Mail](#mail). Mail out is relayed to the gateway on `edge_mail_relay_port`, on loopback.
- **Chat webhooks and media on the edge node** (E6): the [Host routes](#host-routes-and-servlets) rows for `/channels/{chatBridgeId}/webhook` and `/media/{token}`. The webhook's signature is checked with the bridge's replicated public key (never its API key, which stays on the local node), a bridge more than one paired node has, or one with no key, is 401, and the raw request goes in the queue, applied on the local node without a second check. A media address names a file the local node put there (`moqui.edge.EdgeMedia`, by the token's hash), only for that bridge of an active paired node, until its expiry; a file over `channels_edge_media_max_bytes` (5 MiB) or past `channels_edge_media_node_megabytes` (256) of a node's files is refused.
- **Offer and Configure** (the Edge Node wizard, [plan](../../plans/Edge-Node.md) §5.7): on the up connection, fenced by the link generation like the others, a local node asks `Link.GetOffer` (what the edge node offers it, with its own host names; no secret: the DKIM key's public record, public names and addresses) and sends `Link.Configure`. The edge node checks a Configure as input from any client before it changes anything: at most 64 host names, mail domains, bridges and sites each; domains and ids by their syntax; a mode other than passthrough or terminate refuses the whole request. Host names go through the same code as Edge Clients: only the asking node's names change (a name of the node not listed is removed), a name another node holds is refused without naming that node, and the node comes from the link's key, never the request. A local node cannot claim the edge node's own names, which the host table would otherwise route before the edge node's own handling (its admin UI, PASS sites, webhooks, or a certificate for its name): Configure refuses the edge node's public names and `webapp_http_host`, its `smtp_hostname`, `edge_mail_bounce_domain`, the host of its `channels_public_url`, its `public-site` hosts, and the subdomains of each, and any IP address, `localhost` (and its subdomains) and a name with one label. A name the operator adds on Edge Clients is the operator's choice and is not checked against these. Errors other than a refusal of the input go back as one generic message, with the detail in the edge node's log. The rest is kept on its `EdgeClient` row (refused for a node that is not active) for the operator to see. The Test step's checks on the edge node are limited to 20 a minute per node, as each mail-in check reads the node's replicated recipients.
- **Calls** (E7, [plan](../../plans/Edge-Node.md) §5.4): the edge node's media socket and TURN are the [Call media](#call-media) and [TURN](#turn) listeners, with the same rules (a relay reaches only the media port). Call signaling crosses the link as `Link.Signal` and `LocalLink.Signal` streams, fenced by the link generation like the other calls, on `rtc-signal` only. Each end takes only what the other may send. A local node takes from its edge node only `reply` messages, and delivers one only to a socket of its own that bound that leg of that call with its media on that edge node (the frame's `callSessionId`, its topic `rtc:<call>`, and `to` or `callLegId`); a frame, control, relay or room frame from the edge node is dropped, so a compromised edge node reaches no other socket and no other call. An edge node takes from a local node only `frame` (whose `origin`, where replies go, must be that node), `control` and `sip`; the link stamps each with the sending node (`linkNode`) and refuses one that already has the field, and the rooms of a paired node's calls are keyed by that node and the call (E12), so a node reaches only its own calls' rooms; the media driver checks a frame's leg and generation as a cluster message's. The edge node's instance id, which these checks and the routes trust, is pinned at pairing (`edge_node_id`, beside its key): a link naming another id, this node's own or a live cluster member's is refused, and the link never takes over a `CallMediaNode` row a media node registered. The edge node's `turn_auth_secret` goes to the local node only in the answer to Hello on an up connection whose key it pinned (not in Probe's Hello), and the local node keeps it in memory; revoking a local node does not change it (rotate it, [TURN](../interfaces/TURN.md#on-an-edge-node)). Delegated to the local node: `note#MediaEvent`, `get#CaptionSpeaker`, `store#CallTranscript` (only for a leg of the named call) and `transcribe#CaptionAudio` (only for a participant of the call, `userId` required, a chunk at most 4 MiB); queued: `callRecording`, stored only from the queue's own copy of the item's content and only as `_EDGE_`. Run as `_EDGE_`, each refuses a call whose media node is not the local node's edge node (its `CallMediaNode` row `edgeLinked`). A compromised edge node can see and record the media of calls that are not end-to-end encrypted, write captions, media events and recordings of the calls whose media it holds, and spend their participants' transcription quota.
- **SIP** (E8, [plan](../../plans/Edge-Node.md) §5.4 "Voice (SIP)"): the edge node's SIP listeners, RTP range and registrar are the [SIP](#sip) ones, with the same flood caps and failed-digest blocks, on the edge node's own; the calls it routes through the local node get the local node's fraud controls there, and the forwards its fallback makes while the local node is away get only the engine's own checks (the destination's class, the deployment's international and premium switches, the calls at once) and the number's limits at once and a day that its replicated fallback row carries; they start only when `.env` sets `sip_port` or `sip_tls_port`. Its trunks come from the replicated set `voiceTrunks` (their `authJson`, encrypted at rest); no phone's verifier is on it: each digest answer goes to the local node's `check#SipDeviceDigest` (the edge node keeps the nonces and their counts, the local node says whether the response is right for its HA1), so a stolen edge database holds no phone's credentials and outside phones sign in only while the local node is reachable. The local node takes such an answer only for the realm the edge node's registrar challenges with, only while its nonce is at most 330 seconds old by the local clock (the time is inside the nonce, which the phone's response covers, so the edge node cannot change it afterwards), and each (phone, nonce, nonce count) once, so an answer the edge node kept cannot be sent again. Delegated to the local node: the routing, leg and trunk notes, the digest check, bindings and device calls, IVR, queue and transfer services, voicemail's begin, PIN, keys and saved audio, and greeting and prompt audio (`get#CallAudio`, only under the greeting and prompt roots); queued: `voicemail` (edge to local, stored only from the queue's own copy, as `_EDGE_`) and `callAudio` (local to edge, kept only under those roots). Run as `_EDGE_`, `receive#PhoneCall` takes calls only for the edge node's own instance id, the call services only calls whose media is on it, the registrar's binding and call services only its own node and a phone whose fresh answer the local node checked within a minute, and voicemail's keys and saved audio only on a call that gave that box's PIN. A message the edge node queues goes in the box the local node chose for the call, or the called number's own, never one the edge node names. While the local node is away the edge node answers from the replicated `voiceFallbacks` (each number's outside number, which a call may be forwarded to through the engine's own fraud checks, and whether it takes a message). A compromised edge node can answer and record calls to the numbers, take messages for them, forward a number's calls to its replicated outside number, and route calls into the local node as from the trunks. Each delegated call carries how long the edge node waits; the local node starts nothing past it and rolls back a call that ran past it. It cannot sign a phone in or place a call as a user without a fresh answer of that phone, nor read a box's messages without its PIN.
- **SSH tunnel port**: off unless `edge_ssh_tunnel_port` is set (`edge_ssh_tunnel_host`, empty for every address). The edge node's web protection admits each connection (deny, bans, per-client limit), at most 64 at once; each goes down the link to the local node's SSH server, which serves it in process as a connection from the client's address, with its usual logins and limits. Refused while no node is linked, or when the local node runs no SSH server.
- **An all-in-one node** (`node_role=all-in-one`, MoquiAllInOneConf.xml, [plan](../../plans/Edge-Node.md) §10): a full server with its own users and data that also takes one local node's link, with every rule above. What the paired node's traffic can reach on it:
  - **Delegated calls.** A listed service is sent to the paired node only inside that node's owner scope (`moqui_core::edge::owner::with`, set by the engine that took the item), and to that node; outside it the service runs on the server for the server's own item.
  - **The queue.** A paired node's items run only its kind's handler from the server's own `queue-kind` conf, once per item id, as on an edge node.
  - **Replica rows** are `EdgeReplicaValue` rows of that node, encrypted at rest, read by the engines to route its items. They are never shown to the server's own users: only System › Edge Clients shows them, with the records held back for a claim (`moqui.edge.EdgeReplicaConflict`).
  - **Claims.** The server's own items win: Configure refuses the node's bridges, XMPP and SIP domains, numbers and a mail domain the server holds whole (its bounce domain), naming each, and a replicated record that claims a server's mail address, bridge or number is not applied (kept as a conflict, logged). The server's own setup refuses an item the replica holds (`EdgeClaims.secas.xml`). So a paired node cannot take the server's mail, webhooks, numbers or phones' realm. An address both sides hold gets 451 at `RCPT TO`, and a bridge both hold 409 at its webhook, naming the id; neither side gets it until an operator fixes it.
  - **Calls and SIP.** Each signaling message from the link carries the stamped `linkNode`, and a paired node's rooms, call, leg, trunk and phone ids are keyed by that node (`sip/owners.rs`), so its messages never reach the server's own calls, whose ids come from another database. A phone of the server's `sip_domain` is checked on the server; one of the paired node's domain is delegated to it. The paired node's greetings are kept under `dbresource://moqui/edge-node/<node>/`, apart from the server's.
  - **XMPP.** The tunnel ports serve only the paired node's domain, and a tunnel port equal to one of the server's own XMPP ports is not opened.
  - **Trust.** The paired node's owner trusts the server's operators as an edge node's: queued mail, messages and recordings are stored on the server until delivered. A compromised paired node can do on the server what it can on an edge node, for its own items only.

### Call media

Off unless `rtc_media_port` is set, and only in a binary with the `rtc` feature (Maximal, Enterprise, Debug). One UDP socket on `rtc_media_host` (127.0.0.1 by default); browsers are told `rtc_public_address` (the bound address by default; a wildcard bind without it does not start). It carries ICE (STUN) and DTLS-SRTP for every browser leg (`crates/moqui-rtc`, on str0m, ICE-lite). A datagram goes to the peer whose ICE session (its ufrag and password from the SDP exchange over the authenticated socket) accepts it; anything else is dropped. A peer exists only after its leg's `join` was bound by the call services and reached the media node; leaving, a newer generation of the leg, the call's end (`send#MediaControl`) and ICE consent loss remove it. At most 200 rooms and 50 legs a room on a node; SDP over 64 KiB is refused. Media is hop by hop: the SFU decrypts and re-encrypts each stream, except the frames of an end-to-end encrypted call (below), which it forwards as they come. The node registers itself (`CallMediaNode`) every 20 seconds; a call whose node stops registering ends.

Cascading media nodes (`rtc_cascade_enabled`, off by default) add relay links between the media nodes of a call on the
same UDP sockets: one str0m peer a room on each side, DTLS-SRTP like a browser leg, negotiated only by `rtc-signal`
messages on the cluster's client-certificate HTTPS transport (kind `relay`), so only a cluster member opens one. The
origin takes at most 8 links a room and 64 in all, and refuses more (`tooManyRelays`); a message of an older link is
dropped, a room that ended takes no new link, and a link not connected within 10 seconds is dropped. A leg node forwards
to the origin only its own legs' tracks; the origin decides nothing a leg node asks beyond the layers it sends it. Any
cluster member may open a link for any room id: the cluster's members are trusted alike.

ICE-TCP (`rtc_media_tcp_port`, off unless set) takes TCP connections on the same address: at most 1,000. Each carries
RFC 4571 frames of up to 2,048 bytes into the same SFU, which accepts only what a peer's ICE session takes.

End-to-end encrypted calls (W8, [WebRTC, Encrypted calls](../interfaces/WebRTC.md#encrypted-calls)) open nothing new.
Each browser SFrame-encrypts its frames (RFC 9605, AES-128-GCM on WebCrypto, in an `RTCRtpScriptTransform` worker) with
a key of its own per call and epoch. It sends that key to the other browsers' crypto devices as Olm to-device messages
over the `/ChatKeys` endpoints (C7 core), so the server holds and relays only Olm ciphertext; no chat Megolm session is
used. What the server and the media nodes still see: the RTP headers and header extensions (the audio level for the
active speaker), each frame's size and timing, the first 10 bytes of a VP8 keyframe and the first 3 of other VP8 frames
(kept in the clear so the SFU finds keyframes and sizes; authenticated as SFrame metadata), the SFrame header (the
sender's key index and epoch, the counter), and who is in the call with which device. The roster the browsers send keys
to is the server's (`get#Call`): a server that adds a device of a user in the call gets that leg's keys. A browser takes a
key only from the device the roster names for its leg (the Olm sender, which the server cannot forge), with that leg's key
index in its KID (refused while another leg has that index), and decrypts a track only with the keys of the leg it comes
from, each counter once (a replay window); a frame that does not decrypt is dropped, never played. A sender sends only
with keys it made, so a server-made roster cannot make two senders share a key and its nonces. A call page shows each
device as unverified (emoji and QR verification are in My devices, C7, but the call does not read their trust yet). The mode is set at the start (`requireE2ee`, a room's or a link's policy) and never lowered, and a
leg of such a call binds only with a crypto device of its user (`noCryptoDevice`); recording, captions, phone legs, desk
phones, forwarded numbers, guests and merges are refused. The browser's crypto store is encrypted at rest with a store
key the server gives only to its signed-in user (see Chat keys).

### Call recording

Off unless `call_recording_enabled` is true ([Call recording](../interfaces/Call-Recording.md)). It opens no port. Only a
host of the call starts, pauses, resumes or stops it (`org.moqui.impl.CallRecordingServices`). The recorder runs on the
media node, takes frames from a bounded queue and spools under `runtime/tmp/call-recording/`. `store#RecordingFile`
(internal, `allow-remote="false"`) refuses a spool path outside that directory and a location outside
`dbresource://moqui/call/recording/`. The files are read only through `get#RecordingFile` by segment id, from the
BasicComms Calls `Recordings` screen's `recordingFile` transition: the one who recorded it and the call's hosts get the
file, anyone else "not found" (an empty JSON answer) for the same id. No route serves those `dbresource://` locations
to users, and an app's record links (BOSS) go through the same services. The guest page's `ackRecording` and
`withdrawRecording` take the guest's credential and act on its own leg only. Every leg gets the notice, and each is
logged (`CallRecordingConsent`, with every earlier acknowledgement and withdrawal in `CallRecordingConsentHistory`).
The files count toward the owner's personal drive quota: a full drive refuses a start, and a recording that fills it
stops, so recording cannot fill the database past the quota by more than a segment. Retention deletes the files and
rows (`expire_Recordings_hourly`).

### TURN

The embedded TURN server (`crates/moqui-rtc` `turn/`, host `rtc/turn.rs`) runs on the media node. Each listener is off
unless set: `turn_port` (UDP and TCP), `turn_tls_port` (TLS), and 443 on `turn_tls443_host`. TLS uses the
`runtime/cert` certificates. Relays are UDP sockets in `turn_relay_ports`. Its rules:

- **Credentials** are short-lived TURN REST ones: `<expiry>:<legId>`, HMAC-SHA1 with `turn_auth_secret` (generated
  when unset; is-secret). They go only to an admitted leg (`get#IceServers`, `get#GuestIceServers`). An expired one
  is refused, and so is one of a leg that left (revoked by the media node).
- **Relay peers** are only the media nodes' media ports (and `turn_allowed_peers`, by address and port). A permission
  to any other address, or a channel to any other address or port, is dropped before the server sees it, and nothing
  is relayed to or from one, another port of a media node's address included. The exception is an XMPP user's own
  credential (`xmpp.<userId>`, from `XmppServices.get#IceServers`, XEP-0215), for calls between XMPP clients: it may
  relay to public unicast addresses too, never to loopback, private (RFC 1918, unique-local), link-local, shared
  (100.64/10), multicast or documentation ranges, nor an IPv6 address that carries an inside IPv4 one (6to4 of one,
  IPv4-compatible, NAT64, Teredo), nor another port of a media node's address or of the TURN listeners' own address
  (a relay there would come from the server itself, past a firewall that keeps the outside off those ports).
- **Limits:** two allocations per credential, `turn_max_allocations` in all, and `turn_max_kbps` each way per
  allocation; excess data is dropped. A stream buffers at most 64 KiB without a whole message. No TCP relays to
  peers are offered.

coturn can take its place ([TURN](../interfaces/TURN.md)), with the same secret and a deny-all, allow-media-node
peer rule.

### SIP

The SIP engine (`crates/moqui-sip` on rsipstack, host `src/sip/`) runs beside a media node, only in a binary with the
`sip` feature (Maximal, Enterprise, Debug). Each listener is off unless set: `sip_port` (UDP and TCP on `sip_host`),
`sip_tls_port` (TLS on `sip_tls_host`, the `runtime/cert` certificates). Each phone leg has a UDP RTP socket in
`sip_rtp_ports` on `sip_rtp_host`. An exposed SIP listener is scanned within hours, so its rules are release gates
([SIP](../interfaces/SIP.md) "Fraud controls"):

- **Who is answered.** An INVITE from a trunk's host or listed signaling addresses is a trunk's call; any other
  INVITE or REGISTER is challenged with SIP Digest (SHA-256 and MD5, nonces that expire after 5 minutes and carry
  random bytes and a MAC under a per-process key, nonce counts that must go up). Only a PBX trunk's credentials
  (profile "PBX on a private network") answer it: a provider trunk's are the provider account's password, so a digest
  with them is refused and counted as a failure, and a provider's calls come only from its addresses. A listed address
  range is at most a /8 (IPv4) or a /32 (IPv6); the services refuse a wider one and the engine ignores it. Other requests from addresses that
  are not a trunk's are not answered. Without the registrar, REGISTER is refused (403) without a
  challenge, so it counts no failures. Over UDP a
  source address can be forged, so a trunk taken by its address is best on TLS (the default) or TCP.
- **The registrar (V4).** Off unless `sip_registrar_enabled` is true, even with the listeners on. With it, a REGISTER
  and a phone's INVITE answer the same challenge with a desk or soft phone's credentials: a generated username and a
  22-character random password that is shown to its user once and never stored. Only its digest verifiers are kept
  (`SipDeviceVerifier.ha1`, encrypted, per algorithm and realm): SHA-256, and MD5 only for a device of the legacy
  profile, so an MD5 answer for a standard device fails. A failed or replayed answer counts against its source as any
  failed digest; no device or account is locked. Each REGISTER or signed INVITE looks the username up once
  (`get#SipDeviceAuth`), within the per-source rate. Rotation deletes the old verifiers and revocation the device's,
  and either ends its bindings and its calls on every engine. A binding is the flow the phone opened (its transport
  and source address and port); calls to the phone go back over it only, never to the address its Contact names, and
  a stream connection that closes ends its bindings. A device has at most 10 bindings, and expiry is held to
  `sip_registrar_min_expires`..`sip_registrar_max_expires`. A phone's own call is placed as its user through the same
  checks as the browser's (`place#PhoneCall`: outbound permission, caller ID, international and premium, limits,
  emergency first), so a phone's credentials give no more than its user has. Plain RTP from a phone is taken from its
  SDP's address or the address it signals from (its NAT's), on any port until the first packet latches.
- **Failed digests block the source.** `authFailureLimit` failures in `authFailureWindowSeconds` from one address
  block it for `authBlockSeconds` (5 in 10 minutes, for an hour, by default): its datagrams and stream messages are
  dropped before a transaction is made, nothing is answered, and the block is kept in `VoiceAuthFailure` across
  restarts (the engine counts failures in memory and writes only the block). An IPv6 source counts as its /64. The
  engine tracks at most 50,000 sources; when full, the ones not blocked go first, so rotating addresses does not lift
  the blocks. No account is locked or counted.
- **What an incoming call reaches.** Only a number in `PhoneNumber` rings anyone, through the call services'
  ringing; the SDP must be audio with a codec in common, and SRTP when the trunk's profile requires it (488
  otherwise).
- **Outbound.** Calls go out only from `send#SipControl` (the `rtc-signal` cluster topic, kind `sip`, to that node),
  which only the voice services send, after their checks (caller ID grant, outbound enabled, international and
  premium off by default, concurrency and daily limits, the emergency path). With home country 1, the area codes of
  the plan's other countries (the Caribbean's, Bermuda's) are international. The engine checks the number's class,
  the emergency trunk and the concurrency limit again before an INVITE goes out. A display name or extra header with
  a line break or a quote is cleaned or left out, so it cannot add headers (a `P-Asserted-Identity`) to the INVITE.
- **Forwards to outside numbers (V3).** A call that comes in can make the server dial out: a user's forwarding rules,
  their simultaneous ring to a mobile, and a ring group's fallback number. That is toll fraud's other door (a caller
  rings in to be bridged to a premium or foreign number), so a forward is a phone leg under the same checks as a call
  the user places (`VoiceServices` `forwardCheck` and `dialForward`): the user needs outbound calls and
  `VoiceUserSetting.forwardEnabled` (off by default, set only by a voice admin); international and premium need the
  deployment's and the user's permission; emergency numbers are never a target; the user's limits at once and per day
  count their forwards with the calls they place, and the deployment's calls in a day count every forward; and the
  engine checks the class and its calls at once again before the INVITE. A ring group's fallback number is set by a
  voice admin and checked against the deployment's policy. The caller ID of a forward is the number the call came in
  on, never one the caller supplied. A forward to one of this deployment's numbers is routed inside (no hairpin through
  the trunk), a forward back to the caller's number is skipped, and a call takes at most `call_forward_max_hops`
  forwards, with a user or group already in its plan skipped, so rules cannot loop a call or fan it out without bound.
  Caller lists and rules are the user's own (another user's are "not found"); the rule's target is checked when it is
  saved and again when a call uses it.
- **Transfers and adds to outside numbers (V5).** A user in a call can make the server dial out by transferring the
  call's outside party to a number, or adding a number to the call; a desk phone can ask for a transfer with REFER.
  Each is a call the user places, under the same checks (`CallTransfers.js`, `VoiceServices` `forwardCheck` with
  `transfer` and `dialForward`): outbound calls enabled for the user, international and premium only with the
  deployment's and the user's permission, emergency numbers refused (they are dialed directly), the user's limits at
  once and per day (which count every number they added or transferred to, `CallLeg.dialedByUserId`, so adding numbers
  one by one does not get past them), the deployment's calls in a day, and the engine's own check of the class and its calls at once
  before the INVITE. The limits are counted under one lock (`CallLimitLock`), so calls placed at once cannot pass them
  together. A REFER is taken only from a registered phone's answered leg in a call, as its user; its target is
  a username or a number under those checks, checked before the phone is answered (403 when refused, so it keeps the
  call); an attended REFER (Replaces) is refused. A person called, transferred to or added must pass the apps'
  `call-target` providers (IronStone: someone of the user's organizations), answered as a user that does not exist when
  refused. Only someone in the call (an
  answered leg of theirs) may transfer, add or merge; a call that still rings is not transferred; a stale revision or
  a second open transfer is refused. An external user (`EXTERNAL_USER`) neither transfers nor adds. An end-to-end encrypted call adds no phone legs. An IVR option, a menu's or a
  queue's fallback to a number is set by a voice admin and checked against the deployment's policy when it is saved
  and when a call uses it; a menu loop ends after 20 menus, and a queue's fallback chain after 10.
- **Credentials.** Trunk credentials are `VoiceTrunk.authJson`, encrypted; they answer only the trunk's realm and
  allowed algorithms. TLS to a trunk verifies its certificate against `sip_tls_ca_file` (the system bundle by default).
- **Media.** SRTP (AES_CM_128_HMAC_SHA1_80, SDES keys over TLS) unless the trunk's profile is a private-network PBX;
  a packet that fails its tag or replay window is dropped. RTP goes back where the leg's authenticated packets come
  from. Plain RTP is taken only from the SDP's address and, once the first packet latched, only from that source;
  anything else is dropped, so a guessed port neither injects audio nor takes the call's audio. RTCP is dropped.
  rsipstack logs whole SIP messages (SDES keys, digest answers) at INFO, so the host keeps only its warnings.
- **Floods.** rsipstack has no caps of its own, so the engine's transport inspector holds new requests (outside a
  dialog) to `sip_request_rate` a second per source (`sip_request_burst` at once; 20 and 100 by default, an IPv6
  source as its /64), drops them while `sip_max_transactions` (2,000) run, and closes TCP and TLS connections past
  `sip_max_connections` (500), or past `sip_max_connections_per_source` (100) from one source, so one source cannot
  hold the connections the registrar's phones keep open. What is over is dropped unanswered; requests in a dialog and responses pass, so calls
  going on are not cut. Answers sent before an incoming call's decision expire after 30 seconds (1,000 at most).
- **Limits.** A leg's jitter buffer holds 10 packets; a leg's consumer queue a second of frames; datagrams over 2 KiB
  are cut. The engine registers its node (`SipNode`) every 20 seconds.

### XMPP

The XMPP server (`crates/moqui-xmpp`, host `src/xmpp/`, [XMPP](../interfaces/XMPP.md)) runs only in a binary with the
`xmpp` feature (Maximal, Enterprise, Debug) and only when `xmpp_domain` is set. Each listener is off unless set:
`xmpp_port` (STARTTLS, required before anything else) on `xmpp_host`, `xmpp_tls_port` (direct TLS) on
`xmpp_tls_host`, and `/xmpp-websocket` with `xmpp_websocket_enabled`. Local users only: no server-to-server.

- **Sign-in** takes app passwords only, never the account password (which would count toward the account's lockout
  and cannot carry a second factor): SCRAM-SHA-256 and SCRAM-SHA-256-PLUS against the verifiers stored when the
  app password was made (salted, 10,000 iterations), with tls-exporter channel binding where the connection has its
  own TLS session and a downgrade (`y` where the server binds) refused; PLAIN over TLS only; FAST tokens (XEP-0484)
  bound to the client's user-agent id, kept encrypted, expiring (`xmpp_fast_token_days`), and without channel binding
  refused unless their count grows; a token goes with the app password it was issued under. An unknown user gets a salt
  derived from the name and `xmpp_scram_decoy_key` (is-secret, generated when unset), so the exchange does not tell it
  apart, and its salt stays the same across restarts and nodes. A disabled account cannot sign in, and each service
  call logs the user in again by name. A session checks every minute that its app password is not revoked and the
  account may sign in, and ends when not.
- **Failures** are counted per source address (moqui-lib `source_guard.rs`, as the SIP engine counts failed digests):
  5 in 10 minutes block the address for an hour, a /64 for IPv6; the account is never locked. Over WebSocket the
  address is the client a trusted proxy names. At most 20 connections from one address at once and 10,000 in all
  (TCP and WebSocket); a connection not signed in within a minute is closed; a stanza or stream header over 256 KiB,
  or nested over 32 deep, ends the stream; a session whose pushes back up (512) is closed. Stream management keeps at
  most 1,000 unacknowledged stanzas and 10 detached sessions a user; PEP 200 nodes a user and 256 items a node;
  uploads 60 slots a user an hour.
- **Authorization** is the user's: each stanza becomes a service call as the signed-in user, through
  `XmppClientServices` (granted to `ALL_USERS`), and every service checks the room (`get#RoomAccess`) or works on the
  user's own rows. An external user (`EXTERNAL_USER`) learns only the usernames of the people of their own rooms.
- **XML** is parsed by rxml, which refuses DTDs, entity declarations and processing instructions.
- **End-to-end encrypted Moqui rooms** (C7) are not served; OMEMO messages are kept as they came, unreadable to Moqui,
  never in a bridged room, with no text in snippets, search, notifications or agents. PEP items (OMEMO bundles hold
  public keys) are read by access model: `open` any local user, `presence` the people the owner shares a room with,
  `whitelist` the owner.
- **TURN for XMPP calls** (XEP-0215): off unless `xmpp_turn_enabled` is true; while off, external services are not
  advertised and a request gets an empty list. On, a credential keyed to the user (`xmpp.<userId>`) may relay to public
  unicast addresses only, never loopback, private, link-local, shared or documentation ranges ([TURN](#turn)); it is a
  relay to the internet within the TURN quotas, as a public TURN service's is. An external user (`EXTERNAL_USER`) never
  gets one, and a user at most `xmpp_turn_per_hour` (30) an hour (counted on the distributed cache
  `moqui.xmpp.turn.issued`); a refusal is the same empty list.
- **Push** (XEP-0357) registrations are stored, with their app server's secret encrypted; a notification has the message
  count only, and goes nowhere until federation (C3b).

### Voicemail

Voicemail ([Voicemail](../interfaces/Voicemail.md)) opens no port: it runs on the SIP engine above, for calls to a
user's number that nobody answers, and on the access number (`PnrVoicemail`) when an administrator sets one.

- **The PIN over the phone.** A caller who presses `*` in a greeting, or calls the access number, can enter a box
  number and a PIN. `check#VoicemailPin` (internal) checks it against the box's argon2 hash and limits failures per
  box (5 in 15 minutes, then 15 minutes refused from any caller) and per source (the caller's number, 10 in an hour,
  then an hour). The source is a caller ID the caller can fake, so the box's limit is the one that holds: about 240
  guesses a day, which tries every four-digit PIN in six weeks and every six-digit one in eleven years, so a new PIN
  has at least `voicemail_pin_min_digits` (6 by default) digits. A box that does not exist, has no PIN, or is blocked is refused in
  the same words after the same hash work and is counted the same way, so the answers say nothing of which boxes
  exist. Three refusals end the call. The PIN is never logged, and weak ones (one digit repeated, a run) are refused
  when set. Accepted, the caller hears only that box's new messages, and its keys act only on them (`act#PhoneVoicemail`
  checks the box).
- **The audio route.** Messages and greetings are under `dbresource://moqui/call/voicemail/`, which no route serves.
  The audio is read only through `get#VoicemailAudio` and `get#VoicemailGreeting`, from the Calls `Voicemail` and
  Settings `Phone` screens' transitions: the box's owner gets the file, anyone else "not found" (an empty JSON answer),
  the same as for an id that does not exist. The engine writes only through `store#VoicemailFile` (internal), which
  takes a spool path only under `runtime/tmp/voicemail` and a location only under the voicemail root.
- **Uploads.** A greeting upload is at most 10 MB, checked as WAV or Ogg Opus by its first bytes, then decoded and
  re-encoded by the server (`convert#GreetingAudio`, internal, which reads and writes only under the greetings' root);
  a minute is kept.
- **Mail out.** The email notice carries only a link unless the owner turns on the audio or the transcript in it; the
  screen warns that mail cannot be recalled. Transcription sends the audio to the speech provider only when
  `voicemail_transcribe` is on.
- **Limits.** A message is at most `voicemail_max_seconds` (120; at most 600), spooled to disk as it is recorded; one
  under a second is dropped.

### Chat bridges

The bridge layer ([Chat bridges](../interfaces/Chat-Bridges.md), C1) opens no port and routes no endpoint: a network
adapter (C4 on) brings its own transport, and C1 ships none.

- **Input.** `ChatBridgeServices.ingest#Event` is `allow-remote="false"`: only code in the server (an adapter, after its
  own checks of a webhook's signature or a gateway's session) calls it. It authenticates the bridge by its inbound token
  (`check#BridgeToken`: the SHA-256 against `ChatBridge.inboundTokenHash`, in constant time; a bridge with no hash takes
  none) and refuses the whole batch, storing nothing, for an unknown or disabled bridge, a wrong token, a malformed
  transaction id, more than 200 items or 1 MB, or an item that names a local user. An item posts only as an external
  actor (no login, no `UserAccount`) into a room an administrator mapped to that bridge (`noMapping` otherwise), as a
  member a moderator has not taken out, edits and deletes only its own messages, and carries plain text only (escaped to
  HTML; files are refused). A puppet link to a local user is provenance, never authority. Its messages fetch no link
  previews, reach a room's agents only under the room's `agentsReadExternal`, and never start the framework's agents.
- **Output.** A room's messages go only to the networks an administrator mapped it to; a link to an external room grants
  no rights. Delivery rows keep no message text.
- **Administration.** `find#Bridges`, `store#Bridge`, `store#RoomBridge` and `end#RoomBridge` need `ChatBridgeAdmin`.
  `ChatBridge.configJson` is an encrypted field and never returned; the inbound token is kept only as its hash.
  `retry#Delivery`: the message's sender, a moderator of the room, or `ChatBridgeAdmin`; anyone else is told "not found"
  or "not allowed" with the same parameters.
- **Internal services.** `send#Message`, `edit#Message`, `react#Message` and `redact#Message` take `senderActorId` only
  with authorization disabled. `get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`,
  `get#BridgeCapabilities`, `wake#BridgeDelivery`, `run#BridgeDeliveries`, `migrate#ChatActors` and the
  `ChatBridgeServices` internals are `allow-remote="false"`.

### Database

`entity_ds_db_conf` default `h2` (and an empty name) selects the embedded Postgres cluster. There is no H2 driver and no H2 TCP server.

Embedded Postgres is on when the entity datasource is embedded and the build has `embedded-postgres` (Dev, Lite, Maximal). The data directory is `runtime/db/embedded`. On Unix there is no TCP: `listen_addresses` is empty, and the socket directory is the data directory. `PORT` 5432 names the socket file `.s.PGSQL.5432`. A second postmaster (search, or the sim overlay) uses the same socket recipe. The client uses the bootstrap superuser, the password in the cluster `pgpass` file, and `NoTls`. This tree does not set `unix_socket_permissions` or `pg_hba.conf`. On Windows the same default listens on `127.0.0.1` and an ephemeral port.

External Postgres or MySQL is off unless `entity_ds_db_conf` is `postgres`, `mysql`, or `mysql8`. This process is a client. Omitted ports default to 5432 and 3306. TLS is on unless `entity_ds_sslmode=disable`.

### Jobs

The scheduler is a thread in this process. `scheduled_job_check_time` is 60 seconds. The first job pass waits 120 seconds, then every 60. `0` starts no job runner. Tick hooks such as the cluster heartbeat still run.

Scheduled cron jobs do not log in a user and do not run with artifact authorization. That is the scheduler's default. `dispatch` leaves `user_name` empty unless the row is an overflow queue (`ASYNC_QUEUE_`). `run_job` calls `internal_login_user` only when that name is set, then calls the service with authorization disabled. `ServiceJobUser` is who gets the completion or error notification. It is not the identity the job runs as. An explicit run-now logs in as the caller, and a queued fire-and-forget call logs in as the original caller, and both still disable authorization for the call. One job's error is logged and recorded. It does not stop the scheduler.

| Job | Seed state | Service |
| --- | --- | --- |
| `poll_EmailServer_frequent` | paused | `poll#EmailServer` |
| `send_AllProducedSystemMessages_frequent` | paused | `send#AllProducedSystemMessages`, `anonymous-all` |
| `consume_AllReceivedSystemMessages_frequent` | paused | `consume#AllReceivedSystemMessages`, `anonymous-all` |
| `run_EntitySyncAll_frequent` | paused | `run#EntitySyncAll`, `anonymous-all` |
| `backup_EmbeddedDatabase_daily` | paused | `backup#EmbeddedDatabase`. The mirror remote has no URL until set |
| `sync_EmailAccounts_frequent` | running | `sync#DueEmailAccounts` |
| `fetch_AllGitRepositories_frequent`, `process_PendingGitUpdates_frequent`, `retry_GitWebhookDeliveries_frequent` | running | git fetch, pending git ECA, webhook retry |
| `render_ScheduledScreens_frequent` | running | `render#ScheduledScreens` |
| `IndexDataFeedDocuments`, `ExportEntityDataSnapshot`, `ImportEntityDataSnapshot`, `ProcessTimePeriodPay` | no cron | ad hoc only |
| `ExampleSftpLocal_poll`, `DemoZiretSftp_poll_*` | paused | demo outbound SFTP |

Also running, on the same authorization-off path: the cleanup and `gc_*` jobs that are not paused, `send_NotificationDigests_daily`, `send_DueDrafts_frequent`, `return_SnoozedMessages_frequent`, `process_DataFeedPending_frequent`, chat expiry and retention, `rollup_LlmUsage`, and Mantle order, GL, reminder, and calendar jobs that are not paused. Also paused: the two ElasticSearch log cleaners, `poll_PaymentGatewayDetails`, `send_SalesInvoicePastDueEmails`, `check_AllPartyDuplicates`.

### Secrets

Startup, before listeners, replaces each secret `default-property` that still ends in `CHANGEME` after the environment and `runtime/.env` are read. The value is 32 CSPRNG bytes, base64, written into `runtime/.env`. A new file is mode `0600`. An unwritable `.env` fails startup. `entity_ds_crypt_pass_old` is the exception: the sentinel is kept. `cluster_tls_cert` and `cluster_tls_key` are generated as a pair when `cluster-tls` is on, which it is in every server package. The same path fills `ssh_host_key`, `session_cookie_key`, and `git_lfs_token_key`.

### Other local processes

The `pdf` feature, which every server package includes, starts headless Chromium when a binary is found, with `--remote-debugging-address=127.0.0.1` and `--remote-debugging-port=0`. This tree does not add its own authentication on that socket. If no Chromium binary is found, the server still starts and PDF is unavailable.

An OpenSearch child starts only when `search_backend` is opensearch, `elasticsearch_url` is loopback (the property default is `http://127.0.0.1:9200`), nothing is already answering, `runtime/opensearch/bin` exists, `opensearch_run` is not `false` (default `true`), and there is no pid file. The default `search_backend` is `postgres`, so the child stays off. User and password properties default to empty.

IPP printing and the SFTP client are outbound. Git, DAV, `/termws`, and `/notws` are routes on the public HTTP server. Web Push (the `web-push` feature) posts to the push service named by each subscription a browser stored, with a VAPID token; internal addresses are refused, no redirect is followed, and the payload is encrypted to the browser and holds no message body. FCM (`service/fcm.rs`) posts only to the token endpoint of the configured service account (`fcm_service_account`, is-secret, never generated) and to `fcm_base_url` (FCM by default), both from the conf, through the framework's outbound HTTP; the data message holds what a Web Push holds, plus a ring's call id, room id, caller name, video flag and expiry (and a ring's end, its call id and reason), and no message body.

## Heavy lock-down

A production install keeps the default conf's closed switches and does not publish the listeners that are on.

- Do not publish the HTTP port. Empty `webapp_http_bind` is every address, and empty `webapp_http_port` is 8080.
- Do not leave `entity_ds_crypt_pass` or `session_cookie_key` as `CHANGEME`. Startup writes generated values into `runtime/.env` when they still end in `CHANGEME`. Keep that file private (mode `0600`). The same generator fills `ssh_host_key`, the cluster certificate and key, and `git_lfs_token_key`. `entity_ds_crypt_pass_old` stays the sentinel on purpose.
- Do not load demo users.
- Leave tarpits on. The dev conf turns them off for screens, transitions, and services.
- Leave `webapp_allow_origins` empty. The dev conf sets `*`. A foreign origin is then refused. Own origin stays allowed.
- Leave SSH off unless you want it. The default port is empty. The dev conf sets `2222` on `127.0.0.1`.
- Leave A2A, MCP, and the gateway off unless a client is configured. The model facade can stay on without those servers.
- Leave git `allow-http` false, and DAV `account-password` false.
- The cluster listener binds every interface on 8444 and requires a client certificate. Keep it on a private network.
- Leave inbound SMTP off unless a gateway and a non-empty allow-list are in place. An empty allow-list does not start the listener.
- Per-client web protection is off until `webapp_trusted_proxies` is set or `webapp_web_protection` is `true`. Set the proxy list when a reverse proxy is in front, so the client address and the per-client rules see the client rather than the proxy.

## Public demo server

A public demo breaks one rule above on purpose: it loads the demo users, and anyone can log in as `john.doe`, an
`ADMIN`. The demo component (`runtime/dev-component/demo`, loaded by `runtime/conf/MoquiDemoConf.xml`) narrows what
that login reaches:

- Tools' SQL, Data Load, Export and Snapshots, Database Speed Test and Connections, Service Run and Service Load
  Runner are replaced by a screen that says they are disabled.
- `ADMIN` is VIEW-only on Tools, System, Monitor, `/moqui`, EntitySync and SystemMessage receive, `/resources` (so refused: `run` is not a view verb), git repositories,
  report git sync and report webhooks. It loses `SearchRemote`, `DashboardRemote`, `ADMIN_PASSWORD`, `SSH_SFTP`,
  `SSH_GIT`, `MONITOR_INGEST` and `REPORT_SAVE_LOCATION`. `john.doe` leaves `ADMIN_ADV`.
- SSH (`ssh-server enabled="false"`), DAV, the MCP server and client, A2A both ways and the gateway are off, and mail
  connects only to `localhost`.
- Password changes and resets, sign-in factors, and username, disabled and require-password-change changes are refused
  for the shared login, so one visitor cannot lock the others out.

Visitors can still change any business data, and `john.doe` can view every entity in Tools. Restore the database on a
schedule, and keep real keys out of it. Its README has the rest.

## How to use this for review

Walk each surface in the deployment you have: is it enabled, is it reachable from the networks you care about, how does it authenticate, what authorization (including inherit) applies, and what untrusted input does it accept. Application components add screens, services, and REST roots, and they need their own pass.

This file maps the surface. It does not prove the controls. The proof catalog is [Proof-Tests.md](Proof-Tests.md). The client suite those tests run is `runtime/base-component/framework/test/client/`. Reporting is in [SECURITY.md](../../SECURITY.md).

## Confirmed absent

| Surface | Where it was looked for | What this repository has |
| --- | --- | --- |
| Jetty, and a `web.xml` session store | `http.rs` binds a `TcpListener` and axum. A search for a file named `web.xml` under this tree found none. The name survives as a comment in `MoquiDefaultConf.xml` and as an exclusion in `plans/Parity-Mapping.md`. The conf comment still says "Embedded Jetty". | hyper, axum, and rustls. Sessions are `moqui.server.HttpSession` rows. Servlets and filters are conf elements read by `web/servlet.rs`. |
| H2 TCP server and the H2 console | No `/h2` route. No `H2Console` in `runtime/conf`. `entity_ds_db_conf=h2` is the embedded database name. Parity notes say the H2 console was removed and `H2ServerToolFactory` is not ported. Port 9092 is not a bind. | Embedded Postgres, Unix socket, no TCP. On Windows, TCP to `127.0.0.1` on an ephemeral port. |
| Jackrabbit on 8081 | The repository element is commented XML in the dev conf. `JackrabbitRunToolFactory` is not ported. | No process listens on 8081 for it. (`8081` elsewhere is an example HTTP port.) |
| SubEtha SMTP | No SubEtha code. The entity comment still names `SubEthaSmtpToolFactory`. Parity mapping says that factory is not ported. | Inbound SMTP, off until `smtp_listen_port` and a non-empty allow-list are set. Mail from anywhere but the gateway is refused. |
| `/groovysh`, `Tools/GroovyShell` | No conf endpoint or host route for `/groovysh`; the obsolete `Tools/GroovyShell.xml` screen was removed. | These paths are 404. The browser Terminal UI and the SSH admin shell are described above. |
| `/rpc/json` | No host route. No `rpc.xml` under webroot. No file named `rpc.xml` in the tree. | The path is 404. |
| `/elastic`, `/kibana` | No routes and no servlet patterns with those paths. | `/search` (`SearchRemote`) and `/dashboard` (`DashboardRemote`). |
| `/htmlr` | No route. The client-suite README says there is no `/htmlr` mapping. | The path is not served. |
| `/vapps` | No `vapps` subscreen in `webroot.xml`. | `qapps` and `apps`. |
| `/rest/api_key`, `/rest/moquiSessionToken`, `/rest/userInfo` | Comments in `rest.xml` only. | Not served. |

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- `emailOAuth.xml` `callback` and `sso.xml` `login`, `callback`, and `logout` were not fully read. The screens are `require-authentication="false"`, and those transitions are `require-session-token="false"` and read-only.
- `ScreenTreeNested.xml` is `require-authentication="false"` under `ScreenTree.xml`, which is required. `check_authentication` uses every screen in `render_defs()`. Whether a direct URL always includes the parent was not traced.
- Token-route failure behavior (a bad token, an anonymous session, the status code) was not read in `screen/token_route.rs`. The mr-boss conf and the dispatch call in `screen/mod.rs` were read.
- Service REST `*.rest.xml` roots were not listed. Seed data covers `/moqui` for `ADMIN` and `/moqui/git` for `ALL_USERS`, and the git services still check repository rights.
- The public `login_user` wrapper's handling of `Checked::SecondFactor`, and whether `current_user_id` stays empty, was not read. DAV treats an empty user id as failure (`dav/mod.rs`). `finish_login` is not called on the second-factor branch (`user/login.rs`).
- Assist authorization inside each Rust service body was not read. `docs/ai/Assist.md` is the contract cited above (cookie session, `AgentUse`, 404 for another user's object, 403 from inside a run, CSRF on POST).
- `listViewColumns` and `statusBar` call services with authorization disabled and do not check `userId` in the transition (`webroot.xml`). Whether artifact authorization on the root transition still wraps those scripts was not read.
- The entity REST page size used after `pageNoLimit` is removed was not read. `rest-max-page-size` is 1000.
- Unix embedded Postgres: this tree sets an empty `listen_addresses` and password auth, and it does not set the socket-directory mode or `pg_hba.conf`. Whether another local user can open `.s.PGSQL.5432` is whatever `postgresql_embedded` 0.21 defaults to. The superuser password is in the cluster `pgpass` file.
- A peer that holds the cluster certificate can clear entity one-caches, push `notification-message` and `collab-update` onto the local bus, and drop named live sessions. How far a crafted notification body is rendered to a browser was not traced past `notify::publish`.

### Integration-fix cluster rollout

The compact `collab-update` envelope carries the update once, in `body.frame.update`. Receivers apply that update before broadcasting the frame to local editors. Incoming notification socket messages are capped at 16 MiB, and a collaboration update whose complete cluster envelope exceeds that limit is refused before local application or acknowledgement.

Upgrade and restart every member together, with collaborative editing stopped until all members run the new version. Mixed versions are unsupported for this wire-format change; older receivers expect a second top-level update and HTTPS receivers still enforce 1 MiB. No database migration is required.
