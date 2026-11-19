# V7 Session Management

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V7 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V7.1 Session Management Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.1.1 | 2 | Idle and absolute session limits documented with rationale | Gap | `webapp-list/webapp[webroot]/session-config@timeout` 60 min is documented (Attack-Surface-Web.md "Session cookie", conf comment), with "there is no absolute lifetime"; no rationale against NIST 800-63B anywhere in `docs/` | - |
| v5.0.0-7.1.2 | 2 | Concurrent sessions per account: limit and behavior documented | Gap | no per-account web session limit or policy; `session-config@max` (10,000) is a global live-table cap that evicts to the row and ends nothing (`session.rs` `evict_over_cap`); only `ssh-server@max-sessions-per-user` and `terminal@max-sessions-per-user` exist | - |
| v5.0.0-7.1.3 | 2 | Federated sign-in systems and session coordination documented | Application | the deployer's IdPs; the framework's RP side is documented: Attack-Surface-Routes.md `/sso/*` row, Recent-Auth.md "What counts as a sign-in" (`prompt=login`, `max_age=0`, `auth_time`); logout goes to the IdP's end-session URL (`moqui-host/src/sso.rs`, `id_token_hint`); no IdP-initiated logout | - (document) |

## V7.2 Fundamental Session Management Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.2.1 | 1 | Server checks every session token | Framework | `crates/moqui-core/src/user/session.rs` `verify_session_cookie` (HMAC-SHA256 under `session-config@cookie-key`; a bad MAC is no session), then the live table or the `HttpSession` row (`adopt_presented`, `install_from_row`) | V7SessionManagement "v5.0.0-7.2.1: a session cookie changed by the client signs nobody in"; `a_bad_mac_is_an_unknown_session` (`session_tests.rs`) |
| v5.0.0-7.2.2 | 1 | Sessions use generated tokens, not fixed keys | Framework | a new random id and token for each session (`session.rs` `resolve_id`, `random_hex`); `api_key`/`login_key` headers are expiring per-request credentials (`login.rs` `check_login_key`), not the browser session | V7SessionManagement "v5.0.0-7.2.2: a session cookie the client makes up is not taken; the server issues its own"; SecurityAuthn "A07 the login form issues a new session id" |
| v5.0.0-7.2.3 | 1 | Session reference tokens random, unique, 128 bits or more | Framework | session id and session token 20 bytes (160 bits) from `getrandom` (`session.rs` `SESSION_ID_BYTES`, `SESSION_TOKEN_BYTES`, `random_hex`), the cookie MACed as well | V7SessionManagement "v5.0.0-7.2.3: a session id is at least 128 bits, a new one for each session"; `session_tests.rs` `session_ids_and_tokens_are_160_bits_from_the_operating_systems_random_source` (in-process, the generator) |
| v5.0.0-7.2.4 | 1 | New session token at sign-in and re-authentication | Gap | met for password, second factor, key, app password and SSO sign-ins (`session.rs` `rotate_on_password_login`, `login.rs` `internal_login`; SEC-WEB-05 fixed); not for a re-authentication (`reauthenticate#User` keeps the id: `user/factor.rs` `reauthenticate`, Recent-Auth.md) nor a password login of the session's own user (`login.rs` `authenticate`, `same_user`); seen in code, not in the register | - |

