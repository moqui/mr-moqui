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

//! `ssh` of a client spec in a build without the `ssh` feature: every call says so (`ssh.rs` is the client).

use std::time::Duration;

use serde_json::Value;

use super::http::Server;

/// What the SSH calls need of the client host.
#[allow(dead_code)]
pub(crate) struct Env<'a> {
    pub(crate) server: &'a Server,
    pub(crate) insecure: bool,
    pub(crate) timeout: Duration,
    pub(crate) cap: &'a dyn Fn(Duration) -> Duration,
    pub(crate) allow: &'a dyn Fn(&str, u16) -> Result<(), String>,
    pub(crate) id: &'a dyn Fn() -> u64,
}

pub(crate) struct Ssh;

impl Ssh {
    pub(crate) fn new() -> Self {
        Ssh
    }

    pub(crate) fn call(&self, name: &str, _args: &Value, _env: &Env<'_>) -> Result<Value, String> {
        Err(format!("{name}: this build has no SSH client (the ssh feature)"))
    }

    pub(crate) fn close(&self, _ids: &[u64]) {}

    pub(crate) fn close_all(&self) {}
}
