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

//! The command pipeline (plans/agent-code/02-Orchestration.md section 1, T3's `orchestration.dispatchCommand`): one
//! command, with a client-made `commandId`, runs in one transaction that writes the entity rows, the change rows, the
//! effect rows and the receipt. A command whose `commandId` has a receipt answers the stored result and writes nothing,
//! so a client may send a command again after a dropped socket.
//!
//! A command on a thread locks the `AgentConversation` row first (`for update`). The database holds the lock, so two
//! nodes of a cluster never run two commands of one thread at once.
//!
//! Each command type is a handler in a file of its group ([`thread`]). The dispatcher checks the type, the
//! internal-only flag and the thread's owner; the handler checks its fields and any further permission.

pub mod thread;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::json;

use crate::change::{self, ChangeSpec};
use crate::effect::{self, EffectSpec};
use crate::shell::CONVERSATION;
use crate::support::{in_tx, int_of, json_of, json_text, now_millis, stamp, text_of};

pub(crate) const COMMAND: &str = "moqui.agent.code.AgentCodeCommand";

/// The longest `commandId` (T3 `CommandId` is a UUID; the field is an `id`).
pub const COMMAND_ID_MAX: usize = 40;

/// Why a command was refused. The wire error carries `code` and `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
    pub required_permission: Option<&'static str>,
}

impl CommandError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_command",
            message: message.into(),
            required_permission: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: message.into(),
            required_permission: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: "conflict",
            message: message.into(),
            required_permission: None,
        }
    }

    #[must_use]
    pub fn forbidden(permission: &'static str) -> Self {
        Self {
            code: "forbidden",
            message: format!("this needs the {permission} permission"),
            required_permission: Some(permission),
        }
    }

    fn failed(err: &Error) -> Self {
        Self {
            code: "failed",
            message: err.to_string(),
            required_permission: None,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// What a handler did: its result, and the changes and effects to store with it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    pub result: serde_json::Value,
    pub changes: Vec<ChangeSpec>,
    pub effects: Vec<EffectSpec>,
}

/// One command as a handler sees it.
pub struct Call<'a> {
    pub facade: &'a EntityFacade,
    pub user_id: &'a str,
    pub command: &'a serde_json::Value,
    pub now: i64,
    /// The thread the command names, locked, when the command has a `threadId`.
    pub thread: Option<EntityValue>,
}

impl Call<'_> {
    /// A text field of the command, trimmed; `None` when absent, empty or not text.
    #[must_use]
    pub fn text(&self, field: &str) -> Option<String> {
        self.command
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    }

    #[must_use]
    pub fn flag(&self, field: &str) -> Option<bool> {
        self.command.get(field).and_then(serde_json::Value::as_bool)
    }

    /// The locked thread; an error when the command named none.
    ///
    /// # Errors
    ///
    /// When the command has no `threadId`.
    pub fn thread(&self) -> Result<&EntityValue, CommandError> {
        self.thread
            .as_ref()
            .ok_or_else(|| CommandError::invalid("the command needs a threadId"))
    }

    #[must_use]
    pub fn thread_id(&self) -> String {
        self.thread
            .as_ref()
            .and_then(|row| text_of(row, "conversationId"))
            .unwrap_or_default()
    }
}

/// A command handler.
pub type Handler = fn(&mut Call<'_>) -> Result<Outcome, CommandError>;

/// One command type.
pub struct CommandType {
    pub name: &'static str,
    pub handler: Handler,
    /// Accepted only from the server's own code (T3's internal commands), never from a client.
    pub internal: bool,
    /// The command makes its thread (`threadId` names a thread that does not exist yet).
    pub creates_thread: bool,
}

/// Every command type this server runs.
#[must_use]
pub fn command_types() -> &'static [CommandType] {
    thread::TYPES
}

fn command_type(name: &str) -> Option<&'static CommandType> {
    command_types().iter().find(|kind| kind.name == name)
}

/// A command's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub command_id: String,
    /// The last change number the command wrote, `None` when it wrote no change.
    pub sequence: Option<i64>,
    pub result: serde_json::Value,
    /// The answer is a stored one: the command ran before.
    pub replayed: bool,
}

impl Receipt {
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "commandId": self.command_id,
            "sequence": self.sequence,
            "result": self.result,
            "replayed": self.replayed,
        })
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= COMMAND_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn stored(facade: &EntityFacade, command_id: &str, user_id: &str) -> Result<Option<Receipt>, CommandError> {
    let row = facade
        .find(COMMAND)
        .and_then(|find| find.eq("commandId", Value::string(command_id)).disable_authz().one())
        .map_err(|err| CommandError::failed(&err))?;
    let Some(row) = row else {
        return Ok(None);
    };
    if text_of(&row, "userId").as_deref() != Some(user_id) {
        return Err(CommandError::conflict(format!("commandId {command_id} is used")));
    }
    Ok(Some(Receipt {
        command_id: command_id.to_string(),
        sequence: int_of(&row, "changeSeq"),
        result: json_of(&row, "resultJson"),
        replayed: true,
    }))
}

