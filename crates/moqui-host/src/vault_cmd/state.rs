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

//! What `moqui vault` remembers between runs (plans/Vault-Instances.md §9, decision (d)): for each server and vault,
//! the highest key version and member-list revision its client accepted and the hash of that member list. The next
//! open hands it to the vault module, which refuses a chain that does not hold that entry with that member list (a
//! rollback, or a fork however high it goes). Kept in `~/.config/moqui/vault/state.json` (or under
//! `XDG_CONFIG_HOME`), readable by its owner only. It holds no key and no secret.

use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde_json::{json, Value};

/// The state file.
pub(crate) fn state_file() -> Result<PathBuf, Error> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| Error::new("no HOME to keep the vault state in"))?;
    Ok(config.join("moqui/vault/state.json"))
}

/// The remembered state of each server's vaults.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct State {
    servers: serde_json::Map<String, Value>,
}

impl State {
    /// The file's state; none yet, or a file that does not read, is an empty state (logged), so a lost file costs only
    /// the memory, never the vault.
    pub(crate) fn read(path: &Path) -> State {
        let Ok(text) = std::fs::read_to_string(path) else {
            return State::default();
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(servers)) => State { servers },
            _ => {
                tracing::error!("vault state {} does not read; starting with none", path.display());
                State::default()
            }
        }
    }

    /// What is remembered of `vault_id` on `server`, in the vault module's shape (`remembered`), or null.
    pub(crate) fn remembered(&self, server: &str, vault_id: &str) -> Value {
        self.servers
            .get(server)
            .and_then(|vaults| vaults.get(vault_id))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Keeps what the module answered to remember: never lower than what is kept (the module refused a lower one).
    pub(crate) fn remember(&mut self, server: &str, vault_id: &str, remembered: &Value) {
        let key = |r: &Value| (r["version"].as_u64().unwrap_or(0), r["revision"].as_u64().unwrap_or(0));
        let kept = self.remembered(server, vault_id);
        if !kept.is_null() && key(&kept) > key(remembered) {
            return;
        }
        let vaults = self.servers.entry(server.to_string()).or_insert_with(|| json!({}));
        if let Some(vaults) = vaults.as_object_mut() {
            vaults.insert(vault_id.to_string(), remembered.clone());
        }
    }

    /// Writes the state, readable by its owner only.
    pub(crate) fn write(&self, path: &Path) -> Result<(), Error> {
        let text = serde_json::to_string_pretty(&Value::Object(self.servers.clone()))
            .map_err(|err| Error::new(format!("vault state: {err}")))?;
        let written = path.parent().map_or(Ok(()), make_dir).and_then(|()| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            // a file made before, or by another tool, is its owner's alone too
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            std::io::Write::write_all(&mut file, text.as_bytes())
        });
        written.map_err(|err| Error::new(format!("vault state {}: {err}", path.display())))
    }
}

/// The state's directory, its owner's alone (0700) when this makes it.
fn make_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
