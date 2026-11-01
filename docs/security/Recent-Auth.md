# Recent sign-in

Some actions need the user to have signed in a moment ago, not just to hold a signed-in session: resetting their chat
identity, starting or deleting a key backup, and removing a device that holds encryption keys. A stolen session (a cookie
taken from a browser left signed in) then cannot do them alone. The check is reusable for any sensitive action.

## What counts as a sign-in

The session keeps the time of its user's last sign-in that proved who they are (`moquiLastAuth`, with the user id):

- a password login: the login form, an SSH or terminal password, Basic credentials;
- the code that finishes a second factor;
- an SSO login (`/sso/callback`);
- a re-authentication (`reauthenticate#User`).

A login key, an API key, an app password and an internal login (jobs, `internalLoginUser`) never count. A logout drops it,
and it belongs to its user: another user signing in to the same session does not take it over.

The code is `crates/moqui-core/src/user/login.rs` (`recent_auth`, `last_auth_millis`, `check_reauth_password`) and
`user/factor.rs` (`reauthenticate`).

## Services

`org.moqui.impl.RecentAuthServices` (`crates/moqui-core/src/service/recent_auth.rs`):

| Service | What |
| --- | --- |
| `check#RecentAuth` | `recent` within `maxAgeSeconds` (300 by default), `authAgeSeconds`, and how the user confirms who they are: `method` `password` (with `secondFactor`) or `sso` (an account without a password: `authFlowId`, `authFlowList`). With `required`, not recent is an error |
| `reauthenticate#User` | The signed-in user's password, then the code of their second factor. A wrong password or code counts as a failed login. A checked password waits five minutes for its code, which may then come alone. The login, the session and its id do not change |
| `send#ReauthCode` | A code to an email or SMS factor, once the password was checked |

A service that guards an action calls `check#RecentAuth` and refuses when it is not recent. The ChatKeys services answer
`reasonCode` `reauthRequired` ([Chat keys](../interfaces/Chat-Keys.md)).

## In the browser

`/ReAuth/check`, `/ReAuth/password` and `/ReAuth/sendCode` (webroot `ReAuth.xml`) serve `js/WebrootReAuth.js`
(`moqui.reauth`, loaded for a signed-in user):

- `ensure()` checks first and asks only when needed;
- `retry(fn)` runs `fn` and, on `reauthRequired`, asks and runs it once more;
- `confirm()` asks: a password dialog, then the code (offering to send it to an email or phone factor), in Quasar dialogs.

An SSO-only account goes to `/sso/login?authFlowId=…&reauth=true&returnTo=<this page>`. An OpenID provider is asked to sign
the user in again (`prompt=login`) and returns to the page, where the user does the action again.

In the terminal, BasicComms Settings, My devices has a "Confirm it is you" form (password and code).

## Tests

- `cargo test -p moqui-core --lib user::login`: which logins count, and a re-authentication's password check.
- `./moq test framework --spec ChatKeys,ChatKeyBackup`: each guarded action refused with the same parameters after an
  internal login and allowed after `reauthenticate#User` or a password login; the second factor's code; the age.

**Not checked live:** an SSO provider's `prompt=login` (a provider that ignores it answers from its own session).
