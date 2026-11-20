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

//! The client specs' `vault` member in a binary built without the vault feature (test_client/vault.rs is the member).

use serde_json::Value;

#[derive(Default)]
pub(super) struct Clients;

impl Clients {
    pub(super) fn call(&self, name: &str, _args: &Value) -> Result<Value, String> {
        Err(format!("{name}: vault is not built into this host"))
    }

    pub(super) fn clear(&self) {}
}
