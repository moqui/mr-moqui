# Recent sign-in

Some actions need the user to have signed in a moment ago, not just to hold a signed-in session: resetting their chat
identity, starting or deleting a key backup (or its keys, or giving it another public key), removing a device that
holds encryption keys, issuing single use codes, adding or ending a second factor, making an SSH key, an access token
or an app password, and changing the account's own email address (where a password reset is mailed) or its provider
link (`externalUserId`). An administrator's actions on another account need it too: setting its password, issuing its
codes, adding or ending its factors, changing its email address or provider link, revoking its keys and Login as. A
stolen session (a cookie taken from a browser left signed in) then cannot do them alone. The check is reusable for any
sensitive action.

## What counts as a sign-in

The session keeps the time of its user's last sign-in that proved who they are (`moquiLastAuth`, with the user id):

- a password login: the login form, an SSH or terminal password, Basic credentials;
- the code that finishes a second factor;
- an SSO login (`/sso/callback`) at the time the provider says the user signed in: an OpenID Connect ID token's
  `auth_time` (none without one, so a login the provider answered from an old session of its own is not recent); an
  OAuth provider proves no time, so only a re-authentication the user asked for counts, taken at its word;
- a re-authentication (`reauthenticate#User`).

A login key, an API key, an app password and an internal login (jobs, `internalLoginUser`, an administrator's "login as")
never count. A logout drops it, and it belongs to its user: another user's login in the same session drops it, so the
first user coming back by a key or an internal login has none. A login for one piece of work (a service called with
credentials) puts the session's own back.

The code is `crates/moqui-core/src/user/login.rs` (`recent_auth`, `last_auth_millis`, `check_reauth_password`) and
`user/factor.rs` (`reauthenticate`).

## Services

`org.moqui.sso.RecentAuthServices` (`crates/moqui-core/src/service/recent_auth.rs`):

| Service | What |
| --- | --- |
| `check#RecentAuth` | `recent` within `maxAgeSeconds` (300 by default), `authAgeSeconds`, and how the user confirms who they are: `method` `password` (with `secondFactor`) or `sso` (an account without a password: `authFlowId`, `authFlowList`). With `required`, not recent is an error |
| `reauthenticate#User` | The signed-in user's password, then the code of their second factor. A wrong password or code counts as a failed login. A checked password waits five minutes for its code, which may then come alone. The login, the session and its id do not change |
| `send#ReauthCode` | A code to the user's own email or SMS factor, once the password was checked; one each half minute in a session |
| `require#RecentAuth` | Refuses unless the user signed in within the last five minutes: the error "Confirm it is you: sign in again to do this", `reasonCode` `reauthRequired`, and the response header `X-Moqui-Reauth: required`. A caller with authorization disabled (a job, a setup service) passes. It guards the caller's own credentials and factors, and an administrator's actions on another account |

A service that guards an action calls `check#RecentAuth` or `require#RecentAuth` and refuses when it is not recent.
The ChatKeys services answer `reasonCode` `reauthRequired` ([Chat keys](../interfaces/Chat-Keys.md)). The account
services go through `org.moqui.impl.UserServices.check#AccountAction` with `recentAuth`: the factor and code services
on the user's own account, and `update#UserAccount` when it changes the user's own `emailAddress` or `externalUserId`
(a cleared value counts); `user_keys.rs` calls `require_recent_auth` for `create#UserSshKey`, `create#UserAccessToken`
and `create#UserAppPassword`.

An administrator's actions on another account are guarded too, after the permission and the check of the access that
account holds (`heldBeyond`): setting its password (`update#Password` with `ADMIN_PASSWORD`), issuing its codes, adding
or ending its factors, changing its email address or provider link (`update#UserAccount`), revoking its keys, tokens,
app passwords, SSH keys, phones and sessions (`revoke#AccountAccess`, every kind; `revoke#Sessions` for another
account's sessions), and Login as (Tools `UserAccountDetail.xml` and mr-boss Admin's `UserDetail.xml` `loginAs`, which
log in only when it is recent). Ending the user's own other sessions is not guarded. `reset#Password` is not either:
anyone may ask it, and it only mails the account's own address. A caller with authorization disabled (a job, a setup
service, a lifecycle's scheduled step) passes. Mantle's account lifecycle asks it before it starts a disable now or a
removal now that ends the account's memberships, and before a retry of a step that revokes access or ends sessions, so
such an operation is refused whole (`REAUTH_REQUIRED`) rather than left half done; a People separation is never
refused: without a recent sign-in its account steps wait for an identity operator.

## In the browser

`/ReAuth/check`, `/ReAuth/password`, `/ReAuth/sendCode` and `/ReAuth/sendEmailCode` (webroot `ReAuth.xml`) serve `js/WebrootReAuth.js`
(`moqui.reauth`, loaded for a signed-in user):

- `ensure()` checks first and asks only when needed;
- `retry(fn)` runs `fn` and, on `reauthRequired`, asks and runs it once more;
- `confirm()` asks: a password dialog, then the code (offering to send it to an email or phone factor), in Quasar dialogs.

A form (`m-form`, `WebrootVue.qvt.js` `submitGo`) whose answer carries `X-Moqui-Reauth` calls `moqui.reauth.confirm()`
and sends the form once more; a cancel shows the refusal.

An SSO-only account (no password) confirms one of two ways:

- **A code to its verified address.** When the account's `emailAddress` has a `VerifiedAddress` row (the provider verified
  it under a verified-email link rule, or the registration's mail link did), `check#RecentAuth` answers `emailCode`, and
  `confirm()` offers it: `/ReAuth/sendEmailCode` (`send#ReauthEmailCode`, one a half minute in a session) mails a single-use
  code, and `/ReAuth/password` takes it as `{code}` alone (`reauthenticate#User`). A wrong code counts as a failed login.
  This does not depend on the provider.
- **A fresh sign-in at its provider.** A POST of `/sso/reauth` with `authFlowId`, a same-origin `returnTo` and the session
  token (SEC-WEB-84); `/sso/login` is always a plain sign-in, so a link or another site cannot start a re-authentication.
  An OpenID provider is asked to sign the user in again (`prompt=login&max_age=0`) and returns to the page, where the user
  does the action again. The callback counts it only when the ID token's `auth_time` is after the request (a minute of
  clock skew allowed), so a provider that ignores the request gives no recent sign-in. An OAuth 2 provider has no
  `auth_time`, so its round trip counts only when the flow's `OauthFlow.reauthPrompts` is Y: set it (Admin's provider
  detail, "Confirms a sign-in", which a holder of `ADMIN_CONNECTION_MANAGE` changes after a recent sign-in) only for a
  provider that asks the user to sign in on every authorization. `check#RecentAuth` lists only the flows that can answer it
  (OpenID Connect, and OAuth with `reauthPrompts` Y).

