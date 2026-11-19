# V6 Authentication

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V6 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V6.1 Authentication Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.1.1 | 1 | Brute-force defenses and lockout abuse documented | Framework | `Attack-Surface-Web.md` "Login lockout and passwords", "Web protection": `user-facade/login@max-failures`, `@disable-minutes`, the `web-protection` rules and how they turn on; attempts do not extend a lock window (lockout abuse only implied, not named) | doc review (no test) |
| v5.0.0-6.1.2 | 2 | Documented list of banned context words | Application | none: the framework keeps no word list | - |
| v5.0.0-6.1.3 | 2 | All sign-in pathways documented with their strength | Gap | pathways spread over the maps (Web, Admin, SSH, Git, Comms, Files, AI); no one list with the strength each must hold | doc review (no test) |

## V6.2 Password Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.2.1 | 1 | Minimum password length | Framework | `user/policy.rs` `password_acceptable` and `UserServices.xml` `update#PasswordInternal`, `user-facade/password@min-length` 8 | SecurityAuthn "A07 a short password is rejected over HTTP"; `policy.rs` `password_acceptable_rejects_secret_and_accepts_a_mixed_password` |
| v5.0.0-6.2.2 | 1 | Users can change their password | Framework | `UserServices.xml` `update#Password` from `Login/changePassword`, `ChangePassword`, `/rest/s1/moqui/users/{userId}/password/update` | to write: black-box |
| v5.0.0-6.2.3 | 1 | Change asks current and new password | Framework | `update#Password`: `oldPassword` required, `ec.user.checkOldPassword` (`login.rs` `check_old_password`); a code stands in only in a pre-authenticated session; the admin path needs `ADMIN_PASSWORD` and `require#RecentAuth` | `login_tests.rs` `the_old_password_of_a_change_is_behind_the_login_gate`; to write: black-box |
| v5.0.0-6.2.4 | 1 | New passwords screened against common passwords | Gap | none: `password_acceptable` and `update#PasswordInternal` check length, classes and history only | - |
| v5.0.0-6.2.5 | 1 | No character-class composition rules | Gap | `user-facade/password@min-digits` 1 and `@min-others` 1 by default (0 lifts them); a `<` in `newPassword` is refused by the parameter check (`allow-html` none, `service/validate.rs` `clean_text`; likely, not run) | - |
| v5.0.0-6.2.6 | 1 | Password fields masked | Framework | `webroot/Login.html.fancy` `type="password"`; the `<password/>` widget renders `type="password"` (html and qvt render goldens) | to write: black-box |
| v5.0.0-6.2.7 | 1 | Paste and password managers allowed | Framework | `autocomplete="current-password"` / `"new-password"` on the inputs; no paste blocking found | to write: black-box |
| v5.0.0-6.2.8 | 1 | Password checked exactly as entered | Framework | `login.rs` `password_login` takes the text as sent, no trim or case change; string parameters are not trimmed (`service/validate.rs` `convert_text` converts numbers and dates only); likely | to write: black-box |
| v5.0.0-6.2.9 | 2 | Passwords of 64+ characters accepted | Framework | no maximum in `password_acceptable`, `update#PasswordInternal` or the forms; argon2id hash (likely) | to write: black-box |
| v5.0.0-6.2.10 | 2 | No forced periodic password rotation | Configuration | `user-facade/password@change-weeks` 0; the default 104 expires a password (`login.rs` `password_expired`) | - |
| v5.0.0-6.2.11 | 2 | Context word list enforced on new passwords | Gap | no word-list check in `password_acceptable` or `update#PasswordInternal` | - |
| v5.0.0-6.2.12 | 2 | New passwords screened against breached passwords | Gap | none found: no breach list or lookup | - |

## V6.3 General Authentication Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.3.1 | 1 | Anti brute-force and stuffing controls | Configuration | account lock on by default (`login.rs` `gate_password`, `user-facade/login@max-failures` 3, `@disable-minutes` 5); the per-client `login` rate limit and `auth-failure` ban need `webapp-list/webapp[webroot]/web-protection@enabled` or `webapp-list/webapp[webroot]@trusted-proxies`; SEC-WEB-81, SEC-WEB-83 open | - |
| v5.0.0-6.3.2 | 1 | No default or well-known accounts usable | Framework | seed accounts have no password (`_NA_` disabled, `_EDGE_`, agent users); `john.doe` only in `demo` data, not in `tools@empty-db-load`; `user-facade/login@initial-admin-enabled` false, then loopback or setup secret (`user/setup.rs`) | SecurityAuthn "A07 create initial admin over HTTP fails when users exist"; `setup_tests.rs` `only_a_loopback_peer_with_no_proxy_header_is_local`; to write: black-box (`john.doe` refused) |
| v5.0.0-6.3.3 | 2 | Multi-factor sign-in required | Configuration | `moqui.security.UserGroup.requireAuthcFactor` Y, a data step (on `ALL_USERS` for everyone; `factor.rs` `second_factor_required`); off by default; keys, app passwords and SSO ask none | - |
| v5.0.0-6.3.4 | 2 | Every pathway documented, strength consistent | Gap | `login.rs` `internal_login` (login keys, app passwords, SSH keys, SSO) asks no second factor; `requireAuthcFactor` binds password sign-ins only; SSO strength unchecked (SEC-WEB-84) | - |