/// Run `command` for `user_id`. `internal` is true only for the server's own callers.
///
/// # Errors
///
/// A command that is not valid, not allowed, names a thread the user does not own, or that a handler refuses; and a
/// storage failure.
pub fn dispatch(
    facade: &EntityFacade,
    user_id: &str,
    command: &serde_json::Value,
    internal: bool,
) -> Result<Receipt, CommandError> {
    let name = command
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CommandError::invalid("the command needs a type"))?;
    let command_id = command
        .get("commandId")
        .and_then(serde_json::Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| CommandError::invalid("the command needs a commandId of 1 to 40 letters, digits, '-', '_' or '.'"))?
        .to_string();
    let kind = command_type(name).ok_or_else(|| CommandError {
        code: "unknown_command",
        message: format!("{name} is not a command"),
        required_permission: None,
    })?;
    if kind.internal && !internal {
        return Err(CommandError {
            code: "internal_only",
            message: format!("{name} is accepted only from the server"),
            required_permission: None,
        });
    }
    if let Some(receipt) = stored(facade, &command_id, user_id)? {
        return Ok(receipt);
    }
    let thread_id = command
        .get("threadId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let mut refused = None;
    let ran = in_tx(facade, COMMAND, || {
        match run(facade, user_id, command, kind, &command_id, thread_id.as_deref()) {
            Ok(receipt) => Ok(receipt),
            Err(err) => {
                let text = err.to_string();
                refused = Some(err);
                Err(Error::new(text))
            }
        }
    });
    match ran {
        Ok(receipt) => Ok(receipt),
        Err(err) => {
            if let Some(refused) = refused {
                return Err(refused);
            }
            // two sends of one command at once: the other one stored the receipt
            if let Ok(Some(receipt)) = stored(facade, &command_id, user_id) {
                return Ok(receipt);
            }
            Err(CommandError::failed(&err))
        }
    }
}

fn run(
    facade: &EntityFacade,
    user_id: &str,
    command: &serde_json::Value,
    kind: &CommandType,
    command_id: &str,
    thread_id: Option<&str>,
) -> Result<Receipt, CommandError> {
    let failed = |err: Error| CommandError::failed(&err);
    let thread = match (thread_id, kind.creates_thread) {
        (Some(id), false) => {
            let row = facade
                .find(CONVERSATION)
                .map_err(failed)?
                .eq("conversationId", Value::string(id))
                .for_update(true)
                .disable_authz()
                .one()
                .map_err(failed)?
                .filter(|row| text_of(row, "userId").as_deref() == Some(user_id))
                .filter(|row| text_of(row, "statusId").as_deref() != Some("AgcDeleted"))
                .ok_or_else(|| CommandError::not_found(format!("there is no thread {id}")))?;
            Some(row)
        }
        _ => None,
    };
    let mut call = Call {
        facade,
        user_id,
        command,
        now: now_millis(),
        thread,
    };
    let outcome = (kind.handler)(&mut call)?;
    let mut sequence = None;
    for spec in &outcome.changes {
        sequence = Some(change::append(facade, spec).map_err(failed)?);
    }
    for spec in &outcome.effects {
        effect::enqueue(facade, spec).map_err(failed)?;
    }
    let mut receipt = facade.make_value(COMMAND).map_err(failed)?;
    receipt.set("commandId", Value::string(command_id));
    receipt.set("commandType", Value::string(kind.name));
    receipt.set("userId", Value::string(user_id));
    let conversation_id = outcome
        .result
        .get("threadId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| thread_id.map(str::to_string));
    if let Some(id) = conversation_id {
        receipt.set("conversationId", Value::string(id));
    }
    receipt.set("receivedDate", stamp(call.now));
    receipt.set("resultJson", Value::string(json_text(&outcome.result)));
    if let Some(seq) = sequence {
        receipt.set("changeSeq", Value::I64(seq));
    }
    receipt.create(facade).map_err(failed)?;
    Ok(Receipt {
        command_id: command_id.to_string(),
        sequence,
        result: outcome.result,
        replayed: false,
    })
}

/// Delete receipts older than 7 days (T3 keeps them about a week).
///
/// # Errors
///
/// When the delete fails.
pub fn clean_receipts(facade: &EntityFacade) -> Result<usize, Error> {
    let before = now_millis() - 7 * 24 * 60 * 60 * 1000;
    let removed = facade
        .find(COMMAND)?
        .condition(moqui_db::sql::Condition::less("receivedDate", stamp(before)))
        .disable_authz()
        .delete_all()?;
    Ok(usize::try_from(removed).unwrap_or_default())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