In the terminal, BasicComms Settings, My devices has a "Confirm it is you" form (password and code).

## SSO sign-in hook

The conf's `user-facade` `login` `sso-linked-service` names a service told of each real sign-in through an OAuth or
OpenID Connect flow (`handle#Callback`) that links or confirms the account's provider id (`UserAccount.externalUserId`),
so an application can tie its own records to the identity (Smithy's GitHub authors, claimed by signing in with GitHub).
It is called once the sign-in's transaction commits, with authorization disabled, with:

| Parameter | |
|---|---|
| `userId`, `authFlowId` | who signed in, through which flow |
| `providerHost` | the host of the user info endpoint the profile was read from (the flow's own, or its client type's: `api.github.com`), else of the ID token's issuer |
| `externalUserId` | the provider's id |
| `login` | the provider's username (`login`, `preferred_username` or `username`), when the profile has one |
| `email`, `emailVerified` | the profile's email, and whether the provider's `email_verified` claim is true |
| `urlsOverridden` | the flow names an endpoint (OAuth) or a discovery or base URL (OpenID) of its own instead of its client type's |

A failure is logged and never blocks or undoes the sign-in. A write of `externalUserId` by hand (an administrator's
link) is no sign-in and calls nothing. Test: `cargo test -p moqui-core service::sso`.

## Tests

- `cargo test -p moqui-core --lib user::login`: which logins count, an account switch, a local login, and a
  re-authentication's password check; `service::sso`: the `auth_time` rules.
- `cargo test -p moqui-host --test it -- sso::`: against a test OpenID provider, a fresh sign-in is recent and an old
  `auth_time` is not; a re-authentication sends `prompt=login` and `max_age=0` and counts only with a new `auth_time`.
- `./moq test framework --spec RecentAuth,ChatKeys,ChatKeyBackup`: each guarded action refused with the same parameters
  after an internal login or an account switch and allowed after `reauthenticate#User` or a password login; the second
  factor's code, a wrong one counted; a code sent only after the password, not twice in half a minute; the age.
- `./moq test framework --spec CredentialGuards,AdminSessions`: a stale administrator refused another account's codes,
  password, email and credential revoke, each allowed after `reauthenticate#User`; the owner's own features unchanged.

**Not checked live:** a real SSO provider's `prompt=login`, `max_age` and `auth_time`.
