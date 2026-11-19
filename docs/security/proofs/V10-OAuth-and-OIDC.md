# V10 OAuth and OIDC

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V10 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V10.1 Generic OAuth and OIDC Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.1.1 | 2 | Tokens held only where they are used | Gap | Met for the browser: the SSO access token is used once server side for user info and dropped (`moqui-host/src/sso.rs` `complete_async`), the ID token stays in the session; mail refresh and access tokens are encrypted server side and the popup page carries no token (`moqui-mail/src/oauth.rs` `send_page`); FCM and Sinch tokens stay in process. Not met: a mail account manager repoints its server and the XOAUTH2 token goes there (SEC-MAIL-84), a provider row's `tokenUrl` gets refresh tokens (SEC-OUT-82) | PASS: to write: in-process |
| v5.0.0-10.1.2 | 2 | Callback values tied to the starting browser session | Framework | SSO: huskarl 0.11.10 random 32-byte `state` and `nonce` and an S256 verifier in the session's pending state (`service/sso.rs` `login`), taken and removed before the callback is used (`callback`); mail: state, verifier and nonce in the session, taken once, same user, 10 minutes (`oauth.rs` `take_pending`, `finish_pending`); app client 48-byte verifier, 24-byte state (`moqui-client/src/sso.rs` `start`, `callback`) | FW: moqui-host it `sso::an_oidc_login_creates_the_user_maps_groups_and_rotates_the_session` (tampered state), moqui-mail `oauth_tests::the_code_exchange_checks_state_pkce_scope_nonce_and_address`; PASS: mr-pass SsoHandoff "SSO callback is browser-bound ..." |

## V10.2 OAuth Client

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.2.1 | 2 | Code flow protected from forged token requests | Framework | huskarl constant-time `state` check (`grant/authorization_code/flow.rs` `check_state`) and PKCE S256 (`plain` only when the document lists `plain` alone); mail `state` plus PKCE S256 (`oauth.rs` `start_sign_in`, `exchange`) | FW: as 10.1.2; PASS: SsoHandoff "SSO callback is browser-bound ..." |
| v5.0.0-10.2.2 | 2 | Defense when several authorization servers are used | Gap | one `/sso/callback` for every flow; the RFC 9207 `iss` is checked only when the discovery document advertises it (huskarl `flow.rs` `complete_inner`); OAuth 2 flows and mail (`/emailOAuth/callback`, Google and Microsoft) check none; confidential clients limit a misdirected code; no register id | - |

## V10.3 OAuth Resource Server

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.3.1 | 2 | Resource server checks a token's audience | N/A | not an OAuth resource server: `/mcp`, `/a2a`, `/llm/v1` take Moqui login keys and app passwords as `Bearer` (`moqui-llm/src/a2a/mod.rs` `log_in`, `gateway.rs`), no OAuth access token; MCP OAuth not built (`mcp/mod.rs` `www_authenticate`) | PASS: mr-pass PublicIdentity "completing the factor issues a client and site scoped credential" |
| v5.0.0-10.3.2 | 2 | Access decided by the token's delegation claims | N/A | as 10.3.1 | PASS: to write: black-box |
| v5.0.0-10.3.3 | 2 | User taken from claims that never move | N/A | as 10.3.1 | PASS: to write: in-process |
| v5.0.0-10.3.4 | 2 | Token's sign-in strength or age checked | N/A | as 10.3.1 | PASS: mr-mantle SmithyGitAccess "an account with no password to give again signs in again instead" |

## V10.4 OAuth Authorization Server

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.4.1 | 1 | Redirect URIs matched exactly to registered ones | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.2 | 1 | Code used once; reuse revokes its tokens | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.3 | 1 | Authorization code lives at most ten minutes | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.4 | 1 | Each client limited to its needed grants | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.5 | 1 | Refresh token replay prevented for public clients | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.6 | 2 | PKCE required, S256 only, verifier checked | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.7 | 2 | Open client registration vetted and consented | N/A | no client registration: the handoff knows only the configured redirect URIs | - |
| v5.0.0-10.4.8 | 2 | Refresh tokens have a hard expiry | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.9 | 2 | Users can revoke refresh and reference tokens | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.4.10 | 2 | Confidential clients authenticate on back-channel calls | N/A | the handoff's clients are public installed apps (PKCE, no secret) | PASS: SsoHandoff "SSO handoff uses the registered callback ..." (wrong secret) |
| v5.0.0-10.4.11 | 2 | Clients given only the scopes they need | N/A | no scopes: the handoff's clients are mr-moqui's own apps and a device key acts as the account (likely; see NOTES) | PASS: PublicIdentity "completing the factor issues a client and site scoped credential" |