## V6.4 Authentication Factor Lifecycle and Recovery

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.4.1 | 1 | System-made initial secrets random, brief, one-time | Framework | `reset#Password`: `randomString(12)` (getrandom), argon2 in `resetPassword`, `user-facade/password@email-expire-hours` 48, taken only as `oldPassword`, cleared on change; setup secret about 99 bits, new each start (`user/setup.rs`) | `login_tests.rs` `the_old_password_of_a_change_is_behind_the_login_gate` (reset expiry); PASS: to write: black-box |
| v5.0.0-6.4.2 | 1 | No password hints or secret questions | Framework | no flow shows or asks one; `UserAccount.passwordHint` remains a column, kept private (`script/host/user.rs` `USER_ACCOUNT_PRIVATE`), set only in demo data | to write: black-box |
| v5.0.0-6.4.3 | 2 | Password reset keeps the second factor | Gap | factor kept: the reset password works only as `oldPassword` of `update#Password`, which then asks the code; SEC-ADM-82 open (a relay administrator receives reset mail) | - |
| v5.0.0-6.4.4 | 2 | Lost-factor recovery with identity proofing | Application | an administrator ends factors or issues codes (`invalidate#UserAuthcFactorEntry`, `create#SingleUseAuthcCodes` under `ADMIN_IDENTITY_MANAGE` and `require#RecentAuth`); no proofing step | - |

## V6.5 General Multi-factor authentication requirements

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.5.1 | 2 | One-time codes usable once | Framework | `validate#UserAuthcFactorCode`: a single-use row ends on use (`thruDate`); TOTP only for a step after `lastTotpStep` (SEC-WEB-12 fixed); no row lock on use (likely) | `totp.rs` `a_code_is_taken_for_a_step_after_the_last_one`; to write: black-box (single-use reuse) |
| v5.0.0-6.5.2 | 2 | Low-entropy lookup secrets stored slow-hashed, salted | Gap | `factor.rs` `single_use_hash`: SHA-256 with the fixed salt `SaltySalt` over 8-digit codes (then field-encrypted `factorOption`) | - |
| v5.0.0-6.5.3 | 2 | Codes and TOTP seeds from a CSPRNG | Framework | `factor.rs` `random_code`, `totp_secret` (20 bytes) from `getrandom` | to write: in-process |
| v5.0.0-6.5.4 | 2 | Codes carry at least 20 bits | Framework | `factor.rs` `random_code`: 8 digits printed (`create#SingleUseAuthcCodes`), 6 digits sent (`create#SingleUseAuthcFactor`) | `factor.rs` `random_codes_have_the_width_and_strings_are_url_safe` |
| v5.0.0-6.5.5 | 2 | Code lifetimes bounded | Gap | a sent code is good 24 hours (`create#SingleUseAuthcFactor` `thruDate`); TOTP 30-second step, one step either side (`totp.rs`) | - |

## V6.6 Out-of-Band authentication mechanisms

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.6.1 | 2 | SMS codes: verified numbers, alternatives, risk notice | Gap | an unvalidated `UafSms` factor counts and is sendable (`get#UserAuthcFactorInfo` ignores `needsValidation`); TOTP offered; no risk notice seen | - |
| v5.0.0-6.6.2 | 2 | Sent codes bound to their sign-in attempt | Gap | `validate#UserAuthcFactorCode` takes any in-date single-use row of the user, not the session that asked; a new send ends only that factor's earlier codes | - |
| v5.0.0-6.6.3 | 2 | Sent-code guessing rate limited | Framework | a wrong code counts as a failed login (`countFailure`, lock after 3); one send per 30 s per factor (`send#ExternalAuthcCode`); code checks are not bounded by `LoginCheck` (likely) | to write: black-box |

## V6.8 Authentication with an Identity Provider

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-6.8.1 | 2 | No identity spoofing across identity providers | Framework | `service/sso.rs` `linked_account` (`externalUserId` with `externalAuthFlowId`), `link_allowed` (verified email rule), `unscoped_link_allowed` | `sso_tests.rs` `a_provider_signs_in_only_the_accounts_linked_to_it_and_no_closed_one` |
| v5.0.0-6.8.2 | 2 | Assertion signatures always verified | Framework | OIDC needs a validated ID token (`moqui-host/src/sso.rs`; huskarl against `jwks_uri`, no `none`); OAuth 2 carries no assertion; no SAML (huskarl verifier not read) (map) | to write: in-process |
| v5.0.0-6.8.3 | 2 | SAML assertions single use | N/A | no SAML sign-in: `service/sso.rs` refuses `AftSaml` | - |
| v5.0.0-6.8.4 | 2 | IdP-reported strength and recency checked | Gap | `acr` and `amr` never read; `auth_time` only for an OIDC re-authentication (`service/sso.rs` `sign_in_time`); an OAuth 2 round trip counts as recent (SEC-WEB-84) | - |
