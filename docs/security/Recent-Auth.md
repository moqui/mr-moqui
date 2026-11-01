# Recent sign-in

Some actions need the user to have signed in a moment ago, not just to hold a signed-in session: resetting their chat
identity, starting or deleting a key backup (or its keys, or giving it another public key), and removing a device that
holds encryption keys. A stolen session (a cookie
taken from a browser left signed in) then cannot do them alone. The check is reusable for any sensitive action.

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

`org.moqui.impl.RecentAuthServices` (`crates/moqui-core/src/service/recent_auth.rs`):

| Service | What |
| --- | --- |
| `check#RecentAuth` | `recent` within `maxAgeSeconds` (300 by default), `authAgeSeconds`, and how the user confirms who they are: `method` `password` (with `secondFactor`) or `sso` (an account without a password: `authFlowId`, `authFlowList`). With `required`, not recent is an error |
| `reauthenticate#User` | The signed-in user's password, then the code of their second factor. A wrong password or code counts as a failed login. A checked password waits five minutes for its code, which may then come alone. The login, the session and its id do not change |
| `send#ReauthCode` | A code to the user's own email or SMS factor, once the password was checked; one each half minute in a session |

A service that guards an action calls `check#RecentAuth` and refuses when it is not recent. The ChatKeys services answer
`reasonCode` `reauthRequired` ([Chat keys](../interfaces/Chat-Keys.md)).

## In the browser

`/ReAuth/check`, `/ReAuth/password` and `/ReAuth/sendCode` (webroot `ReAuth.xml`) serve `js/WebrootReAuth.js`
(`moqui.reauth`, loaded for a signed-in user):

- `ensure()` checks first and asks only when needed;
- `retry(fn)` runs `fn` and, on `reauthRequired`, asks and runs it once more;
- `confirm()` asks: a password dialog, then the code (offering to send it to an email or phone factor), in Quasar dialogs.

An SSO-only account goes to `/sso/login?authFlowId=…&reauth=true&returnTo=<this page>` (a same-origin `returnTo` only). An
OpenID provider is asked to sign the user in again (`prompt=login&max_age=0`) and returns to the page, where the user does
the action again. The callback counts it only when the ID token's `auth_time` is after the request (a minute of clock skew
allowed), so a provider that ignores the request gives no recent sign-in.

In the terminal, BasicComms Settings, My devices has a "Confirm it is you" form (password and code).

## Tests

- `cargo test -p moqui-core --lib user::login`: which logins count, an account switch, a local login, and a
  re-authentication's password check; `service::sso`: the `auth_time` rules.
- `cargo test -p moqui-host --test it -- sso::`: against a test OpenID provider, a fresh sign-in is recent and an old
  `auth_time` is not; a re-authentication sends `prompt=login` and `max_age=0` and counts only with a new `auth_time`.
- `./moq test framework --spec RecentAuth,ChatKeys,ChatKeyBackup`: each guarded action refused with the same parameters
  after an internal login or an account switch and allowed after `reauthenticate#User` or a password login; the second
  factor's code, a wrong one counted; a code sent only after the password, not twice in half a minute; the age.

**Not checked live:** a real SSO provider's `prompt=login`, `max_age` and `auth_time`.
