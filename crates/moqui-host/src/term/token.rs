/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! A session's token (plans/Terminal-Fancy2.md §4.3, D14): `MOQUI_TERMINAL_TOKEN` in a command or host session's
//! environment, beside `MOQUI_TERMINAL_SESSION`, so `moqui term` run inside it reads and drives its user's sessions. It
//! is an HMAC of the session's id under `terminal@token-key` (generated into `runtime/.env`), so a held session's token
//! still holds after the server restarts, with no row; it is good only while that session runs, only from this
//! machine, only for the four driving services, and only on the sessions its session's kind reaches.

use super::session::{Kind, Session};

/// The headers a program sends its session's id and token in.
pub(crate) const SESSION_HEADER: &str = "x-moqui-terminal-session";
pub(crate) const TOKEN_HEADER: &str = "x-moqui-terminal-token";

/// The conf's token key, when it is set and not the placeholder the conf ships. Read with no context too.
fn conf_key() -> Option<String> {
    Some(moqui_core::context::conf_env::setting("terminal@token-key"))
        .filter(|key| !key.is_empty() && !key.ends_with("CHANGEME"))
}

/// The token of session `id` under `key`.
pub(crate) fn token_with(key: &str, id: &str) -> String {
    use base64::Engine as _;
    use hmac::{KeyInit, Mac};
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).unwrap_or_else(|_| unreachable!());
    mac.update(b"moqui terminal session ");
    mac.update(id.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// The token of session `id`, for its environment; `None` while the conf has no key.
pub(crate) fn token(id: &str) -> Option<String> {
    conf_key().map(|key| token_with(&key, id))
}

/// Whether `token` is session `id`'s under `key`.
pub(crate) fn holds_with(key: &str, id: &str, token: &str) -> bool {
    !id.is_empty()
        && !token.is_empty()
        && moqui_lib::codec::constant_time_eq(token_with(key, id).as_bytes(), token.as_bytes())
}

/// Whether `token` is session `id`'s.
pub(crate) fn holds(id: &str, token: &str) -> bool {
    conf_key().is_some_and(|key| holds_with(&key, id, token))
}

/// Whether a program in session `from` may read and drive session `to` (D14): one of the same user, of a kind it
/// reaches. A command line or a shell reaches the user's command lines and shells; nothing else is reached.
pub(crate) fn reaches(from: &Session, to: &Session) -> bool {
    from.username == to.username
        && matches!(from.kind, Kind::Command | Kind::Host)
        && matches!(to.kind, Kind::Command | Kind::Host)
}

#[cfg(test)]
mod tests {
    use super::{holds_with, token_with};

    #[test]
    fn a_token_is_its_sessions_alone_under_its_key() {
        let token = token_with("key one", "s1");
        assert!(holds_with("key one", "s1", &token));
        assert!(!holds_with("key one", "s2", &token), "another session's id");
        assert!(!holds_with("key two", "s1", &token), "another key");
        assert!(!holds_with("key one", "s1", ""));
        assert!(!holds_with("key one", "", &token));
        assert_eq!(token, token_with("key one", "s1"), "the same after a restart");
    }
}
