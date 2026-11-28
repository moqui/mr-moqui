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

//! Sharing a session (plans/Terminal-Fancy2.md §4.4, D3, D15): its owner lets named users watch a command line or a
//! shell, and lets one type only when that user holds the kind's own permission (`SSH_SHELL`, `TERMINAL_HOST`). A
//! share is a `moqui.terminal.TerminalSessionShare` row and is kept on the running session; revoked, its user's windows
//! are let go; ended, the session's rows are closed; a held session taken back after a restart reads its open rows
//! again. A user with a share passes the same rules for the place as the owner (the socket's and the services' checks).

use std::collections::BTreeMap;
use std::sync::Weak;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::{to_plain_string, Value};

use super::access::{COMMAND_PERMISSION, HOST_PERMISSION};
use super::registry::Registry;
use super::services::{answer, factory, refuse, text, username, Out};
use super::session::{Kind, Session};

const ENTITY: &str = "moqui.terminal.TerminalSessionShare";

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, share_terminal_session)?;
    moqui_core::rust_service!(map, revoke_terminal_session_share)?;
    Ok(())
}

/// The account id of `username`, read with authorization off.
fn user_id_of(facade: &EntityFacade, username: &str) -> Option<String> {
    facade
        .find("moqui.security.UserAccount")
        .ok()?
        .eq("username", Value::string(username))
        .disable_authz()
        .one()
        .ok()??
        .get("userId")
        .map(to_plain_string)
}

/// Close the open rows of session `id`, of `username` alone when given.
fn close_rows(facade: &EntityFacade, id: &str, username: Option<&str>) -> Result<(), moqui_core::context::Error> {
    let mut find = facade
        .find(ENTITY)?
        .eq("terminalSessionId", Value::string(id))
        .eq("thruDate", Value::Null)
        .disable_authz();
    if let Some(username) = username {
        find = find.eq("username", Value::string(username));
    }
    for mut row in find.list()?.values().iter().cloned() {
        row.set("thruDate", moqui_core::user::now_value());
        row.update(facade)?;
    }
    Ok(())
}

/// The owner's own command line or shell `sessionId`, or the refusal.
fn owned(parameters: &BTreeMap<String, Value>) -> Result<(std::sync::Arc<Session>, String), Out> {
    let owner = username()?;
    let factory = factory().map_err(Err)?;
    super::services::adopt_held(&factory);
    match Registry::of(&factory).find(&text(parameters, "sessionId"), &owner) {
        Some(session) if session.kind != Kind::Apps => Ok((session, owner)),
        Some(_) => Err(refuse("notAvailable", "An apps session cannot be shared")),
        None => Err(refuse("notFound", "No such terminal session")),
    }
}

/// `share#TerminalSession`: let `username` watch the session, and type in it with `write` when they hold the kind's
/// permission. Sharing again changes the share.
pub fn share_terminal_session(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let (session, owner) = match owned(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let with = text(parameters, "username");
    if with.is_empty() || with == owner {
        return refuse("badUser", "Share a session with another user");
    }
    let Some(user_id) = user_id_of(facade, &with) else {
        return refuse("badUser", "No such user");
    };
    let write = matches!(text(parameters, "write").as_str(), "Y" | "true");
    if write {
        let permission = if session.kind == Kind::Host {
            HOST_PERMISSION
        } else {
            COMMAND_PERMISSION
        };
        if !moqui_core::user::user_has_permission(&user_id, permission).unwrap_or(false) {
            return refuse(
                "noPermission",
                &format!("{with} may watch it, but typing in it needs the {permission} permission"),
            );
        }
    }
    let written = (|| -> Result<(), moqui_core::context::Error> {
        let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
        close_rows(facade, &session.id, Some(&with))?;
        let mut row = facade.make_value(ENTITY)?;
        row.set("terminalSessionId", Value::string(&session.id));
        row.set("username", Value::string(&with));
        row.set("fromDate", moqui_core::user::now_value());
        row.set("sharedByUsername", Value::string(&owner));
        row.set("writeAllowed", Value::string(if write { "Y" } else { "N" }));
        row.create(facade)?;
        Ok(())
    })();
    if let Err(err) = written {
        return Err(err.into());
    }
    session.share(&with, write);
    tracing::info!(
        "terminal session {} of {owner} shared with {with}{}",
        session.id,
        if write { ", who may type" } else { " to watch" }
    );
    answer("session", &session.to_json())
}

/// `revoke#TerminalSessionShare`: take `username`'s share back; their windows on the session are let go.
pub fn revoke_terminal_session_share(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let (session, owner) = match owned(parameters) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let with = text(parameters, "username");
    let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
    close_rows(facade, &session.id, Some(&with))?;
    if session.unshare(&with) {
        tracing::info!("terminal session {} of {owner}: {with}'s share revoked", session.id);
    }
    answer("session", &session.to_json())
}

/// Close the share rows of session `id`, which ended: on the request pool, a failure logged.
pub(crate) fn close_on_end(factory: Weak<ExecutionContextFactory>, id: String) {
    crate::edge::handle().spawn(async move {
        let row_id = id.clone();
        let closed = crate::worker::on_pool_without_authz(&factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_core::context::Error::new("the entity facade is not installed"))?;
            close_rows(&facade, &row_id, None)
        })
        .await;
        if let Err(err) = closed {
            tracing::error!("terminal session {id}: its shares were not closed: {}", err.message());
        }
    });
}

/// Read the open shares of `session` again, on the request pool (a held session taken back after a restart, D15).
pub(crate) fn reload(factory: Weak<ExecutionContextFactory>, session: std::sync::Arc<Session>) {
    crate::edge::handle().spawn(async move {
        let id = session.id.clone();
        let read = crate::worker::on_pool_without_authz(&factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_core::context::Error::new("the entity facade is not installed"))?;
            let rows = facade
                .find(ENTITY)?
                .eq("terminalSessionId", Value::string(&id))
                .eq("thruDate", Value::Null)
                .disable_authz()
                .list()?;
            Ok(rows
                .values()
                .iter()
                .map(|row| {
                    let with = row.get("username").map(to_plain_string).unwrap_or_default();
                    (
                        with,
                        row.get("writeAllowed").map(to_plain_string).as_deref() == Some("Y"),
                    )
                })
                .collect::<Vec<_>>())
        })
        .await;
        match read {
            Ok(shares) => {
                for (with, write) in shares.into_iter().filter(|(with, _)| !with.is_empty()) {
                    session.share(&with, write);
                }
            }
            Err(err) => tracing::error!(
                "terminal session {}: its shares were not read: {}",
                session.id,
                err.message()
            ),
        }
    });
}
