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

//! Which OS account each user's host sessions run as, where `terminal@run-as` is `person` (plans/Terminal-Fancy2.md
//! §4.6): a `moqui.terminal.TerminalOsAccount` row each, kept on System › Terminal Sessions by an administrator (the
//! System app's update) who also holds `TERMINAL_HOST`. The row only names the account: whether the server may start
//! programs as it is the root-owned `/etc/moqui/run-as.conf`'s to say ([`super::runas`]).

use std::collections::BTreeMap;

use moqui_core::entity::EntityFacade;
use moqui_lib::object::{to_plain_string, Value};

use super::access::HOST_PERMISSION;
use super::services::{answer, refuse, text, Out};

const ENTITY: &str = "moqui.terminal.TerminalOsAccount";

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, find_terminal_os_accounts)?;
    moqui_core::rust_service!(map, store_terminal_os_account)?;
    moqui_core::rust_service!(map, expire_terminal_os_account)?;
    Ok(())
}

/// An administrator who may keep the accounts: the System app's `action`, and `TERMINAL_HOST`.
fn may_keep(action: &str) -> bool {
    moqui_core::user::authorize_own("AT_XML_SCREEN", "component://tools/screen/System.xml", action).is_ok()
        && moqui_core::user::has_permission(HOST_PERMISSION).unwrap_or(false)
}

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// `find#TerminalOsAccounts`: the users' accounts in force.
pub fn find_terminal_os_accounts(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_keep("view") {
        return refuse("notAllowed", "Not allowed");
    }
    let rows = facade
        .find(ENTITY)?
        .eq("thruDate", Value::Null)
        .order_by("username", false)
        .disable_authz()
        .list()?;
    let listed: Vec<serde_json::Value> = rows
        .values()
        .iter()
        .map(|row| {
            let text = |key: &str| row.get(key).map(to_plain_string).unwrap_or_default();
            serde_json::json!({"username": text("username"), "osUsername": text("osUsername"), "fromDate": text("fromDate")})
        })
        .collect();
    answer("accounts", &serde_json::Value::Array(listed))
}

/// End the row in force of `username`, if any.
fn expire(facade: &EntityFacade, username: &str) -> Result<(), moqui_core::context::Error> {
    let rows = facade
        .find(ENTITY)?
        .eq("username", Value::string(username))
        .eq("thruDate", Value::Null)
        .disable_authz()
        .list()?;
    for mut row in rows.values().iter().cloned() {
        row.set("thruDate", moqui_core::user::now_value());
        row.update(facade)?;
    }
    Ok(())
}

/// `store#TerminalOsAccount`: `username`'s host sessions run as `osUsername` from now on.
pub fn store_terminal_os_account(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_keep("update") {
        return refuse("notAllowed", "Not allowed");
    }
    let (username, os_username) = (text(parameters, "username"), text(parameters, "osUsername"));
    if username.is_empty() || !name_ok(&os_username) || os_username == "root" {
        return refuse(
            "badAccount",
            "Name a user and an OS account (letters, digits, . _ -; never root)",
        );
    }
    // the administrator is checked above; the rows are the terminal's own
    let _authz = moqui_core::context::ExecutionContext::with_current(|ec| ec.disable_authz());
    expire(facade, &username)?;
    let mut row = facade.make_value(ENTITY)?;
    row.set("username", Value::string(&username));
    row.set("fromDate", moqui_core::user::now_value());
    row.set("osUsername", Value::string(&os_username));
    row.create(facade)?;
    tracing::info!("terminal: {username}'s host sessions run as the OS account {os_username} from now on");
    Ok(BTreeMap::new())
}

/// `expire#TerminalOsAccount`: `username` has no OS account from now on, so no host session where run-as is person.
pub fn expire_terminal_os_account(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_keep("update") {
        return refuse("notAllowed", "Not allowed");
    }
    let username = text(parameters, "username");
    let _authz = moqui_core::context::ExecutionContext::with_current(|ec| ec.disable_authz());
    expire(facade, &username)?;
    tracing::info!("terminal: {username} has no OS account from now on");
    Ok(BTreeMap::new())
}