## V10.5 OIDC Client

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.5.1 | 2 | ID token nonce matched to the request | Framework | every OpenID flow sends a nonce (`moqui-host/src/sso.rs` `.oidc(true)`; `OidcFlow.useNonce` is not read) and huskarl requires it (`token/id_token.rs` `validate_inner`); mail `check_id_token` compares it in constant time | FW: moqui-mail `oauth_tests::the_code_exchange_checks_state_pkce_scope_nonce_and_address`; SSO: to write: in-process |
| v5.0.0-10.5.2 | 2 | User identified by a stable provider subject | Gap | met: an account is linked by the provider's `sub` with its flow (`service/sso.rs` `linked_account`, `externalAuthFlowId`); not met: user info members are merged with no `sub` compare, so another subject's `email` or `preferred_username` can name or link the account (SEC-WEB-86) | - |
| v5.0.0-10.5.3 | 2 | Provider metadata issuer must match the expected one | Gap | the discovery document is parsed in `moqui-host/src/sso.rs` `Inner::built` and its own `issuer` is trusted; `OidcFlow` has no issuer field and the issuer is not compared with the discovery URL (huskarl's `AuthorizationServerMetadata::fetch` check is not used); Attack-Surface-Admin says the same; no register id | - |
| v5.0.0-10.5.4 | 2 | ID token audience is this client | Framework | huskarl takes an `aud` of the client id with no other audience (`id_token.rs` `validate_inner`); mail `check_id_token` takes a list that holds the client id without refusing others (token straight from the token endpoint; see NOTES) | FW: to write: in-process |
| v5.0.0-10.5.5 | 2 | Back-channel logout tokens checked | N/A | no OIDC back-channel logout endpoint (none in `webroot/sso.xml` or the crates) | - |

## V10.6 OpenID Provider

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.6.1 | 2 | Provider allows only safe response types | N/A | not an OpenID provider: issues no ID token (the app handoff answers a device key) | - |
| v5.0.0-10.6.2 | 2 | Provider resists forced logout | N/A | as 10.6.1 | - |

## V10.7 Consent Management

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-10.7.1 | 2 | User consents to each authorization | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.7.2 | 2 | Consent prompt explains access, client and lifetime | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |
| v5.0.0-10.7.3 | 2 | User reviews and withdraws granted consents | N/A | mr-moqui is not an OAuth authorization server (see the notes below) | - |

## Notes

No part of mr-moqui is an OAuth authorization server or an OpenID provider, and none is an OAuth
resource server: `/mcp`, `/a2a` and `/llm/v1` take mr-moqui's own login keys and app passwords. So sections 10.4
and 10.7 do not apply. Two handoffs are like an authorization server without speaking OAuth: the installed app's
sign-in (`crates/moqui-core/src/service/sso_app.rs`, a code with PKCE to a redirect address after a consent page,
ending in a device key) and the PASS authority's provider handoff and delegations. Read against these sections they
show the following; the device key's renewal and lifetime are session matters, graded under [V7](V7-Session-Management.md).

- v5.0.0-10.3.1 (Resource server checks a token's audience), the PASS authority's provider handoff and delegations: delegation found by token, client and site (`public/Account.js` `authenticateDelegation`), taken only from a registered client (`mr-mantle/script/PassAuthority.js` `run`)
- v5.0.0-10.3.2 (Access decided by the token's delegation claims), the PASS authority's provider handoff and delegations: the delegation's account, client and site decide: the call runs as its account (`internalLoginUser`), the site is the path's and in the client's `@sites` (`PassAuthority.js` `run`); no scopes are issued
- v5.0.0-10.3.3 (User taken from claims that never move), the PASS authority's provider handoff and delegations: `DelegatedSession.userId`, the framework account id (`authenticateDelegation`)
- v5.0.0-10.3.4 (Token's sign-in strength or age checked), the PASS authority's provider handoff and delegations: the delegation's `issuedAt` is the visitor's sign-in time for Smithy's recent sign-in (`PassAuthority.js` `passSignedInAt`, `SmithyGit.js` `reauthenticate`, SEC-PFS-63 fixed); an SSO delegation's issue is not the provider's `auth_time`
- v5.0.0-10.4.1 (Redirect URIs matched exactly to registered ones), the installed app's sign-in handoff: app handoff (`service/sso_app.rs` `Redirects::allows`): listed URIs exact, checked again at the exchange; by default also any `127.0.0.1` or `[::1]` URI, any port and path; `webapp-list/webapp[webroot]@app-loopback-redirect="false"` leaves the exact list `@app-redirect-uris`
- v5.0.0-10.4.1 (Redirect URIs matched exactly to registered ones), the PASS authority's provider handoff and delegations: callback only the client's `pass-authority/client[<id>]@callback`, never a request value (`public/Account.js` `prepareSso`)
- v5.0.0-10.4.2 (Code used once; reuse revokes its tokens), the installed app's sign-in handoff: app code taken once before any check (`sso_app.rs` `exchange_app_code`, `usedDate`); the device key of the first use is not revoked on a replay; no register id
- v5.0.0-10.4.2 (Code used once; reuse revokes its tokens), the PASS authority's provider handoff and delegations: `exchangeSso` refuses a consumed handoff (row locked); the delegation of the first use is not revoked on a replay
- v5.0.0-10.4.3 (Authorization code lives at most ten minutes), the installed app's sign-in handoff: app code 60 s (`sso_app.rs` `CODE_MILLIS`)
- v5.0.0-10.4.3 (Authorization code lives at most ten minutes), the PASS authority's provider handoff and delegations: handoff code 60 s (`public/Account.js` `completeSso` `codeExpiresAt`)
- v5.0.0-10.4.4 (Each client limited to its needed grants), the installed app's sign-in handoff: the handoff's only exchange is a code with its PKCE verifier (`/sso/appToken`, `exchange_app_code`); no implicit flow; the apps' password sign-in for a device key is Moqui's own login (V6)
- v5.0.0-10.4.4 (Each client limited to its needed grants), the PASS authority's provider handoff and delegations: `exchangeSso` takes a code and verifier only; a password sign-in through a registered frontend (`account/session`, then `issue#Delegation`) is the site's own login with its second factor (likely; judgment, see NOTES)
- v5.0.0-10.4.5 (Refresh token replay prevented for public clients), the installed app's sign-in handoff: (analogue) the apps' device key renews itself (`user/login.rs` `renew_device_key`): the old key works 10 minutes more and may renew again in them; no reuse detection, not sender-constrained
- v5.0.0-10.4.6 (PKCE required, S256 only, verifier checked), the installed app's sign-in handoff: `sso_app.rs` `check_start` (S256 only, 43-character challenge), `exchange_app_code` (`verifier_ok`, `challenge_of`)
- v5.0.0-10.4.6 (PKCE required, S256 only, verifier checked), the PASS authority's provider handoff and delegations: (equivalent) the verifier's digest is set at `prepareSso` by the authenticated client over the back channel and checked at `exchangeSso`; nothing of it crosses the browser
- v5.0.0-10.4.8 (Refresh tokens have a hard expiry), the installed app's sign-in handoff: (analogue) each device key renewal gets a full `login-key@device-expire-hours` (2160) from now, no cap from the first sign-in (`renew_device_key`)
- v5.0.0-10.4.8 (Refresh tokens have a hard expiry), the PASS authority's provider handoff and delegations: (analogue) no refresh tokens; a delegation slides 15 minutes on use, never past 12 hours from issue (`authenticateDelegation`)
- v5.0.0-10.4.9 (Users can revoke refresh and reference tokens), the installed app's sign-in handoff: device keys listed and signed out by their user (`service/user_keys.rs` `get_device_keys`, `revoke_device_key`; desktop Devices through `qapps.xml` `clientDevices`, `revokeClientDevice`), all at once by `revoke#OwnAccess`
- v5.0.0-10.4.9 (Users can revoke refresh and reference tokens), the PASS authority's provider handoff and delegations: sign-out revokes the delegation (`PassAuthority.js` `accountAction` `revokedAt`), a password change ends all (`accountStamp`); no list of a visitor's delegations
- v5.0.0-10.4.10 (Confidential clients authenticate on back-channel calls), the PASS authority's provider handoff and delegations: the registered client's secret compared by digest (`public/Account.js` `clientRegistered`, `System.propertyMatches`) on `sso-prepare`, `sso-exchange`, account actions and every delegation; gRPC by its secret or the edge link
- v5.0.0-10.4.11 (Clients given only the scopes they need), the PASS authority's provider handoff and delegations: a client is registered for its `@sites`; a delegation is for one client and site (`issueDelegation`, `authenticateDelegation`)
- v5.0.0-10.7.1 (User consents to each authorization), the installed app's sign-in handoff: every app sign-in asks the user at `/sso/appCode` and takes only a POST with the session token and the shown `pendingId` (`sso_app.rs` `confirm_app_login`, `issue_app_code`); no remembered consent
- v5.0.0-10.7.2 (Consent prompt explains access, client and lifetime), the installed app's sign-in handoff: the page names the app (as its own word), its port or redirect URI, the user, and that the key is revocable (`sso_app.rs` `app_text`, `confirm_app_login`); not the key's lifetime (90 days, renewable) nor that it acts as the whole account
- v5.0.0-10.7.3 (User reviews and withdraws granted consents), the installed app's sign-in handoff: the key a consent made is listed and revoked by its user (`get#UserDeviceKeys`, `revoke#UserDeviceKey`, `revoke#OwnAccess`); nothing to modify (no scopes)
