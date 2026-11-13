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

//! The record of every write (plans/design-dev-aide.md 15): a `moqui.dev.DevAideAudit` row for each save, create, move,
//! delete and git command that writes, with the user, the root and path, the hashes before and after, and a detail.

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

pub const AUDIT: &str = "moqui.dev.DevAideAudit";

/// One audited write.
#[derive(Debug, Default)]
pub struct Write<'a> {
    pub action: &'a str,
    pub root: &'a str,
    pub path: &'a str,
    pub hash_before: Option<&'a str>,
    pub hash_after: Option<&'a str>,
    pub detail: Option<serde_json::Value>,
}

/// Write the audit row of `write`, as the current user. A failure is logged and the write it records stands.
pub fn record(facade: &EntityFacade, write: &Write<'_>) {
    if let Err(err) = try_record(facade, write) {
        tracing::error!(
            "Could not write the Dev AIDE audit row of {} {} {}: {err}",
            write.action,
            write.root,
            write.path
        );
    }
}

fn try_record(facade: &EntityFacade, write: &Write<'_>) -> Result<(), Error> {
    let mut row = facade.make_value(AUDIT)?;
    row.set_sequenced_id_primary(facade)?;
    let user = moqui_core::user::current_user_id()?.unwrap_or_default();
    row.set("userId", Value::string(user));
    row.set("auditDate", moqui_core::user::now_value());
    row.set("actionEnumId", Value::string(write.action));
    row.set("rootId", Value::string(write.root));
    row.set("filePath", Value::string(write.path));
    if let Some(hash) = write.hash_before {
        row.set("hashBefore", Value::string(hash));
    }
    if let Some(hash) = write.hash_after {
        row.set("hashAfter", Value::string(hash));
    }
    if let Some(detail) = &write.detail {
        row.set("detail", Value::string(detail.to_string()));
    }
    row.create(facade)?;
    Ok(())
}
