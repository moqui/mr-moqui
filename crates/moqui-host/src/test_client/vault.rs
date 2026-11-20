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

//! The client specs' `vault` member (plans/Vault-Instances.md V1): vault clients of a spec, each the vault crate's own
//! session natively (`crate::vault_cmd::client`, which `moqui vault` drives too), so a spec makes a personal key,
//! checks a fingerprint, signs a member list and a wrap, opens a vault through its chain and an item, the way a real
//! client does and with its refusals. The clients end with their spec.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use moqui_lib::sync::lock;
use serde_json::{json, Value};

use crate::vault_cmd::client::VaultClient;

/// A spec's vault clients, by id.
#[derive(Default)]
pub(super) struct Clients {
    next: AtomicU64,
    held: Mutex<HashMap<u64, VaultClient>>,
}

impl Clients {
    /// `vault.new`, `vault.call` (`{id, request}`, answered as the module answers) and `vault.close`.
    pub(super) fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "vault.new" => {
                let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
                lock(&self.held).insert(id, VaultClient::default());
                Ok(json!(id))
            }
            "vault.call" => {
                let id = args["id"].as_u64().unwrap_or(0);
                let mut held = lock(&self.held);
                let client = held
                    .get_mut(&id)
                    .ok_or_else(|| format!("no vault client {id}: it ended with its spec or was closed"))?;
                Ok(client.answer(&args["request"]))
            }
            "vault.close" => {
                lock(&self.held).remove(&args["id"].as_u64().unwrap_or(0));
                Ok(Value::Null)
            }
            other => Err(format!("{other} is not a call of the client host")),
        }
    }

    /// Ends every client of the spec; their keys are dropped, which clears them.
    pub(super) fn clear(&self) {
        lock(&self.held).clear();
    }
}
