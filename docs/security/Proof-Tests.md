# Security proof tests

Named HTTP proofs for this repository. They record what is tested. They are not a pentest report and not a WAF claim.

- **Where:** [Attack-Surface.md](Attack-Surface.md)
- **What:** [OWASP Top 10:2025](https://owasp.org/Top10/) (plus WSTG / ASVS L1 ideas, and API Top 10 for `/rest`)
- **Public tests** are the client specs under `runtime/base-component/framework/test/client/`. Each feature name starts with the OWASP id, and a comment names the Moqui Java pytest it ports.

The line-by-line map of all 143 Java HTTP tests is [Java-Port-Catalog.md](../../runtime/base-component/framework/test/client/Java-Port-Catalog.md): 125 ported, 0 pending, 18 not applicable. This file is the control catalog and the notes on what a green run does and does not prove. Application components (Mantle, Marble, IronStone) are out of scope, as are the Java Selenium smokes and the A2A interop script.

Java's Spock suite (`Security*.groovy`) was not ported. A row below that names only a Spock class is not a claim that a Rust unit test covers it.

## How to run

```sh
./moq test-client framework --serve
```

`--serve` starts `moqui test framework --spec ClientFixtures --db snapshot --serve` on a scratch database, under `conf/MoquiLoadTestConf.xml` (`test/client/suite.js`). That file is the production conf with the screen, transition, and service tarpits off, so `LoadBasic` is not answered 429. `SecurityTarpit` skips on that conf. A skip there is the expected result of `--serve`.

`SecurityCors` fails its spec when the server allows every origin (the dev conf, `webapp_allow_origins=*`). The foreign-origin `/notws` proof skips on that conf. The other specs run on any conf.

Fixture users come from `runtime/base-component/framework/test/ClientFixtures.spec.js`. They are not demo seed. `sec.ip.v4`, `sec.ip.loop`, and `sec.mfa.totp` are rows only. Against a server `--serve` did not start:

```sh
moqui test framework --spec ClientFixtures --db existing
./moq test-client framework --server http://127.0.0.1:8080
```

Features that need a missing fixture user skip and name that command. The other proofs log in as `john.doe` / `moqui` (`--param user=`, `--param password=`). None sends that user a wrong password (the fourth failure locks the account, `max-failures="3"`), and none logs it out over REST (`UserAccount.hasLoggedOut` ends its other sessions). Wrong-password and lockout use `sec.lock.test`.

### Tarpit on the production conf

`runtime/conf/MoquiProductionConf.xml` leaves the tarpits on. Start a scratch server with the fixtures, then run only `SecurityTarpit` against it. Both features must be 429. A skip on this conf means the tarpit or the fixture did not engage.

```sh
MOQUI_CONF=conf/MoquiProductionConf.xml \
  ./moq test framework --spec ClientFixtures --db snapshot \
  --serve 127.0.0.1:0 --ready-file /tmp/moqui-tarpit.url
# when the ready file holds a URL:
./moq test-client framework --spec SecurityTarpit --server "$(cat /tmp/moqui-tarpit.url)"
```

Stop the server when the client exits (its standard input is the terminal). The screen proof is the demo `ALL_SCREENS` row (120 hits in 60 seconds) on `GET /apps/tools/dashboard` as `john.doe`. The transition proof is `sec.tap.user`, 3 hits in 60 seconds, on `POST /apps/setPreference`.

## Catalog

| Control | OWASP | Client spec |
| --- | --- | --- |
| Unauthenticated Tools is the login page | A01 | `SecurityAccess` |
| `/rest/e1` and `/rest/s1` without credentials are 401 | A01 | `SecurityAccess` |
| Removed `/rest/api_key`, `/rest/moquiSessionToken`, `/rest/userInfo` are 404 | A01 | `SecurityAccess` |
| `POST /rpc/json` is 404 | A01 | `SecurityAccess` (the path is gone; the JSON-RPC behaviors below are not applicable) |
| `getPreferences` without a login is 401 and has no preferences | A01 | `SecurityAccess` |
| A static path with `..` does not serve screen XML | A01 | `SecurityAccess` |
| A user with no artifact authz cannot open Tools | A01 | `SecurityAccess` |
| A VIEW-only user cannot open SQL Runner | A01 | `SecurityAccess` |
| Login `returnTo` off the site is refused; a path on the site is followed | A01 | `SecurityAccess` |
| Login `Referer` off the site is refused | A01 | `SecurityMisc` |
| Login `returnTo` with `Host: evil.example` does not redirect to that host | A01 | `SecurityMisc` |
| CSP (`frame-ancestors`, `form-action`), `X-Frame-Options`, nosniff | A02 | `SecurityHeaders` |
| Session cookie is `moqui.session`, HttpOnly, SameSite=Lax | A02 | `SecurityHeaders` |
| Login HTML is not publicly cacheable | A02 | `SecurityHeaders` |
| No `Origin` header means no `Access-Control-Allow-Origin` | A02 | `SecurityHeaders` |
| No H2 console | A02 | `SecurityHeaders` |
| Unknown origin, and its preflight, are 401 with no allow-origin | A02 | `SecurityCors` |
| The server's own origin gets allow-origin and credentials; its preflight is allowed (204) | A02 | `SecurityCors` |
| The server's host on another port or scheme is 401 | A02 | `SecurityCors` (no Java pytest; the origin compare includes scheme, host, and port) |
| REST login needs no session token; an unknown user is 401 with the shared text | A07 | `SecurityAuthn` |
| A POST with the session token runs; without it the preference is not stored | A01 | `SecurityAuthn` |
| Login issues a new session id | A07 | `SecurityAuthn` |
| `api_key` in the query, or a garbage `api_key` header, does not log in | A07 | `SecurityAuthn` |
| Basic on `/rest/e1`: unknown user 401, valid user is authenticated | A07 | `SecurityAuthn` |
| A REST 401 in JSON has no stack frames | A10 | `SecurityErrors` |
| `/notws` from a foreign origin is refused (logged-in session) | A01 | `SecurityWebSocket` |
| An anonymous `/notws` upgrade is 401 | A01 | `SecurityWebSocket` |
| The server's host on another port or scheme is refused for `/notws` | A01 | `SecurityWebSocket` |
| Screen proofs: escaped extra path, FOP without login, login form action, VIEW-only service run, anonymous data import, `qzSign`, `GET` login with a password, `WEB-INF`, `GET /notws`, SQL Runner query string, menu data, FOP `filename`, DataSnapshot download and parent segments, ElFinder put, Tools CSV, remote data import | A01, A03, A05 | `SecurityScreens` |
| `/email/NOTAREALID.png` is HTTP 200, `image/png` with no charset, inline, and the 1x1 tracking PNG | A01 | `SecurityScreens` |
| `/htmlr` and `/htmlr/...` are 404, not a PDF and not a stack trace | A10 | `SecurityScreens` (the renderer is not mounted) |
| `/status` from loopback is public JSON (no heap, datasources, or VmVendor) | A01 | `SecurityStatus` |
| `X-Forwarded-For` and `Forwarded` do not change that loopback result | A01 | `SecurityStatus` |
| `/search` and `/dashboard` without a login are 401; without `SearchRemote` the denial is not 401 | A01 | `SecurityProxy` |
| Basic and `api_key` on `/search`: no permission 403, bad password 401, garbage key does not authenticate, `SearchRemote` is past authorization | A01 | `SecurityProxy` |
| Entity REST and service REST authz, method override, CSRF on create, password fields omitted, identity fields, email server host, `UserLoginKey` | A01 | `SecurityRest` |
| `/rest/entity.json`: anonymous 401, no `REST_SCHEMA` 403, ADMIN 200 on one entity document | A01 | `SecurityRest` |
| `/rest/sm` HMAC missing or bad is 403; a valid signature needs no login; timestamp outside the window is 403; inside the window is accepted; a replay is rejected; `SmatNone` accepts an anonymous post; an unknown remote is rejected | A01 | `SecurityMessages` |
| Service Run session token, `api_key` and `login_key` headers from the fixture login key, Basic on a screen, shared wrong-password text, OTP pre-auth, MFA, initial admin, short password, login JSON omits credentials, password-reset enumeration, IP allow list, lockout | A07 | `SecurityAuthnMore` |
| Unauthorized and error HTML have no script tag or stack frame | A10 | `SecurityMisc` |
| A SQL-looking preference is stored as data | A05 | `SecurityMisc` |
| Multipart upload: `MZ` and a `#!` script are rejected; a ZIP is accepted | A02 | `SecurityMisc` |
| Screen tarpit and the `sec.tap.user` transition tarpit are 429 | A01 | `SecurityTarpit` (production conf; skips under `--serve`) |

HMAC signing in the client is `hmac.sha256Base64` and `hmac.sha256Timestamp` ([Client-Testing](../dev/Client-Testing.md)). They match `SmatHmacSha256` and `SmatHmacSha256Timestamp`.

### Not applicable

These Java HTTP tests have no port. The reason is the behavior they call.

| Java test | Reason |
| --- | --- |
| Six `/groovysh` tests (HTTP GET, anonymous upgrade, admin eval, anonymous-after-admin, VIEW-only, same-origin admin) | No Groovy shell and no `/groovysh` route. The conf still has an endpoint element `path="/groovysh"` `enabled="true"`. The listener does not register it. |
| Eight JSON-RPC tests, including `api_key` in a JSON body, array bodies, and `reset#Password` over `/rpc/json` | `/rpc/json` is 404 on purpose (`SecurityAccess`). |
| `test_entity_sync_put_needs_all_authz`, `test_entity_sync_put_with_all_authz_stores`, `test_entity_sync_put_does_not_store_user_group_permission` | Those proofs call `put#EntitySyncData` over `/rpc/json`. No REST stand-in was added. |
| `test_rest_schema_anonymous_is_401`, `test_rest_schema_does_not_set_acao_wildcard` | Both hit `/rest/entity.swagger`, which 404s before auth. Swagger 2 is not built. The schema gate is the `/rest/entity.json` rows in `SecurityRest`. |

### In-process proofs that were not ported

Java's Spock classes check these without an HTTP client. This pass did not look for a matching Rust unit test, and this catalog does not claim one.

- Conf and utility flags: SubEtha and Jackrabbit factories absent, `upload-executable-allow` default false (the HTTP magic check is ported), shipped `entity_ds_crypt_pass` still `CHANGEME` until startup replaces it, password and login-key hash types, `allow-html` `safe` and `any`, HSTS on `screen-secure` only when the request is HTTPS, `email_allowed_hosts` empty, HMAC replay cache declared `distributed`, percent-encoded query names dropped, `getClientIp` address shapes.
- Integrity: component zip-slip and a contained-entry expand, XML external entities in `MNode`, ElFinder `joinUnderRoot` and hashed `..`, `create#UserAccount` ignoring extra password fields, DataSnapshot `disableEntityEca` restoring `ADMIN` membership, denied entity find writing `ArtifactAuthzFailure`.
- Authn beyond the HTTP rows: lockout re-enable after `disable-minutes`, password-reset token single use and expiry, `hasLoggedOut`, `loginUserKey` stored as a hash, WebSocket handshake `api_key` in the header versus the query, a handshake not keeping the previous user, `AUTHZA_ALL` without `SQL_RUNNER_WEB` or `GROOVY_SHELL_WEB`.
- Login history stored, and a failed login not storing the password.
- `/llm/v1` CSRF, a dummy `login_key` or Basic not skipping it, and an empty `allowed-path` failing closed (`LlmServletTests`). A2A with `a2a_enabled` true. HOLD `TransactionCacheDb` not writing production rows.
- Dependency versions that do not exist here (Log4j, Shiro) and the H2 TCP server.

## Recorded differences

Kept on purpose, and asserted by the specs. The parity notes are [Parity-Diffs](../../plans/Parity-Diffs.md) (P9-06 and the client-test-harness table).

- A preflight is 204 with an empty body.
- The session cookie is `moqui.session`.
- A missing or wrong session token is 401 `Session token is missing or is not valid`.
- An anonymous `/notws` upgrade is 401. The foreign-origin proof uses a logged-in session.
- CORS and `/notws` compare scheme, host, and port. The same host on another port or scheme is refused.
- An IP-refused REST login is 401 and not logged in. The body names the user and the client address (`check_ip_allowed`). The spec does not require Java's shared invalid-login sentence. `X-Forwarded-For` does not supply the allow-list address while `webapp_client_ip_header` is empty.

`WebUtilities.isSameOriginRedirect` with no host argument uses the webapp `http-host`, which is empty in the default conf, so an absolute `returnTo` is refused. The login screen calls that one-argument form. The request `Host` header is not a configured host. `SecurityMisc` is the proof.

Three `SecurityScreens` assertions are tighter than the Python ones they port. A VIEW-only service run counts only 401 or 403 (a 302 would be the run succeeding). An anonymous data import counts a 302 only when `Location` is the login. A `GET` of the login with the demo password must still leave the session on the login page.

## Proof strength

- `--serve` does not prove the tarpit. `SecurityTarpit` skips while the status is not 429, which is correct for the load-test conf and is a failed proof on the production conf.
- `/status` is requested from loopback. The three features prove the public JSON shape and that forwarded headers do not hide it. They do not prove that a TCP source outside `webapp_status_ips` is rejected.
- The HMAC replay cache is local unless a distributed cache is configured. A second server does not see the first server's replay.
- With `search_backend` at its default `postgres`, `/search` and `/dashboard` are 404 after authentication. The `SearchRemote` feature asserts the response is neither 401 nor 403.
- The ElFinder feature posts `cmd=put` at the volume root `v0_cm9vdA` and expects JSON containing `write not allowed` or `error`, and not `changed`. That is the root-refusal on every conf. It does not prove a production-only block of writes under `component://`.
- The menu feature passes when the body is not the Tools menu, including when the status is 500. An anonymous `GET /menuData/apps/tools` logged `cannot read property 'extraPathNameList' of undefined` (`screen_host.rs`). The in-process menu test expects 401 once `getMenuData` runs. The HTTP proof does not require that status.
- The email pixel proof is `GET /email/NOTAREALID.png`: status 200, `image/png`, inline, and the PNG magic. The client harness records the first four bytes as hex (`89504e47`). The full 68-byte image, and marking a known message `ES_VIEWED` with `receivedDate` set once, are the unit test `view_email_message_sends_the_png_and_marks_a_known_message`. That is Java's `markEmailMessageViewed` side effect. The Spock method that calls it was not ported.
- `/htmlr` features prove the path is 404. They do not prove an HTML renderer's error page.
- Positive controls sit next to the refusals: a session token that stores a preference, a same-origin CORS allow, a valid HMAC and a timestamp inside the window, `SmatNone` ingest, a loopback-allowed user logging in, an ADMIN schema document, a catch-all entity ALL that creates, a Tools CSV for a user who may open Tools, and a ZIP upload that is accepted. A refusal alone would still pass if the feature had stopped working.
- `requireFixture` skips only on HTTP 401. An MFA password-only login is 200 with `secondFactorRequired` and no `loggedIn`, so those features run. The success feature consumes the single-use code and is last. `ClientFixtures` refreshes the code, and `--db snapshot` is a fresh database.
- The lockout feature is last in `SecurityAuthnMore` and does not log `sec.lock.test` in afterwards.
