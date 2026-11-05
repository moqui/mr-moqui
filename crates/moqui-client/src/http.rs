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

//! The HTTP client every request of the crate goes through, and the form encoding.

use std::time::Duration;

use moqui_lib::error::Error;

/// Seconds a sign-in or config request may take before the client says the server did not answer.
const TIMEOUT_SECONDS: u64 = 60;

/// Seconds to connect: a server that drops the packets is offline soon, so the shell opens on its cache.
pub(crate) const CONNECT_SECONDS: u64 = 10;

/// A client with no redirects (the proxy hands them to the page), no cookie store (the key is the session) and no
/// `Origin` (the server's origin checks pass a request without one).
pub(crate) fn client() -> Result<reqwest::blocking::Client, Error> {
    client_with(Duration::from_secs(TIMEOUT_SECONDS))
}

/// [`client`] with another timeout for the whole request, its body read included.
pub(crate) fn client_with(timeout: Duration) -> Result<reqwest::blocking::Client, Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(CONNECT_SECONDS))
        .user_agent(concat!("moqui-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| Error::new(format!("HTTP client: {err}")))
}

pub(crate) use moqui_lib::remote_login::{form_body, reason};
