# Attack surface of mr-moqui

This is a map of the HTTP routes, screens, REST, WebSockets, and the other listeners in this repository. It is for operators, security researchers, and review of this repository. It records what can be reached. It is not a vulnerability report, and it is not a WAF. Proof of the controls lives in [SECURITY_TESTS.md](SECURITY_TESTS.md).

Application components (Mantle, Marble, IronStone, and the mr-boss screens beyond the framework's token-route mount) add their own screens and services. Those are out of scope here, except where the framework mounts a route they plug into.

To report an issue that should stay private until it is fixed, write to [moqui-board@googlegroups.com](mailto:moqui-board@googlegroups.com). The policy is in [SECURITY.md](SECURITY.md).

## How to read this

A request goes from the client, through a reverse proxy or TLS, to hyper and axum in `crates/moqui-host/src/http.rs`, then screen dispatch, artifact authorization, and on to the database, search, mail, and jobs.

This server terminates TLS itself with rustls (`crates/moqui-host/src/tls.rs`) when the HTTPS listener is started. It is still meant to run behind a proxy. `webapp_https_enabled` defaults to false and is the flag generated URLs use when they pick https. Starting the listener is the separate rule in [HTTP container](#http-container).

**Defaults** are `runtime/base-component/framework/conf/MoquiDefaultConf.xml`. **On** and **off** mean those defaults. Rows say what untrusted input can reach, and what to keep in mind during a review. Designed admin tools are privileged on purpose.

`./moq run` merges `runtime/conf/MoquiDevConf.xml` on top of that file. The dev file sets `instance_purpose` to `dev`, `webapp_allow_origins` to `*`, and `ssh_port` to `2222`, and it sets `tarpit-enabled` false for screens, screen transitions, and services. It does not declare an H2 console. The Jackrabbit repository element in it is commented out. The tables below stay on the default conf unless a row says otherwise.

## HTTP authentication and authorization

A servlet request is signed in from the session cookie, HTTP Basic, an `api_key` or `login_key` header, or `authUsername` and `authPassword` (`docs/Web-Scripts.md`). `/notws` accepts the session cookie and, failing that, a `login_key` header. `/termws` accepts the session cookie only. Notification sockets revalidate the original session or hashed login key every 60 seconds and close on logout, a changed user, key revocation or expiry, or a failed credential lookup. Revalidation does not log in again or write login history. Protocol Ping/Pong traffic cannot extend authentication.

Screens carry `require-authentication`. The parser maps `false`, `anonymous-view`, and `anonymous-all` to those modes, and it maps a missing attribute to required. A render requires a user when any screen in the chain is required. An anonymous login counts as a user.

Artifact authorization runs as each screen, transition, service, REST path, entity, git repository, and LLM call is pushed. Inheritable allow and always records cover children on that stack unless a more specific deny wins.

Seed grants, from `runtime/base-component/tools/data/ToolsSecurityData.xml`:

| Group | Artifact | Grant |
| --- | --- | --- |
| `TOOLS_APP` | `component://tools/screen/Tools.xml`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `SYSTEM_APP` | `component://tools/screen/System.xml`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API` | `/moqui`, `AT_REST_PATH`, `inheritAuthz` Y | `ADMIN`, `AUTHZT_ALWAYS`, `AUTHZA_ALL` |
| `MOQUI_API_GIT` | `/moqui/git`, `inheritAuthz` Y | `ALL_USERS`, `AUTHZT_ALLOW`, `AUTHZA_ALL`. The services still check `AT_GIT_REPOSITORY` |

`user-facade` `authz` sets the privileged groups to `ADMIN` and `ADMIN_ADV`. Creating, updating, or deleting membership in those groups is allowed only when the caller is already a member. Seed and install, and a call with authorization disabled, skip that check. The sealed permissions are `GROOVY_SHELL_WEB`, `SQL_RUNNER_WEB`, `SSH_SHELL`, `SERVICE_LOAD_RUNNER`, `ADMIN_LOGIN_AS`, `ADMIN_PASSWORD`, `SearchRemote`, `DashboardRemote`, `REST_SCHEMA`, and `LlmGateway`. A `UserGroupPermission` row for one of those can be written only by a caller who already has that permission.

The proxy permissions are `SearchRemote` and `DashboardRemote`. Both are seeded on `ADMIN`.

`AgentTypeData.xml` defines `AgentUse`, `AgentAdmin`, `AgentAudit`, `LlmGatewayUse`, `A2aServerUse`, `McpServerUse`, and `AgentSkillAuthor`. The `ADMIN` grant rows in that file are `AgentUse`, `AgentAdmin`, and `AgentSkillAuthor`. `A2aServerUse`, `McpServerUse`, and `LlmGatewayUse` are not among those rows. `AgentAudit` is not among them either. `SecurityTypeData.xml` grants `GROOVY_SHELL_WEB`, `SQL_RUNNER_WEB`, `SSH_SHELL`, `SERVICE_LOAD_RUNNER`, and `ADMIN_LOGIN_AS` to `ADMIN_ADV`, and `ADMIN_PASSWORD` to `ADMIN`.

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

`/health` and `/metrics` use the TCP peer, not this header. `/status` uses `ec.user.getClientIp()`, which can differ.

### Field encryption

`entity_ds_crypt_pass` ships as `MoquiDefaultPassword:CHANGEME` with `is-secret="true"`. It is the `entity-facade` `crypt-pass`. The algorithm is `PBEWithHmacSHA256AndAES_128`, salt `20201202`, 10 iterations. The same secret encrypts an OAuth refresh token and a client secret stored on `EmailOAuthProvider`. Alternate decrypt still tries `PBEWithMD5AndDES` with the current pass and with `entity_ds_crypt_pass_old`, and the AES algorithm with the old pass. The old pass ships as the same `CHANGEME` sentinel and is not replaced by the generator.

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

The listener is hyper and axum. `webapp_http_bind` empty binds every address. `webapp_http_port` empty is port 8080. There is no separate servlet descriptor: filters and servlets are the webroot `<webapp>` children in `MoquiDefaultConf.xml`, run by the screen chain. The session is the database row above.

`webapp_https_enabled` is `false`. `webapp_https_listen_port` empty is 8443 when the listener is used. The value `off` starts none. `plan` does not start HTTPS when `runtime/cert` has no usable certificate, unless the port was set explicitly (that case is an error with no certificate). `instance_purpose=dev` tries to generate a development certificate first. `webapp_https_port` is the port clients see in generated URLs, which can differ behind a proxy.

The request deadline is `request-timeout`, code default 300 seconds, and the answer is 504. Exempt paths are `/notws`, `/termws`, paths starting with `/git/`, `/dav/`, and `/llm/v1`, and multipart uploads.

Default Cargo features are `maximal` and `devtools` (`crates/moqui-host/Cargo.toml`). `maximal` includes `enterprise`, which includes `git`, `llm`, `tui`, and `ssh`. `lite` includes `llm` and `tui`. `git` and `ssh` are on the `dev`, `enterprise`, and `maximal` sets. A default `cargo build` therefore compiles `/git`, `/termws`, and the LLM routes. `inbound-smtp` is on Enterprise and Maximal. `embedded-postgres` is on Dev, Lite, and Maximal. Enterprise leaves that feature out and uses an external database.

`<terminal max-sessions="16" max-sessions-per-user="4"/>` limits terminal sessions. The browser socket is `/termws`. The screen is `/qapps/Terminal`, which requires authentication. SSH is a separate listener, under [SSH and the TUI](#ssh-and-the-tui).

## Host routes and servlets

Registration order is `listen` in `crates/moqui-host/src/http.rs`. Anything that does not match a host route falls through to screen dispatch.

| Surface | Path | Default | Authn | Authz | Considerations |
| --- | --- | --- | --- | --- | --- |
| Health | `GET /health` | on | TCP peer in `webapp_status_ips` (default `127.0.0.1`) plus `127.0.0.1`, `::1`, and `0:0:0:0:0:0:0:1`; otherwise 404 | that IP list | JSON `{"ready":true}` or `{"ready":false}` with 503 when pool stats are missing. The client-ip header is not used. |
| Metrics | `GET /metrics` | on | same IP check | same list | Plain text pool, guard, and (when a search replica exists) replica-lag gauges. 503 when pool stats are missing. |
| Notifications | `GET /notws` | on unless the endpoint `enabled` is `false`. Conf endpoint is enabled, timeout 3600000 | origin, then session-cookie MAC, else `login_key` | the signed-in user's notification topics | WebSocket. An origin outside the CORS list is 403. |
| Terminal | `GET /termws` | on when `tui` is compiled and the endpoint is not `enabled="false"`. Conf endpoint is enabled | origin, then session cookie | runs as that user; screens stay authorized; no extra permission | WebSocket bytes and one JSON resize. Counts against the terminal session limits. |
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

There is no axum route for `/groovysh`, `/rpc`, `/elastic`, `/kibana`, `/htmlr`, `/vapps`, or `/h2`. `/fop`, `/search`, and `/dashboard` are servlet patterns on the fallback. `/status` is a screen transition. The conf still has an endpoint `path="/groovysh"` `enabled="true"`. That element is a leftover: the listener never registers the path, and the Groovy Shell screen says this port has no Groovy and no script console.

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
| `/listViewColumns` | no `userId` check in the transition | `get#ListViewColumns` with authorization disabled. `values` is sliced to 500. The comment calls it status configuration read without authorization. |
| `/statusBar` | no `userId` check in the transition | `get#StatusBar` with authorization disabled. |
| `/shellCounts` | 401 when there is no `userId` | then `get#ShellCounts` with authorization disabled. |
| `/userStatus`, `/setUserStatus`, `/userStatusHeartbeat`, `/userStatuses`, `/chatTyping`, `/chatAssistCard`, `/chatUnread` | 401 when there is no `userId` | Chat and status services with authorization disabled, for the current user. `userStatuses` passes up to 500 ids from `userIds`. |
| `/richTextImage` POST | 401 when there is no `userId` | `upload#RichTextImage` with authorization disabled. Multipart field `contentFile`. The GET transition is `read-only`; its body was not read past the tag. |
| `/email/{emailMessageId}` | none; the id is the capability | `viewEmailMessage()` sends a 1x1 PNG (`image/png`, no charset, `Content-Disposition: inline`) for every id. A known `EmailMessage` whose status is not `ES_VIEWED` is updated to `ES_VIEWED` with `receivedDate` set, through `update#moqui.basic.email.EmailMessage` with authorization disabled. A suffix from the first `.` is removed when that `.` is past the first character (`NOTAREALID.png` marks `NOTAREALID`). A missing message is logged. A failure while marking is logged and the response stays the PNG. |
| `/robots.txt` | none | Disallows `apps`, `qapps`, `rest`, `status`, `menuData`, `shellData`, `shellSearch`, `shellCounts`, the user-status and chat paths, `richTextImage`, `llm`, `a2a`, `mcp`. |
| `/favicon.ico` | none | The file under the webroot directory. |

Other root screens that do not require a user: `Login` (`false`), `ChangePassword` (`false`), `SecondFactor` (`anonymous-all`), `echopath` (`false`, standalone, echoes extra path names), the `error` screens (`false`), `emailOAuth` (`false`), `sso` (`false`, compiled with the `sso` feature, which is on enterprise and maximal), `PublicShell` (`false`, chrome for token routes), `assist` (`false`), and `rest` (`false`). `apps` is `false` so its subscreens decide.

`Login` transitions include `login` (`require-session-token="false"`), `logout`, `removePreAuth`, `resetPassword`, `changePassword`, `createInitialAdminAccount`, and `sendOtp`. A user who is already signed in is redirected to `/`. `returnTo` is kept only when `WebUtilities.isSameOriginRedirect` accepts it (`Login.xml`). That helper allows a path that starts with `/`, and an `http` or `https` URL whose host equals the configured host. It refuses `javascript:`, `data:`, `vbscript:`, a leading `//`, a backslash, user info, and control characters. The port is not part of the host compare (`web/mod.rs`). An off-site value is logged and ignored. The initial-admin transition is the service described under passwords: it runs only while no real `UserAccount` exists beyond the seed row.

`apps` transitions: `setPreference`, `setPrefGoLast`, and `getPreferences` are 401 `Authentication required` without a `userId`. `l10nCatalog` and `qzSign` are 401 without a `userId`. `colorSchemeCss` is `read-only` and `require-session-token="false"`. The comment says a scheme is not private, so there is no login. It loads `ScreenColorScheme` and colors with authorization disabled. A scheme name must match `^[a-z][a-z0-9-]{0,40}$`. A color value is rejected when it matches `[;{}<>\\"'!]`. `scheme.extraCss` is appended after that check. It is not passed through the name or value checks. The response is `text/css`.

`apps` subscreens that were read: `AppList` and `ScreenTree` and `Terminal` require authentication. `ScreenTreeNested` sets `require-authentication="false"` and is a child of `ScreenTree`. Whether a direct URL's render chain always includes the parent was not traced. See [Open questions](#open-questions).

### Tools and System

`runtime/base-component/tools/MoquiConf.xml` mounts `system` and `tools` under `apps`. The URLs are `/apps/system` and `/apps/tools`, and the same names through the qapps extra path. There is no `/vapps` mount. `Tools.xml` and `System.xml` omit `require-authentication`, so both are required. `ADMIN` has inheritable always-all on each root, from the seed table above.

SQL Runner, Service Run, Data Import, Data Snapshot, ElFinder, and `qzSign` are designed privileged tools. A screen-by-screen re-check of write refusals (production limits on ElFinder, and the rest of the Java tool table) was not done. The extra permission checks that were read, on top of screen artifact authorization:

| Screen | Check |
| --- | --- |
| `Tools/GroovyShell.xml` | `GROOVY_SHELL_WEB`, then screen permission on the location. The widget text says the Groovy Shell is not available: this port has no Groovy and no script console. The screen has no eval endpoint. |
| `Tools/Entity/SqlRunner.xml`, `SqlScriptRunner.xml` | `SQL_RUNNER_WEB` |
| `Tools/Service/ServiceLoadRunner.xml` | `SERVICE_LOAD_RUNNER` |
| `Tools/dashboard.xml` | shows the three flags above |
| `System/Llm.xml` | `AgentAdmin`, or the screen returns an error. The comment says every screen under it needs `AgentAdmin`. |
| `System/Security/UserAccount/UserAccountDetail.xml` | `ADMIN_LOGIN_AS` for login-as, and a different username |

### Token routes

Token routes are not framework-default paths. `runtime/component/mr-boss/MoquiConf.xml` adds them on the `webroot` webapp. `screen/mod.rs` sends a match to token-route dispatch before the ordinary screen. The failure status of a bad token was not read.

| Name | URL | Token service | Screen |
| --- | --- | --- | --- |
| `s` | `/s/<token>` | `org.moqui.impl.FileServices.check#PublicShareToken` | `mr-boss` `PublicShare.xml` |
| `u` | `/u/<token>` | `mantle.party.ConsentServices.check#UnsubscribeToken` | `Unsubscribe.xml`, post-transition `oneClick` |
| `cal` | `/cal/<token>` | `ironstone.commons.CalendarServices.check#PublishedCalendarToken` | `PublishedCalendar.xml` |
| `r` | `/r/<token>` | `mantle.request.RequestServices.check#IntakeToken` | `RequestIntake.xml` |

mr-boss also adds a per-client rate limit `request-form` on `POST /r/*/submit`: 5 requests in 60 seconds, burst 5. It applies when per-client web protection is on.

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
| `SmatHmacSha256Timestamp` | `t` / `v1`. Skew 10 seconds, max age 300 seconds. Replay cache `moqui.security.hmac.replay`, type `distributed`, live 300 seconds |
| `SmatNone` | anonymous login, no check |

## LLM, A2A, MCP, Assist, git, and DAV

Routes exist when the `llm` feature is compiled. While a protocol is off, its handlers answer 404. `llm-facade` `enabled` is `true`. That element is the model facade. It is not the A2A server, the MCP server, or the gateway. Assist answers 503 when the facade is disabled; `bootstrap` still answers (`docs/Assist.md`).

A user with `EXTERNAL_USER` never holds `A2aServerUse` or `AgentUse`. The Assist contract says that permission also blocks the other agent permissions, so that user starts no run and gets no model call.

### A2A

Off. `a2a_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/a2a` and `GET /.well-known/agent-card.json` answer 404.

From `docs/A2A.md`: a caller needs `A2aServerUse` and `AgentUse`. Credentials are a Bearer login key, an `api_key` or `login_key` header, or Basic with an app password or a login key as the password. The account password is refused (401). Credentials are taken only over TLS, a believed proxy's https, or loopback. Missing credentials are 401 `WWW-Authenticate: Bearer realm="a2a", Basic realm="a2a"`. No permission is 403. The body max is 4 MiB. Waits and streams are at most 256 on a node (503) and 16 per user (429). Every request except the public card needs `A2A-Version: 1.0`. OIDC is not built. An agent is reached only when `exposeA2a="Y"` and the surface includes `a2a`. The seed `assist` agent does not. Writes wait for the human owner.

### MCP

Off. `mcp_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/mcp` and its documents answer 404.

From `docs/MCP.md`: a caller needs `McpServerUse` and `AgentUse`. Credential shapes match A2A, with realm `mcp`. No credentials is 401 JSON-RPC `-31401`. No permission is 403 `-31403`. `server/discover` needs no credentials. The body max is 4 MiB. The transport is `POST /mcp` only; GET and DELETE are 405. The seed agent `mcp` serves `mcp` with `exposeMcp="Y"`. The MCP client cannot approve a write. Public when on, and cached for an hour: `GET /mcp/server-card`, `/mcp/setup`, `/.well-known/mcp.json`, `/.well-known/ai-catalog.json`.

### Gateway

Off. `llm_gateway_enabled` is `false`. The live element is `<gateway enabled="${llm_gateway_enabled}" require-client="true" timeout="300"/>`. An `enabled="true"` example above it in the conf is inside a comment.

From `docs/Gateway.md`: Bearer login key or app password, or Basic with those. The account password is refused. TLS, a believed proxy's https, or loopback. Missing or bad credentials are 401 `invalid_api_key`. The caller needs `LlmGatewayUse` and an `LlmApiClient` row. An app password always needs a client. A login key needs one because `require-client` defaults to true. The body max is 16 MiB. The path is exempt from the webapp request deadline. The gateway `timeout` of 300 seconds answers 504. Paths: `GET /models`, `GET /models/{alias}`, `POST /chat/completions`, `POST /responses`, `POST /embeddings`. An unknown alias is 404. No tools run on the server: function-tool definitions pass through, and any other tool type is 400 `unsupported_tool`.

### Assist

The drawer is screen transitions on `assist.xml` (`require-authentication="false"`), not host routes. Each transition calls the Assist service. GET reads: `bootstrap`, `conversations`, `conversation`, `tasks`, `prompts`, `mentions`, `attachment`, `memories`, `skills`, `events`, `read`. POST: `turn`, `cancel`, `retry`, `follow`, `context`, `submitted`, `decide`, `preview`, `answer`, `feedback`, `queued`, `attachments`, `transcribe`, `textAction`, `memory`, `skill`, `canvasState`, `canvasAction`, `canvasAdjust`, `canvasProblems`.

`docs/Assist.md` is the contract for those calls. The browser session cookie is the login. Every call acts for the current user and needs `AgentUse`. Another user's conversation, run, approval, question, hand-off, or message is 404. A call from inside a run is 403. POST requires `X-CSRF-Token` or `moquiSessionToken`, and a missing token is 401 `Session token is missing or is not valid` before the call. The qapps shell includes the drawer when the llm feature is on and the user has `AgentUse`. Per-call tool policy inside the Rust services was not read.

`GET /assist/runs/{run_id}/events` and `.../events.json` authenticate like `/notws` (origin, session cookie, else `login_key`). The stream is for the run's owner. At most 256 streams; one more is 503 with `Retry-After` (`docs/Assist.md`).

`GET /assist/sandbox` is public and has no session. The host must be a plain name, or the answer is 400. Query `v` is a cache buster. `x=1` adds `https:` to `img-src` only. The iframe is `sandbox="allow-scripts"` without `allow-same-origin` (`docs/Assist.md`). The policy is the frame document, separate from the screen CSP:

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

From `docs/Git-Hosting.md`: HTTP Basic with the username and the password, or an access token or an app password as the password. An account with a second factor needs the token or the app password. The account password cannot finish that login. `Authorization: Bearer` and the `api_key` header also work. No credentials is 401. Credentials are accepted only over HTTPS (the TLS listener, or `X-Forwarded-Proto: https` from a believed proxy), from loopback, or when `allow-http="true"`.

Authorization is artifact type `AT_GIT_REPOSITORY`, name `repositoryId`. The server refuses to serve when that type's authorization is not enabled. VIEW is clone and fetch. CREATE is a new branch or tag. UPDATE is a fast-forward. DELETE is a force-push, a deleted ref, or settings. No view is 404. View without push is 403. Only `refs/heads/` and `refs/tags/` can be pushed. The default branch cannot be deleted. Seed `GitRepositories` is always-all for `ADMIN`. A `remote` repository is a read-only mirror. LFS is under `/git/<repositoryId>.git/info/lfs`. Webhooks are not sent to loopback, private, or link-local addresses unless `webhook-private-network="true"`.

SSH git is a channel on the SSH server, not this route. See [SSH and the TUI](#ssh-and-the-tui).

### DAV

The routes are always registered. There is no `<dav-server>` element in `MoquiDefaultConf.xml`. The defaults are the Rust fallbacks, and `docs/Sync.md` states the same numbers.

| Flag | Default | Effect |
| --- | --- | --- |
| `enabled` | true | `false` is 404 `the DAV server is off` |
| `allow-http` | false | otherwise credentials need a secure request: HTTPS, a believed proxy's https, or loopback |
| `account-password` | false | app password only. `true` also accepts the account password |
| `max-file` | 268435456 (256 MiB) | 413 over that body |
| `sftp-drives` | true | SFTP to drives for a user without `SSH_SFTP` |

OPTIONS is answered with no login: `DAV: 1, 2, 3, calendar-access, addressbook, extended-mkcol`, and `Allow` lists `OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, REPORT`.

Every other method is HTTP Basic. No credentials is 401 `WWW-Authenticate: Basic realm="Moqui", charset="UTF-8"`. Plain HTTP while `allow-http` is false is 403 `credentials over plain HTTP are refused; use HTTPS`. A path whose user segment is not the signed-in username (case-insensitive) is 404. Writes go through the file and adapter services as that user. `docs/Sync.md` paths: `/dav/`, `/dav/principals/<user>/`, `/dav/files/<user>/<drive>/<path>`, `/dav/calendars/...`, `/dav/addressbooks/...`. The framework keeps no calendar or contact of its own. An application serves them through adapter services, and Mantle sets those adapters.

`login_user` does not call `finish_login` when a second factor is required. DAV requires that login to succeed and requires a non-empty current user id. The wrapper that maps a second-factor result onto the current user was not read.

## Non-HTTP listeners

`/metrics` is on the public HTTP listener. It is not its own port. `port_check` probes HTTP, HTTPS, cluster, and SSH before the database opens. It does not probe SMTP.

### Mail

There is no open SMTP receiver. `smtp_listen_port` empty does not start one. The host default is `127.0.0.1`. There is no default port (the doc example is 2525; this process does not take port 25). `inbound-smtp` is an Enterprise and Maximal feature. Dev and Lite omit it. If the port is set on a build without the feature, startup logs an error and does not listen.

The listener is the last hop behind an operator-run gateway (`docs/Mail-Inbound.md`). There is no SMTP `AUTH` and no client certificate. The allow-list is the gateway. A peer outside `smtp_allowed_addresses` gets `554` at connect and the connection closes. An empty allow-list refuses to start the listener. `proxy-protocol`, `starttls`, and `require-tls` default to false. STARTTLS with no certificate does not start the listener. `dsn` defaults to false. The message cap is 25 MiB (`max-message-size` 26214400), 100 recipients, 20 connections.

A permitted gateway can `RCPT TO` a hosted (`EakHosted`) or shared (`EakShared`) address that has no incoming IMAP or POP server. Other recipients are `550`. After `DATA`, the message is stored in that account's Inbox, or Junk when a configured spam header starts with yes or true. That store runs with authorization disabled, including the app `EmailMessage` ECA when Mantle is loaded. There is no relaying. `VRFY` answers `252`. Empty `smtp_authserv_id` keeps every `Authentication-Results` header, including a forged one, and warns at startup. The listener does not check SPF, DKIM, or DMARC.

`poll#EmailServer` (job `poll_EmailServer_frequent`) is paused. If unpaused, the cron is every 15 minutes, `emailServerId=SYSTEM`, `authenticate="anonymous-all"`. It connects out to the configured IMAP or POP server. `sync#DueEmailAccounts` (job `sync_EmailAccounts_frequent`) is running, every minute, `authenticate="anonymous-all"`, `allow-remote="false"`. IMAP IDLE is on by default (`idle-enabled="true"`, `idle-max` 8) and dials the account's server. It does not listen. Both jobs use the scheduler's authorization-off path.

OAuth mail sign-in is not a listen port. It stays off until the operator sets a Google or Microsoft client id and secret. The browser session, PKCE, and server-side state (10 minutes, single use) protect the callback `/emailOAuth/callback`. The redirect URI is the deployment's and is never taken from the request. A server with no field encryption refuses to store the refresh token. The ID token signature is not checked. The callback transition's body was not fully read.

### SSH and the TUI

The SSH server is one `TcpListener`. SFTP, git, the admin shell, `apps`, `agent-apps`, and the SSH cluster channel are channels on it. `moqui tui` with no `--serve` and no `--server` opens no port (`docs/TUI.md`).

Off in the default conf: `ssh_port` empty, `ssh_host` `127.0.0.1`. `0` binds an ephemeral port. `./moq run` merges the dev conf, which sets `ssh_port` to `2222`, still on loopback unless `ssh_host` changes. `MoquiProductionConf.xml` does not set `ssh_port`. The `ssh` feature is in Dev, Enterprise, and Maximal. Lite leaves it out. `moqui tui --serve` starts this listener together with HTTP and the cluster listener.

Password login uses `login_user` (lockout counts, `ipAllowed` of the account and its groups). An account that needs a second factor is refused at the SSH password step. An app password is tried first when the secret has that shape. A public key must match an active `UserSshKey` by SHA-256 fingerprint, then `internal_login_user` with no password. Keyboard-interactive (`login-dialog` default true, and only if `tui` is built) asks no questions: the session has no user until the apps login dialog, the same username, password, and second factor as `moqui tui`. That dialog checks `ipAllowed` and closes after 2 minutes without a login or 5 refused logins. Pre-auth timeout is 30 seconds, and at most 16 connections have not logged in. The host key is `ssh_host_key`, generated into `runtime/.env`.

`SSH_SHELL` is seeded on `ADMIN_ADV` and is sealed, so the ordinary permission UI cannot grant it. A login with `SSH_SHELL` gets the admin shell (`help`, `status`, `service`, `entity` up to 100 rows, `cache clear`, `js` with `ec`, `apps`). Those commands run as that user. `service`, `entity`, and `js` also need Tools-app artifact authorization. Without `SSH_SHELL`, or after a dialog login, the session is only `apps`. `agent-apps` needs a key or password login and no pty. It drives the same client with that user's credentials. A dialog login gets no shell commands, no `agent-apps`, no SFTP, and no git. Git over SSH needs `SSH_GIT` (seeded on `ADMIN`) and a key, unless `git-password-auth` (default false). At most `max-sessions` 8 SSH connections. Terminal sessions also count against the terminal limits. Idle time is 10 minutes.

The SSH cluster channel is not a separate port. It is used when a peer's base URL is `ssh://`. The default transport is HTTPS, so the channel is off unless a member advertises SSH, and the SSH server must be listening. The login user is `moqui-cluster` with an Ed25519 key derived from `cluster_tls_key`. Any other key for that user is rejected. The channel carries the same four cluster topics. It is not a shell.

### SFTP

SFTP is the SSH `sftp` subsystem. Nothing listens for it unless the SSH server is up. `docs/Sync.md` "SFTP to the drives" is this subsystem. The outbound SFTP client dials other hosts.

`sftp-drives` defaults to true. `sftp-root` defaults to `runtime://sftp`. `sftp-max-file` defaults to 256 MiB. The user must already be logged in by key, password, or app password. A dialog login is refused. `SSH_SFTP` (seeded on `ADMIN`) selects the root: one resource location (`runtime://`, `file:`, `component://`, or `dbresource://`), paths normalized against `/`, and `..` that leaves the root is refused. Those calls run with authorization off. The permission checked at login is the gate. A user without `SSH_SFTP` gets the drives that user may open, when `sftp-drives` is true. `sftp-drives="false"` gives that user no SFTP. A write is an upload and a new version. A removal goes to the trash. At most 64 handles per channel.

### Cluster

The cluster HTTPS listener is separate from the public HTTP and HTTPS listeners. Public routes do not ask for a client certificate.

On whenever the process starts listeners (a normal `moqui` with no command, and `moqui tui --serve`). Empty `cluster_https_port` is **8444**. Empty `cluster_bind_address` is **0.0.0.0**. `0` is ephemeral. Commands that do not serve do not start it. `plans/archive/Cluster.md` still says port 8443. That line is stale. The running default is 8444 (`README.md`, `crates/moqui-host/src/cluster_edge.rs`).

A TLS client certificate is mandatory. The verifier accepts only a certificate whose SHA-256 matches this member's `cluster_tls_cert`. The server presents that same certificate. There is no HTTP login and no public app routing. After the handshake, only `POST /cluster/{topic}` for `entity-cache-invalidate`, `notification-message`, `collab-update`, or `session-end`. The body must be JSON with `origin_id` and `message_id`, or the answer is 400. An unknown topic or method is 404. The serialized JSON envelope limit is 16 MiB on HTTPS, SSH and Postgres NOTIFY (including payloads kept in `ClusterMessage`). Oversized messages are refused before fan-out. A 4xx response refuses that message without retrying or marking the peer down. At most 64 connections. Cache clears that entity's one-cache. `session-end` drops this process's live copies of the named session ids. Notification and collab messages are published on the local bus and are not fanned out again.

Postgres `LISTEN moqui_cluster` is off unless `cluster_transport=notify`. It needs a Postgres entity database. MySQL refuses startup. It is one connection outside the pool, not a new TCP port. The payload is not authenticated on its own. A payload over 7,900 bytes is a `ClusterMessage` row id. A notification missed while `LISTEN` is down is lost. Reconnect then clears entity caches. The default `cluster_transport` is empty, which means HTTPS POST to `cluster_base_url`.

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

IPP printing and the SFTP client are outbound. Git, DAV, `/termws`, and `/notws` are routes on the public HTTP server.

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

## How to use this for review

Walk each surface in the deployment you have: is it enabled, is it reachable from the networks you care about, how does it authenticate, what authorization (including inherit) applies, and what untrusted input does it accept. Application components add screens, services, and REST roots, and they need their own pass.

This file maps the surface. It does not prove the controls. The proof catalog is [SECURITY_TESTS.md](SECURITY_TESTS.md). The client suite those tests run is `runtime/base-component/framework/test/client/`. Reporting is in [SECURITY.md](SECURITY.md).

## Confirmed absent

| Surface | Where it was looked for | What this repository has |
| --- | --- | --- |
| Jetty, and a `web.xml` session store | `http.rs` binds a `TcpListener` and axum. A search for a file named `web.xml` under this tree found none. The name survives as a comment in `MoquiDefaultConf.xml` and as an exclusion in `plans/Parity-Mapping.md`. The conf comment still says "Embedded Jetty". | hyper, axum, and rustls. Sessions are `moqui.server.HttpSession` rows. Servlets and filters are conf elements read by `web/servlet.rs`. |
| H2 TCP server and the H2 console | No `/h2` route. No `H2Console` in `runtime/conf`. `entity_ds_db_conf=h2` is the embedded database name. Parity notes say the H2 console was removed and `H2ServerToolFactory` is not ported. Port 9092 is not a bind. | Embedded Postgres, Unix socket, no TCP. On Windows, TCP to `127.0.0.1` on an ephemeral port. |
| Jackrabbit on 8081 | The repository element is commented XML in the dev conf. `JackrabbitRunToolFactory` is not ported. | No process listens on 8081 for it. (`8081` elsewhere is an example HTTP port.) |
| SubEtha SMTP | No SubEtha code. The entity comment still names `SubEthaSmtpToolFactory`. Parity mapping says that factory is not ported. | Inbound SMTP, off until `smtp_listen_port` and a non-empty allow-list are set. Mail from anywhere but the gateway is refused. |
| `/groovysh` | Conf endpoint `enabled="true"`. `http.rs` does not route it. `Tools/GroovyShell.xml` says there is no Groovy and no eval endpoint. | The conf element only. |
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
- Assist authorization inside each Rust service body was not read. `docs/Assist.md` is the contract cited above (cookie session, `AgentUse`, 404 for another user's object, 403 from inside a run, CSRF on POST).
- `listViewColumns` and `statusBar` call services with authorization disabled and do not check `userId` in the transition (`webroot.xml`). Whether artifact authorization on the root transition still wraps those scripts was not read.
- The entity REST page size used after `pageNoLimit` is removed was not read. `rest-max-page-size` is 1000.
- Unix embedded Postgres: this tree sets an empty `listen_addresses` and password auth, and it does not set the socket-directory mode or `pg_hba.conf`. Whether another local user can open `.s.PGSQL.5432` is whatever `postgresql_embedded` 0.21 defaults to. The superuser password is in the cluster `pgpass` file.
- A peer that holds the cluster certificate can clear entity one-caches, push `notification-message` and `collab-update` onto the local bus, and drop named live sessions. How far a crafted notification body is rendered to a browser was not traced past `notify::publish`.

### Integration-fix cluster rollout

The compact `collab-update` envelope carries the update once, in `body.frame.update`. Receivers apply that update before broadcasting the frame to local editors. Incoming notification socket messages are capped at 16 MiB, and a collaboration update whose complete cluster envelope exceeds that limit is refused before local application or acknowledgement.

Upgrade and restart every member together, with collaborative editing stopped until all members run the new version. Mixed versions are unsupported for this wire-format change; older receivers expect a second top-level update and HTTPS receivers still enforce 1 MiB. No database migration is required.
