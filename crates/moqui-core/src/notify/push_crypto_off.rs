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

//! The stand-in for `push_crypto.rs` in a build without the `web-push` feature: the same API, and every operation
//! reports that Web Push is not built into this host.

/// Web Push is not built into this host.
pub(crate) const BUILT: bool = false;
const NOT_BUILT: &str = "Web Push is not built into this host (the web-push feature)";

pub(crate) struct Vapid;

impl Vapid {
    pub(crate) fn from_seed(_seed: &str) -> Result<Self, String> {
        Err(NOT_BUILT.to_string())
    }

    pub(crate) fn public_key(&self) -> &str {
        ""
    }

    pub(crate) fn authorization(&self, _endpoint: &str, _subject: &str, _now_secs: u64) -> Result<String, String> {
        Err(NOT_BUILT.to_string())
    }
}

pub(crate) fn origin(_endpoint: &str) -> Result<String, String> {
    Err(NOT_BUILT.to_string())
}

pub(crate) fn check_keys(_p256dh: &str, _auth: &str) -> Result<(), String> {
    Err(NOT_BUILT.to_string())
}

pub(crate) fn encrypt(_payload: &[u8], _p256dh: &str, _auth: &str) -> Result<Vec<u8>, String> {
    Err(NOT_BUILT.to_string())
}