## V7.3 Session Timeout

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.3.1 | 2 | Idle timeout ends the signed-in session | Framework | `webapp-list/webapp[webroot]/session-config@timeout` 60 min by default (`session.rs` `session_timeout_millis`, `is_idle`, `adopt_presented`, `expire_idle`); 0 turns it off; terminal typing counts as use (`note_use`) | `an_idle_session_expires_and_its_row_is_deleted` (`session_tests.rs`) |
| v5.0.0-7.3.2 | 2 | Absolute lifetime ends the signed-in session | Gap | none: a session ends only by idleness (`session.rs` has no age check; Attack-Surface-Web.md "Session cookie"); the installed apps' device key renews itself with a full lifetime each time (`user/login.rs` `renew_device_key`, `user-facade/login-key@device-expire-hours` 2160), so an app's sign-in has none either ([V10 notes](V10-OAuth-and-OIDC.md#notes)) | - |

## V7.4 Session Termination

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.4.1 | 1 | Ended session cannot be used again | Framework | logout deletes the row, retires the id, publishes the end to the cluster and sets `hasLoggedOut` (`session.rs` `renew_after_logout`, `login.rs` `logout`; SEC-WEB-06 fixed); idle rows deleted (`expire_idle`); an LFS token already issued lives to its expiry, 15 or 60 min (Attack-Surface-Git.md) | V7SessionManagement "v5.0.0-7.4.1: a signed-out session's cookies do not sign in again"; `session_tests.rs` `a_logout_ends_the_session_and_its_attributes` |
| v5.0.0-7.4.2 | 1 | Disabled or deleted account loses its sessions | Framework | each request's bind signs out an account disabled for good or past `terminateDate` (`session.rs` `refresh_from_account`, `login.rs` `account_closed`); keys refused (`login.rs` `gate_account`); sockets at their 60 s check (map); a tarpit lock keeps sessions by design; a deleted `UserAccount` row is not caught (`refresh_from_account` returns when no row) | `every_session_of_a_user_ends_and_a_disabled_account_signs_out` (`session_tests.rs`); PublicIdentity (mr-pass) "backend account disable revokes an otherwise unexpired credential" |
| v5.0.0-7.4.3 | 2 | Offer ending other sessions after a factor change | Application | a password change ends the account's other sessions on every server (`UserServices.xml` `update#PasswordInternal` `UserSecurityChecks.endUserSessions`); a second-factor change offers nothing; tool: `org.moqui.admin.IdentityServices.revoke#Sessions` with `allOthers` | PasswordChangeGate (framework) "a new password ends the account's other sessions"; delegation: to write: in-process |
| v5.0.0-7.4.4 | 2 | Sign-out visible on every signed-in page | Framework | the apps shells carry it (`webroot/screen/includes/navbar.html.fancy` logout, `QvtConfig.js` `logoutUrl`); a builder's own shell is its own | FW: to write: black-box |
| v5.0.0-7.4.5 | 2 | Administrators end one user's or everyone's sessions | Framework | `IdentityServices.revoke#Sessions` and `revoke#AccountAccess` (`ADMIN_IDENTITY_MANAGE` and a recent sign-in), System › Security › Sessions bulk action (200 a page, `tools/screen/System/Security/Sessions.xml`); everyone at once only by a new `session-config@cookie-key` (`known_gap_a_new_session_cookie_key_ends_every_session_cookie`) | AdminSessions (framework) "revoke#AccountAccess ends another user's sessions, keys, app passwords, SSH keys and triggers at once" |

## V7.5 Defenses Against Session Abuse

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.5.1 | 2 | Fresh sign-in before changing recovery or factor data | Gap | `require#RecentAuth` (5 min) guards the own email, provider link and factors (`UserServices.xml` `check#AccountAction` `recentAuth`, `service/recent_auth.rs`); open: SEC-ADM-81 (`PATCH /rest/s1/moqui/users/{userId}` writes an email past the guard), SEC-WEB-84 (OAuth 2 re-auth met by a cross-site GET) | PASS: to write: black-box |
| v5.0.0-7.5.2 | 2 | Users see and end their sessions after confirming | Application | `IdentityServices.find#Sessions` and `revoke#Sessions` act on the caller's own with no permission and ask no recent sign-in for them (Recent-Auth.md); the page is the app's (mr-boss Commons Account); tool: `require#RecentAuth` | - |

## V7.6 Federated Re-authentication

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-7.6.1 | 2 | RP and IdP session lifetimes behave as documented | Gap | RP logout sends the IdP's end-session URL (`moqui-host/src/sso.rs`); no IdP-initiated logout; no absolute lifetime, so the time since the IdP sign-in is not bounded; re-auth asks `prompt=login`, `max_age=0` and checks `auth_time` (`service/sso.rs:307-315`); SEC-WEB-84 open | - |
| v5.0.0-7.6.2 | 2 | No session made without the user's action | Gap | likely: `/sso/login` is a `read-only` GET (`webroot/screen/webroot/sso.xml` `login`) another site may start, and an IdP that answers without asking then signs the browser in; `/Login` needs the form | PASS: SsoHandoff (mr-pass) "SSO callback is browser-bound and exchanges a single-use site-scoped code" |
