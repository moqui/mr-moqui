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

//! A vault client's crypto: the vault crate's own `ops::Session` (crates/moqui-vault), natively, which the browser's
//! vault worker runs as wasm. It holds the opened personal key and vault keys and decides what it trusts by itself
//! (keys its own key checked, the member-list chain, what the client remembers); `moqui vault` and the test client's
//! `vault` member both drive it, so neither re-implements a client check.

use moqui_vault::ops::Session;
use moqui_vault::rng::OsRng;
use serde_json::{json, Value};

/// One client's session.
#[derive(Default)]
pub(crate) struct VaultClient {
    session: Session,
}

impl VaultClient {
    /// The module's answer to `request` (`{"op": ..., ...}`): `{"ok": true, "result": ...}` or `{"ok": false,
    /// "error": ...}`.
    pub(crate) fn answer(&mut self, request: &Value) -> Value {
        let text = self.session.call(&request.to_string(), &mut OsRng);
        serde_json::from_str(&text)
            .unwrap_or_else(|_| json!({ "ok": false, "error": "the module's answer is not JSON" }))
    }

    /// `op` with `args`: its result, or the module's refusal.
    pub(crate) fn call(&mut self, op: &str, mut args: Value) -> Result<Value, String> {
        if !args.is_object() {
            args = json!({});
        }
        args["op"] = json!(op);
        let answer = self.answer(&args);
        if answer["ok"] == json!(true) {
            Ok(answer["result"].clone())
        } else {
            Err(format!("vault {op}: {}", answer["error"].as_str().unwrap_or("refused")))
        }
    }
}
