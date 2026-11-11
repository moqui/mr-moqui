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

//! The terminal's audit (plans/Terminal-Fancy.md §5.7): a `moqui.terminal.TerminalSession` row for each command line
//! and host session, written when it is opened and when it ends, with an INFO line each. What is typed and shown is
//! never recorded. A row that cannot be written is logged and the session goes on.

use std::sync::Weak;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::Value;

use super::access::{Facts, Place};
use super::session::Kind;

const ENTITY: &str = "moqui.terminal.TerminalSession";

/// What the opening row says of a session.
pub(crate) struct Opened<'a> {
    pub id: &'a str,
    pub username: &'a str,
    pub kind: Kind,
    pub profile: &'a str,
    pub command_line: &'a str,
    pub directory: &'a str,
    pub facts: &'a Facts,
}

fn flag(value: bool) -> Value {
    Value::string(if value { "Y" } else { "N" })
}

/// Write the opening row, as the signed-in user on this thread.
pub(crate) fn record_open(opened: &Opened<'_>) {
    let (remote, via_edge) = match opened.facts.place() {
        Place::Local => (false, false),
        Place::Remote { via_edge, .. } => (true, via_edge),
    };
    tracing::info!(
        "terminal {} session {} opened for {} from {}{}{}",
        opened.kind.as_str(),
        opened.id,
        opened.username,
        if opened.facts.client_ip.is_empty() {
            "this machine"
        } else {
            opened.facts.client_ip.as_str()
        },
        if remote { ", remote" } else { "" },
        if opened.command_line.is_empty() {
            String::new()
        } else {
            format!(": {}", opened.command_line)
        },
    );
    let written = ExecutionContext::with_current(|ec| -> Result<(), moqui_core::context::Error> {
        let _authz = ec.disable_authz();
        let facade = ec
            .factory()
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("the entity facade is not installed"))?;
        let mut row = facade.make_value(ENTITY)?;
        row.set("terminalSessionId", Value::string(opened.id));
        if let Ok(Some(user_id)) = moqui_core::user::current_user_id() {
            row.set("userId", Value::string(&user_id));
        }
        row.set("username", Value::string(opened.username));
        row.set("kind", Value::string(opened.kind.as_str()));
        row.set("profileName", Value::string(opened.profile));
        row.set("commandLine", Value::string(opened.command_line));
        row.set("directory", Value::string(opened.directory));
        row.set("clientIp", Value::string(&opened.facts.client_ip));
        row.set("remote", flag(remote));
        row.set("viaEdge", flag(via_edge));
        if let Ok(Some(visit_id)) = moqui_core::user::visit_id() {
            row.set("visitId", Value::string(&visit_id));
        }
        row.set("fromDate", moqui_core::user::now_value());
        row.create(&facade)?;
        Ok(())
    });
    if let Some(Err(err)) = written {
        tracing::error!(
            "terminal session {}: its audit row was not written: {}",
            opened.id,
            err.message()
        );
    }
}

/// What writes the ending of session `id` when it ends: its exit status and the reason, on the request pool.
pub(crate) fn on_end(factory: Weak<ExecutionContextFactory>, id: String) -> Box<dyn FnOnce(u32, String) + Send> {
    Box::new(move |status, reason| {
        crate::edge::handle().spawn(async move {
            let row_id = id.clone();
            let written = crate::worker::on_pool_without_authz(&factory, move |factory| {
                let facade = factory
                    .entity()
                    .ok_or_else(|| moqui_core::context::Error::new("the entity facade is not installed"))?;
                let mut row = facade.make_value(ENTITY)?;
                row.set("terminalSessionId", Value::string(&row_id));
                row.set("thruDate", moqui_core::user::now_value());
                row.set("exitStatus", Value::I64(i64::from(status)));
                row.set("endReason", Value::string(&reason));
                row.update(&facade)?;
                Ok(())
            })
            .await;
            if let Err(err) = written {
                tracing::error!(
                    "terminal session {id}: its audit row's end was not written: {}",
                    err.message()
                );
            }
        });
    })
}
