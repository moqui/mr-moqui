# Attack surface: web authentication and controls

How a web request is authenticated and authorized, the controls every request passes (session cookie, CSRF, lockout, tarpit, headers, CORS, client address, web protection), and the HTTP and HTTPS listeners with ACME. The routes themselves are in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## HTTP authentication and authorization

A servlet request is signed in from the session cookie, HTTP Basic, an `api_key` or `login_key` header, or `authUsername` and `authPassword` (`docs/interfaces/Web-Scripts.md`). A key header is checked on every request, ahead of the cookie and Basic. A key for a user other than the session's, or a rejected key while a session is signed in, is a 401. A rejected key with nobody signed in leaves the request anonymous with the login error. `/notws` accepts the session cookie and, failing that, a `login_key` header. `/termws` accepts the session cookie only. Notification sockets revalidate the original session or hashed login key every 60 seconds and close on logout, a changed user, key revocation or expiry, or a failed credential lookup. Revalidation does not log in again or write login history. Protocol Ping/Pong traffic cannot extend authentication.

Screens carry `require-authentication`. The parser maps `false`, `anonymous-view`, and `anonymous-all` to those modes, and it maps a missing attribute to required. A render requires a user when any screen in the chain is required. An anonymous login counts as a user.

Artifact authorization runs as each screen, transition, service, REST path, entity, git repository, and LLM call is pushed. Inheritable allow and always records cover children on that stack unless a more specific deny wins.

### Parent frames

A render checks only the last screen of its path; the screens above it are entered without their own check (Java's `pushInternal` with authorization required of the target alone), so their always- and pre-actions run for a user who may open the target. The qvt shell loads each level of a page as its own request, and the terminal client each level between the app root and the leaf (`Party.tui`), which checks that level as a target. A level the user may not open alone answers, only when the request names in `X-Moqui-Page-Path` a page strictly below it on the same screens that the user may open (the page's screen permitted with its parents on the stack, its subscreens items' user groups, and a shared device's route group), as the page's frame (`screen/frame.rs`): a GET in qvt or tui, written from the level's widgets as its subscreens placeholders alone (up to the first active level), `private, no-store`. Nothing of the level runs: no always-, pre- or widget actions, no section, no form, no label, no transition; its shell providers are not gathered for it (the shell gathers them only from roots the user may open). Its tabs list only the subscreens the user may open: in qvt the shell's menu data fills them, in tui the frame carries them, as the menu data makes each entry of that level for the page (each subscreen authorized below the level, its `menu-include` and `disable-when` evaluated, its title expanded; nothing of the level itself runs, where the menu data also runs the levels' pre-actions). What the frame tells the user is that the level exists, its title (tui) and that it has a subscreens placeholder or a tab bar, which the page's own menu data and breadcrumb titles already showed them. The level asked for without the header, in another mode (an html page), as a transition, for a page the user may not open, that does not resolve, or is not below it, or a level with a screen on its path that grants anonymous access (whose own render may apply that grant where the check does not), is refused or rendered as before; the launcher lists only roots the user may open. Checking the page is one path resolution and one permission check of its screens, as its own request would be.

Seed grants, from `runtime/components/base/tools/data/ToolsSecurityData.xml`:

| Group | Artifact | Grant |
| --- | --- | --- |
| `TOOLS_APP` | `component://tools/screen/Tools.xml`, `inheritAuthz` Y; `moqui.tools.ToolsHomeServices.get#ScreenCount` and `count#Screens` (the Tools home's screen count, counted on a worker) | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `SYSTEM_APP` | `component://tools/screen/System.xml`, `inheritAuthz` Y, which holds Monitor (`System/Monitor.xml`); `moqui.tools.SystemHomeServices.get#SystemAttention`; the `org.moqui.report.dataset.otel_*` datasets and `org.moqui.report.dataset.monitor_annotations`; `org.moqui.monitor.MonitorServices.*`, `TelemetryServices.scrape#Targets` and `run#Checks`, `moqui.monitor.ScrapeTarget` and `SyntheticCheck`; `org.moqui.agent.AgentEvalServices.*` and the agent eval entities (`moqui.agent.AgentScorer`, `AgentScorerAttach`, `AgentDataset`, `AgentDatasetItem`, `AgentExperiment`, `AgentExperimentResult`, `AgentScore`) | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API` | `/moqui`, `AT_REST_PATH`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API_GIT` | `/moqui/git`, `inheritAuthz` Y | `ALL_USERS`, `AUTHZT_ALLOW`, `AUTHZA_ALL`. The services still check `AT_GIT_REPOSITORY`, and the writes ask `git-server` `rest-write-check-service` first (docs/interfaces/Git-Hosting.md, REST) |
| `MOQUI_API_HOSTING` | `/moqui/hosting`, `inheritAuthz` Y | `ALL_USERS`, `AUTHZT_ALLOW`, `AUTHZA_ALL`. `offer` is `anonymous-view` and gives no one's data; `repository` and `mergeRequests` answer only what the caller may view |

The proxy permissions are `SearchRemote` and `DashboardRemote`. Both are seeded on `ADMIN`.

## Cross-cutting controls

These apply unless a row says otherwise.

### Session cookie

The session row is `moqui.server.HttpSession`. The cookie is `moqui.session={id}.{mac}`, then `HttpOnly`, `SameSite=Lax`, and `Path=/`. `Secure` is added only when the request is https: the TLS listener, or `X-Forwarded-Proto: https` from a peer `ClientAddressing` believes. A client that is not that peer cannot turn the flag on by sending the header. The visitor cookie is `moqui.visitor`, with `Max-Age` of one year. The session cookie itself has no `Max-Age`.

The MAC is HMAC-SHA256 of the session id under `session_cookie_key`, base64 URL-safe without padding. A bad MAC is treated as no session. The session id and the session token are 20 random bytes, hex, 40 characters. `session-config` is `timeout="60"` (minutes), `flush="eventual"`, `max="10000"`.

`session_cookie_key` ships as `CHANGEME` with `is-secret="true"`. Startup generates a value into `runtime/.env` when the property still ends in `CHANGEME`. See [Secrets](Attack-Surface-Data.md#secrets).

#### Pages without a session

A request that comes with no session cookie starts no Visit until the cookie comes back or a user signs in. Two kinds of page send no session at all, so no cookie comes back:

- A page a shared cache may keep (`Cache-Control` with `public`), for a request with no live session (no verified session cookie, or one naming a session that is gone) and nobody signed in: no `Set-Cookie`, `moquiSessionToken` or `X-CSRF-Token`, and the session made for the request is discarded unwritten, so a cache never keeps a new session's cookie to hand to others.
- A page whose screen or servlet calls `ec.web.declineSession()`, whatever its caching (a preview frame on its own host, an embed, a beacon). For a request with no session of its own and nobody signed in it is the same as the public page. A request that has a session keeps it and is sent nothing new: no cookie and no token. Either way a Visit the request started at the bind (a returning `moqui.visitor` or a session cookie with no visit yet) is deleted, with a Visitor made for it, and the session forgets it, so no Visit has a declined request as its `initialRequest`. A Visit a sign-in in the request started stays. A page that signs a user in should not decline.

```js
ec.web.declineSession()   // e.g. mr-pass Shop/ShopPreview.js, before it writes the preview
```

A public island's component is the first kind too (impl-shop F72): the `.qvue` or `.qjs` answer of a screen that says `server-static` for that mode itself (not a placeholder screen's automatic one) or declines its session is sent with the `screen-server-static` `Cache-Control` (public by default), so it makes no session, but only for a request that brought no session of its own and has nobody signed in (`render.rs` `is_public_island`). A signed-in user, a request with a session, and any other screen's component get `private, no-store`, and keep their session. A `Cache-Control` the screen set itself stands. A screen that says `server-static` for qvue or qjs must send the same component to everyone; data for one visitor comes by a later request (a transition), which keeps its session.

### The system scope

`ec.artifactExecution.runAsSystem(body)` runs `body` with no user and authorization off, in a fresh execution context that shares the caller's transaction, effective time and agent origin (`user/system.rs` `as_system`; Commerce F72). It is for a service that has already checked its caller and must write what that caller has no right to write directly, such as Mantle's site page saves into a page space with `restrictUpdate` (a rule like `get#WikiPageAccess`'s `!userId && authzOff` then admits it). The caller's user, authorization and artifact stack come back when `body` returns, throws or panics; the messages and errors `body` added are moved to the caller's context. Records written inside have no `changedByUserId` or `lastUpdatedByUserId` of a user. A wiki save can still say whom it was for: `update#WikiPage` reads `historyUserId` only with no user and authorization off, for its `WikiPageHistory` row and a new page's creator (Mantle's site page saves pass the acting user), and `get#WikiPageHistory` names that user for a version that has none of its own.

Only a service's own code may call it: the top artifact frame must be a service's (`AT_SERVICE`), pushed after any screen render in progress on the thread began (`ExecutionContext::in_service_body`). Anything else is refused with an error: a screen's actions, widget expressions and transitions, a screen a service renders (the render began after its frame), a REST resource, a web script, a webapp action and a script with no frame. A remote or REST caller has no way to name it; it reaches the scope only through a service whose own code calls it, so the service decides what runs. Rust code calls `crate::user::as_system` directly. There is no XML action for it.

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

`update#Password` is `authenticate="anonymous-all"` and `allow-remote="false"`. `reset#Password` is `authenticate="anonymous-all"` and `allow-remote="true"` (`UserServices.xml`). `create#InitialAdminAccount` is `authenticate="anonymous-all"`. It returns an error unless `get#InitialAdminNeeded` (`authenticate="anonymous-view"`) answers `needed`: no `UserAccount` row exists but the seed accounts nobody logs in as (`_NA_`, the edge node's delegate `_EDGE_` and the agents' chat users). It then creates the account and adds it to `ADMIN` with authorization disabled. `get#InitialAdminNeeded` answers the password rules only while `needed` is true. The Login screen's actions use the same service, so the page and the service count accounts the same way.

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

Screen renders send `Content-Security-Policy: frame-ancestors 'none'; form-action 'self';`, `X-Frame-Options: sameorigin`, `X-XSS-Protection: 1; mode=block`, and `X-Content-Type-Options: nosniff`. `Strict-Transport-Security: max-age=31536000` is on type `screen-secure`. A stricter CSP in the conf comment is commented out. The Assist sandbox frame uses its own policy, under [Assist](Attack-Surface-AI.md#assist).

### CORS

`webapp_handle_cors` is `true`. `webapp_allow_origins` is empty. An origin is its scheme, host, and port, all three. The server's own origin is allowed: https over TLS or from a believed proxy, with the host and port of `Host` or of a believed proxy's forwarded host and port, and the webapp URLs from the conf. A foreign origin is 401 `Origin not allowed`, before the servlet. An `allow-origins` entry that includes a scheme is matched the same way. A bare domain allows that host and the hosts under it on any scheme and port. `*` allows every origin. The dev conf sets `*`.

An `OPTIONS` preflight is 204 with an empty body, before the servlet chain (`crates/moqui-core/src/screen/mod.rs`). Preflight and actual responses send `Access-Control-Allow-Credentials: true`. Preflight also sends the conf allow-headers (including `api_key`, `login_key`, `moquiSessionToken`, `X-CSRF-Token`, `Authorization`, `A2A-Version`, `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`), allow-methods `GET, PUT, POST, PATCH, DELETE, OPTIONS`, and `Access-Control-Max-Age: 3600`.

`/notws`, `/termws`, and the Assist event stream use the same origin list and answer 403 `origin not allowed`.

### Client IP

`webapp_client_ip_header` and `webapp_trusted_proxies` are empty, so `X-Forwarded-For` is not trusted. `ClientAddressing` believes a peer only when a header is configured. If a header is set and the trusted-proxy list is still empty, the first hop is believed from any peer, and `X-Forwarded-Proto` uses that same gate. The comment above the property still names Jetty's `ForwardedRequestCustomizer`. The listener uses `ClientAddressing`.

On an edge node (`node_role=front_edge`) and an all-in-one node (`node_role=all-in-one`) a header with an empty trusted-proxy list is ignored, with an error at startup: the edge node faces the internet and passes the client address to its local nodes ([Edge node](../Run-Deploy.md#a-local-server-with-an-edge-node)).

`/health` and `/metrics` use the TCP peer, not this header. `/status` uses `ec.user.getClientIp()`, which can differ.
Both take any address in 127.0.0.0/8 as loopback.

A server of `moqui test --serve` (and only that: the command sets it in code before its server starts; no property or
conf turns it on) takes a request's `X-Moqui-Test-Peer` header as its TCP peer and client address, when the peer and
the client address (a believed proxy's header included) are both loopback and the header names an IPv4 address in
127.0.0.0/8, so `moqui test-client` gives each client spec its own requester
([JS-Test-Client](../dev/JS-Test-Client.md#--serve)). The header is removed before any request is served, on every
server. A client can name only a loopback address, from loopback, on a test server; `./moq run` never reads it.

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
| Rate limit `git-anonymous` | git requests with no credentials (`anonymous="true"`: no `Authorization`, `api_key` or `login_key` header), under `/git/**` and at a page's URL (`**/info/refs`, `**/git-upload-pack`, `**/git-receive-pack`, `**/*.git/info/lfs/**`): 120 / 60 s, burst 240 |
| Ban `auth-failure` | 20 events in 600 s, ban 900 s |
| Ban `ci-token-failure` | 100 events in 600 s, ban 900 s: a well-formed CI job, runner or pool token (to `/ci/runner/` or `/git/`) that the CI endpoints or the git server did not know. A token of a job that ended is no event. A CI token found good passes the per-client rate limits for 10 minutes after its last good request (on that host), since the CI endpoints count each job and runner and runners share addresses; a made-up token is counted by its address as any request is ([Pipelines](../interfaces/Pipelines.md#behind-a-front-or-edge-node-and-in-a-pool)) |
| Ban `abuse` | `rate-limited` and `denied`, 300 in 60 s, ban 300 s |
| Path deny `scanners` | `/.env`, `/.git/**`, `/wp-admin/**`, `/wp-login.php`, `**/*.php`, `**/*.asp`, `**/*.aspx` |
| User-agent deny | `sqlmap`, `nikto`, `masscan`, `zgrab` |

**Per host and shared** ([plan](../../plans/Edge-Node2.md) §4.3, E17). A `rate-limit`, `client-rule`, `path-rule` or `user-agent-rule` with `host` (names, `*.example.com` for any name under one, matched against the request's `Host` without its port; a `host` written with a port is warned of at start and never matches) applies only to that host's requests, on top of the rules without `host`; a connection's checks before its first request, and the bans, use only the rules without `host`. The host is the client's own header, so a host rule makes one site stricter and never exempts another: an exempt `client-rule` with `host` exempts that host's requests only. A ban started on one server is told on the cluster topic `web-ban` to the members of its cluster, taken only when the sender is a live member by its `ClusterMember` row, and up each edge link the server dialed (`Signal`, the topic `web-ban`, kind `ban` or `lift`) to its edge node, the ids `edge_node_id` names. Nothing goes down a link: an edge node's own bans may come from any pairing's visitors, so no pairing is told of them, and a local node takes no ban from its edge node. A ban told by a paired node is stamped with that node by the edge node's link and holds only for requests to that node's host names (the forward refuses them 403, counted as `banned by a pairing`), at most 10,000 per node and a week, never for a trusted proxy or a client a rule without `host` exempts; that node's lift lifts only its own. A ban taken from elsewhere is not told on. A shared ban's length is sent in seconds and timed by each server's own clock, while member liveness is the database's; a skew between them changes only when a ban ends. `org.moqui.admin.OperationServices.get#WebProtection` (`ADMIN_OPERATION_VIEW`) lists each rule's counts and the bans held now (at most 1,000), and `lift#WebBan` (`ADMIN_OPERATION_MANAGE`) lifts every ban of an address on the server and tells the others; both `allow-remote="false"`, shown by System › Security › Web Protection.

## HTTP container

The listener is hyper and axum. `webapp_http_bind` empty binds every address. `webapp_http_port` empty is port 8080. The plain HTTP listener is HTTP/1.1. The HTTPS listener offers `h2` and `http/1.1` by ALPN (`webapp_http2=false` offers only `http/1.1`). An HTTP/2 connection takes at most `http2-max-streams` (100) open streams, a request head of at most `max-header-bytes`, h2's bounds on streams a client resets before they are taken (20) and on streams reset for its protocol errors (1,024), PINGs every 30 seconds, and is closed with a GOAWAY after `header-timeout` with no request, or when web protection refuses its client with a closing refusal (HTTP/1.1's `Connection: close`). An HTTP/2 request is given `Host` from `:authority`; a WebSocket over extended CONNECT is checked by web protection as the GET of an HTTP/1.1 upgrade. Over HTTP/2 a `content-type: application/grpc` call goes to the gRPC services that are on (OTLP ingest, PASS authority; the rows below) before web protection's per-request rules and the router, as on their own port, except a call for an edge node's terminate host, which is forwarded as any request. There is no separate servlet descriptor: filters and servlets are the webroot `<webapp>` children in `MoquiDefaultConf.xml`, run by the screen chain. The session is the database row above.

`webapp_https_enabled` is `false`. `webapp_https_listen_port` empty is 8443 when the listener is used. The value `off` starts none. With no usable certificate in `runtime/cert` the listener does not bind, whether or not the port was set, until a name is set up with ACME ([ACME](#acme)), a certificate is added, or while a set-up check runs; it then answers only the challenges of names set up and refuses every other handshake until a certificate is there. An edge node that started so takes no links (`edge_link` is not given the listener's certificates) until it restarts with one. `instance_purpose=dev` tries to generate a development certificate first. `webapp_https_port` is the port clients see in generated URLs, which can differ behind a proxy.

### ACME

Certificates from an ACME authority, only for a name an administrator sets up on System › Certificates (`moqui.server.AcmeName`, `org.moqui.admin.CertificateServices`, `crates/moqui-host/src/acme/`, [plan](../../plans/Edge-Node2.md) §4.8). Nothing is requested for any other name, and nothing a client sends starts an order.

- **The challenge** (TLS-ALPN-01, RFC 8737) is answered on the HTTPS listener: a ClientHello that offers ALPN `acme-tls/1` gets a self-signed certificate carrying the SHA-256 of the order's key authorization, only when its server name has a challenge open (`moqui.server.AcmeChallenge`, at most ten minutes, closed when the order ends), and is refused otherwise; it is never served HTTP. The open challenges are read from their rows again when the last read is a second old, and at most once a second per process, so `acme-tls/1` handshakes cost at most that; an order waits past that second before the authority looks, so every member answers the challenge as it is now. On an edge node the passthrough check comes first, so a local node's passthrough name's challenge goes down its tunnel and the local node answers it; the edge node answers only for names it ends TLS for itself.
- **The set-up check** opens a challenge of its own with a random digest, for at most a minute and closed when the check ends, and connects to the name on 443 as the authority would, taking any certificate (it checks no signature, only that its digest is in the certificate), for at most 20 seconds. It reaches whatever address the name's DNS has; an administrator chooses the name. On a node with no certificate the listener binds for the check and closes after it.
- **Calls to the authority** have a time limit (a minute for the account and the order, three for the validation and the certificate), well under the 15-minute order lock.
- **The account key** is a P-256 key derived from `acme_account_key`, a secret generated into `runtime/.env` ([Secrets](Attack-Surface-Data.md#secrets)). Cluster members share it in their shared `.env`, or each is a separate account. Whoever has it can act as the account at the authority: revoke its certificates, and order for a name whose challenge they can answer.
- **The certificate's key** is new for each order. It is kept in `moqui.server.AcmeCertificate` (`privateKeyPem` encrypted at rest) so every member of a cluster writes it, and in `runtime/cert/<name>/privkey.pem` (mode `0600`). A `runtime/cert/<name>/` directory without the `.moqui-acme` marker is another tool's and is never written over or removed.
- **The authority** is reached over HTTPS checked by the operating system's trust store (`instant-acme` on hyper-rustls), or by the one root `acme_directory_ca` names for a company's own authority; the directory URL is the administrator's choice (`acme_directory_url` by default).

The request deadline is `request-timeout`, code default 300 seconds, and the answer is 504. Exempt paths are `/notws`, `/termws`, paths starting with `/git/` (and git at a page's URL on a public-site host, which is served as its `/git/` request: [Git hosting](../interfaces/Git-Hosting.md#repository-paths)), `/ci/runner/`, `/dav/`, and `/llm/v1`, and multipart uploads; on an edge node, a request it forwards to a member, which applies its own deadline (the forward's waits bound it on the edge node: [Edge](Attack-Surface-Cluster-Edge.md#edge-node)). A screen response past the `screen-facade` `stream-threshold` (64 KiB) is sent as it renders ([Screen output](../ui/Screen-Output.md)): the deadline covers it up to its head, and after that it holds its request worker and its transaction while the client reads. A client that takes nothing for `write-timeout` ends the render, so one that stops reading holds a worker for at most that long.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- The public `login_user` wrapper's handling of `Checked::SecondFactor`, and whether `current_user_id` stays empty, was not read. DAV treats an empty user id as failure (`dav/mod.rs`). `finish_login` is not called on the second-factor branch (`user/login.rs`).
