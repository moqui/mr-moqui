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

//! The thread commands of the sidebar (plans/agent-code/02-Orchestration.md section 1.1): create, archive, delete,
//! settle, snooze, pin and reorder, tags, visit and unread, metadata, and the mode and model pickers. Each one answers
//! with the thread's new summary and writes a `thread.upserted` change for the user (`thread.removed` for a delete).
//!
//! Not here yet: the checks on delegated tasks and side chats (archive, delete) come with tasks in phase P4; the
//! worktree cleanup on delete comes with work roots in phase P2.

use moqui_core::entity::EntityValue;
use moqui_lib::object::Value;
use serde_json::json;

use super::{Call, CommandError, CommandType, Outcome};
use crate::change::ChangeSpec;
use crate::effect::EffectSpec;
use crate::order_key;
use crate::shell::{
    interaction_mode_enum, runtime_mode_enum, runtime_mode_rank, thread_for, CONVERSATION, CONVERSATION_USER,
    SURFACE, TAG,
};
use crate::support::{int_of, parse_iso, stamp, text_of};

/// The longest tag (T3: 64, one line).
pub const TAG_MAX: usize = 64;
/// The longest title.
pub const TITLE_MAX: usize = 255;
/// The longest search text a visit carries (T3: 200).
pub const SEARCH_MAX: usize = 200;

const fn client(name: &'static str, handler: super::Handler) -> CommandType {
    CommandType {
        name,
        handler,
        internal: false,
        creates_thread: false,
    }
}

pub(super) const TYPES: &[CommandType] = &[
    CommandType {
        name: "thread.create",
        handler: create,
        internal: false,
        creates_thread: true,
    },
    client("thread.archive", archive),
    client("thread.unarchive", unarchive),
    client("thread.delete", delete),
    client("thread.settle", settle),
    client("thread.unsettle", unsettle),
    client("thread.snooze", snooze),
    client("thread.unsnooze", unsnooze),
    client("thread.auto-settle.set", auto_settle_set),
    client("thread.pin", pin),
    client("thread.unpin", unpin),
    client("thread.pin.reorder", pin_reorder),
    client("thread.active.reorder", active_reorder),
    client("thread.tag.set", tag_set),
    client("thread.visit", visit),
    client("thread.mark-unread", mark_unread),
    client("thread.notify.set", notify_set),
    client("thread.metadata.update", metadata_update),
    client("thread.runtime-mode.set", runtime_mode_set),
    client("thread.interaction-mode.set", interaction_mode_set),
    client("thread.model-selection.set", model_selection_set),
    CommandType {
        name: "thread.title.regeneration.complete",
        handler: title_complete,
        internal: true,
        creates_thread: false,
    },
];

fn failed(err: &moqui_lib::error::Error) -> CommandError {
    CommandError::failed(err)
}

/// The answer and change of a command that changed `thread_id`.
fn upserted(call: &Call<'_>, thread_id: &str) -> Result<Outcome, CommandError> {
    let summary = thread_for(call.facade, thread_id, call.user_id)
        .map_err(|err| failed(&err))?
        .ok_or_else(|| CommandError::not_found(format!("there is no thread {thread_id}")))?;
    Ok(Outcome {
        result: json!({"threadId": thread_id, "thread": summary}),
        changes: vec![ChangeSpec {
            change_type: "thread.upserted".into(),
            conversation_id: Some(thread_id.to_string()),
            project_id: summary
                .get("projectId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            user_id: Some(call.user_id.to_string()),
            payload: summary,
        }],
        effects: Vec::new(),
    })
}

/// The user's row of the thread, made when it is missing.
fn user_row(call: &Call<'_>) -> Result<EntityValue, CommandError> {
    let thread_id = call.thread_id();
    let found = call
        .facade
        .find(CONVERSATION_USER)
        .and_then(|find| {
            find.eq("conversationId", Value::string(&thread_id))
                .eq("userId", Value::string(call.user_id))
                .for_update(true)
                .disable_authz()
                .one()
        })
        .map_err(|err| failed(&err))?;
    if let Some(row) = found {
        return Ok(row);
    }
    let mut row = call.facade.make_value(CONVERSATION_USER).map_err(|err| failed(&err))?;
    row.set("conversationId", Value::string(&thread_id));
    row.set("userId", Value::string(call.user_id));
    Ok(row)
}

/// Set `fields` on the user's row of the thread and answer the new summary.
fn set_user(call: &Call<'_>, fields: Vec<(&str, Value)>) -> Result<Outcome, CommandError> {
    let mut row = user_row(call)?;
    for (field, value) in fields {
        row.set(field, value);
    }
    row.create_or_update(call.facade).map_err(|err| failed(&err))?;
    upserted(call, &call.thread_id())
}

/// Set `fields` on the thread row and answer the new summary.
fn set_thread(call: &mut Call<'_>, fields: Vec<(&str, Value)>) -> Result<Outcome, CommandError> {
    let thread_id = call.thread_id();
    let row = call
        .thread
        .as_mut()
        .ok_or_else(|| CommandError::invalid("the command needs a threadId"))?;
    for (field, value) in fields {
        row.set(field, value);
    }
    row.update(call.facade).map_err(|err| failed(&err))?;
    upserted(call, &thread_id)
}

fn date_field(call: &Call<'_>, field: &str) -> Result<Option<i64>, CommandError> {
    match call.text(field) {
        None => Ok(None),
        Some(text) => parse_iso(&text)
            .map(Some)
            .ok_or_else(|| CommandError::invalid(format!("{field} is not an ISO date-time"))),
    }
}

fn require_active(call: &Call<'_>) -> Result<(), CommandError> {
    if text_of(call.thread()?, "statusId").as_deref() == Some("AgcArchived") {
        return Err(CommandError::conflict("the thread is archived"));
    }
    Ok(())
}

/// The highest runtime mode the policy allows (`limits.maxRuntimeMode`), as an enum id.
fn max_runtime_mode(call: &Call<'_>) -> String {
    crate::settings::policy_value(call.facade, "limits.maxRuntimeMode")
        .as_str()
        .and_then(runtime_mode_enum)
        .unwrap_or("AgrmFull")
        .to_string()
}

fn runtime_mode(call: &Call<'_>, name: &str) -> Result<&'static str, CommandError> {
    let mode = runtime_mode_enum(name).ok_or_else(|| CommandError::invalid(format!("{name} is not a runtime mode")))?;
    let ceiling = max_runtime_mode(call);
    if runtime_mode_rank(mode) > runtime_mode_rank(&ceiling) {
        return Err(CommandError::invalid(format!(
            "runtime mode {name} is above the limit of this server, {}",
            crate::shell::runtime_mode_name(&ceiling)
        )));
    }
    Ok(mode)
}

/// The fields of a `modelSelection` object.
fn model_fields(selection: &serde_json::Value) -> Result<Vec<(&'static str, Value)>, CommandError> {
    let object = selection
        .as_object()
        .ok_or_else(|| CommandError::invalid("modelSelection is not an object"))?;
    let text = |key: &str| object.get(key).and_then(serde_json::Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let instance = text("instanceId").unwrap_or("native");
    let mut fields = Vec::new();
    if instance == "native" {
        fields.push(("engineEnumId", Value::string("AgengNative")));
        fields.push(("providerInstanceId", Value::Null));
        fields.push(("harnessModel", Value::Null));
        fields.push(("modelId", text("model").map_or(Value::Null, Value::string)));
    } else {
        fields.push(("engineEnumId", Value::string("AgengHarness")));
        fields.push(("providerInstanceId", Value::string(instance)));
        fields.push(("harnessModel", text("model").map_or(Value::Null, Value::string)));
    }
    fields.push(("reasoningLevel", text("effort").map_or(Value::Null, Value::string)));
    fields.push((
        "modelOptionsJson",
        object
            .get("options")
            .filter(|options| !options.is_null())
            .map_or(Value::Null, |options| Value::string(options.to_string())),
    ));
    Ok(fields)
}

fn one_line(text: &str) -> bool {
    !text.contains(['\n', '\r'])
}

fn create(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let facade = call.facade;
    let mut row = facade.make_value(CONVERSATION).map_err(|err| failed(&err))?;
    match call.text("threadId") {
        Some(id) => {
            if !super::valid_id(&id) {
                return Err(CommandError::invalid("threadId is not a plain id of 40 characters or less"));
            }
            let taken = facade
                .find(CONVERSATION)
                .and_then(|find| find.eq("conversationId", Value::string(&id)).disable_authz().count())
                .map_err(|err| failed(&err))?;
            if taken > 0 {
                return Err(CommandError::conflict(format!("thread {id} exists")));
            }
            row.set("conversationId", Value::string(id));
        }
        None => row.set_sequenced_id_primary(facade).map_err(|err| failed(&err))?,
    }
    let thread_id = text_of(&row, "conversationId").unwrap_or_default();
    if let Some(project_id) = call.text("projectId") {
        let exists = facade
            .find("moqui.agent.code.AgentProject")
            .and_then(|find| find.eq("projectId", Value::string(&project_id)).disable_authz().count())
            .map_err(|err| failed(&err))?;
        if exists == 0 {
            return Err(CommandError::not_found(format!("there is no project {project_id}")));
        }
        row.set("projectId", Value::string(project_id));
    }
    let title = call.text("title");
    if let Some(title) = &title {
        if title.chars().count() > TITLE_MAX || !one_line(title) {
            return Err(CommandError::invalid("title is one line of 255 characters or less"));
        }
        row.set("title", Value::string(title));
    }
    row.set("titleSourceEnumId", Value::string(if title.is_some() { "AgtsUser" } else { "AgtsDefault" }));
    let default_mode = crate::settings::server_value(facade, "defaultRuntimeMode");
    let mode_name = call
        .text("runtimeMode")
        .or_else(|| default_mode.as_str().map(str::to_string))
        .unwrap_or_else(|| "approval-required".into());
    row.set("runtimeModeEnumId", Value::string(runtime_mode(call, &mode_name)?));
    let interaction = call.text("interactionMode").unwrap_or_else(|| "default".into());
    row.set(
        "interactionModeEnumId",
        Value::string(
            interaction_mode_enum(&interaction)
                .ok_or_else(|| CommandError::invalid(format!("{interaction} is not an interaction mode")))?,
        ),
    );
    match call.command.get("modelSelection").filter(|value| !value.is_null()) {
        Some(selection) => {
            for (field, value) in model_fields(selection)? {
                row.set(field, value);
            }
        }
        None => row.set("engineEnumId", Value::string("AgengNative")),
    }
    if let Some(branch) = call.text("branch") {
        row.set("branch", Value::string(branch));
    }
    row.set("userId", Value::string(call.user_id));
    row.set("surfaceEnumId", Value::string(SURFACE));
    row.set("statusId", Value::string("AgcActive"));
    row.set("creationSourceEnumId", Value::string("AgcsUser"));
    row.set("lastMessageSeq", Value::I64(0));
    row.set("createdDate", stamp(call.now));
    row.create(facade).map_err(|err| failed(&err))?;
    let mut user = facade.make_value(CONVERSATION_USER).map_err(|err| failed(&err))?;
    user.set("conversationId", Value::string(&thread_id));
    user.set("userId", Value::string(call.user_id));
    user.set("lastVisitedDate", stamp(call.now));
    user.set("readThroughSeq", Value::I64(0));
    user.create(facade).map_err(|err| failed(&err))?;
    upserted(call, &thread_id)
}

fn archive(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    if text_of(call.thread()?, "statusId").as_deref() == Some("AgcArchived") {
        return upserted(call, &call.thread_id());
    }
    let now = call.now;
    set_thread(call, vec![("statusId", Value::string("AgcArchived")), ("archivedDate", stamp(now))])
}

fn unarchive(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    set_thread(call, vec![("statusId", Value::string("AgcActive")), ("archivedDate", Value::Null)])
}

fn delete(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let thread = call.thread()?;
    if call.flag("onlyIfUnused") == Some(true) {
        let used = int_of(thread, "lastMessageSeq").unwrap_or(0) > 0 || text_of(thread, "activeRunId").is_some();
        if used {
            return Ok(Outcome {
                result: json!({"threadId": call.thread_id(), "deleted": false}),
                ..Outcome::default()
            });
        }
    }
    let thread_id = call.thread_id();
    let now = call.now;
    let row = call
        .thread
        .as_mut()
        .ok_or_else(|| CommandError::invalid("the command needs a threadId"))?;
    row.set("statusId", Value::string("AgcDeleted"));
    row.set("deletedDate", stamp(now));
    row.update(call.facade).map_err(|err| failed(&err))?;
    Ok(Outcome {
        result: json!({"threadId": thread_id, "deleted": true}),
        changes: vec![ChangeSpec {
            change_type: "thread.removed".into(),
            conversation_id: Some(thread_id.clone()),
            project_id: text_of(row, "projectId"),
            user_id: Some(call.user_id.to_string()),
            payload: json!({"threadId": thread_id}),
        }],
        effects: Vec::new(),
    })
}

/// The newest run of the thread.
fn latest_run(call: &Call<'_>) -> Result<Option<String>, CommandError> {
    let rows = call
        .facade
        .find("moqui.agent.AgentRun")
        .and_then(|find| {
            find.eq("conversationId", Value::string(call.thread_id()))
                .order_by("submittedDate", true)
                .limit(1)
                .disable_authz()
                .list()
        })
        .map_err(|err| failed(&err))?;
    let run_id = rows.iter().next().and_then(|row| text_of(row, "runId"));
    Ok(run_id)
}

fn settle(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    if let Some(expected) = call.text("expectedLatestRunId") {
        if latest_run(call)?.as_deref() != Some(expected.as_str()) {
            return Err(CommandError::conflict("the thread has a newer run than expectedLatestRunId"));
        }
    }
    let at = date_field(call, "settledAt")?.unwrap_or(call.now);
    set_user(call, vec![("settledDate", stamp(at)), ("unsettleRestoreDate", Value::Null)])
}

fn unsettle(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let restore = date_field(call, "restoreUnsettledAt")?;
    set_user(
        call,
        vec![
            ("settledDate", Value::Null),
            ("unsettleRestoreDate", restore.map_or(Value::Null, stamp)),
            ("activeOrderKey", Value::Null),
        ],
    )
}

fn snooze(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let until = date_field(call, "until")?.ok_or_else(|| CommandError::invalid("snooze needs until"))?;
    if until <= call.now {
        return Err(CommandError::invalid("until is not in the future"));
    }
    set_user(call, vec![("snoozeUntilDate", stamp(until))])
}

fn unsnooze(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    set_user(call, vec![("snoozeUntilDate", Value::Null)])
}

fn auto_settle_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let enabled = call.flag("enabled").ok_or_else(|| CommandError::invalid("enabled is true or false"))?;
    set_thread(call, vec![("autoSettleDisabled", Value::string(if enabled { "N" } else { "Y" }))])
}

/// The lowest key of `field` among the user's threads, so a new pin goes on top.
fn top_key(call: &Call<'_>, field: &str) -> Result<Option<String>, CommandError> {
    let rows = call
        .facade
        .find(CONVERSATION_USER)
        .and_then(|find| {
            find.eq("userId", Value::string(call.user_id))
                .condition(moqui_db::sql::Condition::is_not_null(field))
                .order_by(field, false)
                .limit(1)
                .disable_authz()
                .list()
        })
        .map_err(|err| failed(&err))?;
    let key = rows.iter().next().and_then(|row| text_of(row, field)).filter(|key| order_key::is_valid(key));
    Ok(key)
}

fn pin(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    if user_row(call)?.get("pinnedOrderKey").is_some_and(|key| !key.is_null()) {
        return upserted(call, &call.thread_id());
    }
    let top = top_key(call, "pinnedOrderKey")?;
    let key = order_key::key_between(None, top.as_deref()).map_err(|err| CommandError::conflict(err.to_string()))?;
    set_user(call, vec![("pinnedOrderKey", Value::string(key))])
}

fn unpin(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    set_user(call, vec![("pinnedOrderKey", Value::Null)])
}

fn order_key_field(call: &Call<'_>) -> Result<String, CommandError> {
    call.text("orderKey")
        .filter(|key| order_key::is_valid(key))
        .ok_or_else(|| CommandError::invalid("orderKey is not an order key"))
}

fn pin_reorder(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let key = order_key_field(call)?;
    set_user(call, vec![("pinnedOrderKey", Value::string(key))])
}

fn active_reorder(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let key = order_key_field(call)?;
    set_user(call, vec![("activeOrderKey", Value::string(key))])
}

/// A tag as T3 takes it: trimmed, one line, 1 to 64 characters, matched exactly.
fn clean_tag(tag: &str) -> Option<String> {
    let tag = tag.trim();
    (!tag.is_empty() && tag.chars().count() <= TAG_MAX && one_line(tag)).then(|| tag.to_string())
}

fn tag_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let tag = call
        .command
        .get("tag")
        .and_then(serde_json::Value::as_str)
        .and_then(clean_tag)
        .ok_or_else(|| CommandError::invalid("tag is one line of 1 to 64 characters"))?;
    let tagged = call.flag("tagged").unwrap_or(true);
    let thread_id = call.thread_id();
    let find = call
        .facade
        .find(TAG)
        .map(|find| {
            find.eq("conversationId", Value::string(&thread_id))
                .eq("tag", Value::string(&tag))
                .eq("userId", Value::string(call.user_id))
                .disable_authz()
        })
        .map_err(|err| failed(&err))?;
    let present = find.count().map_err(|err| failed(&err))? > 0;
    if tagged && !present {
        let mut row = call.facade.make_value(TAG).map_err(|err| failed(&err))?;
        row.set("conversationId", Value::string(&thread_id));
        row.set("tag", Value::string(&tag));
        row.set("userId", Value::string(call.user_id));
        row.set("taggedDate", stamp(call.now));
        row.create(call.facade).map_err(|err| failed(&err))?;
    } else if !tagged && present {
        find.delete_all().map_err(|err| failed(&err))?;
    }
    upserted(call, &thread_id)
}

fn visit(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    if call.text("searchQuery").is_some_and(|query| query.chars().count() > SEARCH_MAX) {
        return Err(CommandError::invalid("searchQuery is 200 characters or less"));
    }
    let at = date_field(call, "visitedAt")?.unwrap_or(call.now);
    let mut fields = vec![("lastVisitedDate", stamp(at))];
    if call.flag("opened").unwrap_or(true) {
        let seq = int_of(call.thread()?, "lastMessageSeq").unwrap_or(0);
        fields.push(("readThroughSeq", Value::I64(seq)));
        fields.push(("markedUnread", Value::string("N")));
    }
    set_user(call, fields)
}

fn mark_unread(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    set_user(call, vec![("markedUnread", Value::string("Y"))])
}

fn notify_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let notify = match call.text("notify").as_deref() {
        Some("default") => "AgcnDefault",
        Some("all") => "AgcnAll",
        Some("none") => "AgcnNone",
        _ => return Err(CommandError::invalid("notify is default, all or none")),
    };
    set_user(call, vec![("notifyEnumId", Value::string(notify))])
}

/// Compare-and-set: when the command has `expected<Field>`, the thread's field must equal it (null for an empty one).
fn expect(call: &Call<'_>, wire: &str, field: &str) -> Result<(), CommandError> {
    let key = format!("expected{}{}", wire[..1].to_uppercase(), &wire[1..]);
    let Some(expected) = call.command.get(&key) else {
        return Ok(());
    };
    let current = text_of(call.thread()?, field);
    if expected.as_str().map(str::trim).filter(|t| !t.is_empty()) != current.as_deref() {
        return Err(CommandError::conflict(format!("{wire} is not the expected value")));
    }
    Ok(())
}

fn metadata_update(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    require_active(call)?;
    let mut fields = Vec::new();
    if call.command.get("title").is_some() {
        expect(call, "title", "title")?;
        let title = call.text("title").ok_or_else(|| CommandError::invalid("title is empty"))?;
        if title.chars().count() > TITLE_MAX || !one_line(&title) {
            return Err(CommandError::invalid("title is one line of 255 characters or less"));
        }
        fields.push(("title", Value::string(title)));
        fields.push(("titleSourceEnumId", Value::string("AgtsUser")));
    }
    if call.command.get("branch").is_some() {
        expect(call, "branch", "branch")?;
        fields.push(("branch", call.text("branch").map_or(Value::Null, Value::string)));
    }
    if call.flag("expectedEmpty") == Some(true) && int_of(call.thread()?, "lastMessageSeq").unwrap_or(0) > 0 {
        return Err(CommandError::conflict("the thread is not empty"));
    }
    let regenerate = call.flag("regenerateTitle") == Some(true);
    if fields.is_empty() && !regenerate {
        return Err(CommandError::invalid("the update names no field"));
    }
    let mut outcome = if fields.is_empty() {
        upserted(call, &call.thread_id())?
    } else {
        set_thread(call, fields)?
    };
    if regenerate {
        outcome.effects.push(EffectSpec {
            effect_type: "title.generate".into(),
            conversation_id: Some(call.thread_id()),
            payload: json!({"threadId": call.thread_id(), "userId": call.user_id}),
        });
    }
    Ok(outcome)
}

fn title_complete(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let title = call.text("title").ok_or_else(|| CommandError::invalid("title is empty"))?;
    // a user's own title wins over a generated one
    if text_of(call.thread()?, "titleSourceEnumId").as_deref() == Some("AgtsUser") {
        return upserted(call, &call.thread_id());
    }
    let title: String = title.lines().next().unwrap_or_default().chars().take(TITLE_MAX).collect();
    set_thread(call, vec![("title", Value::string(title)), ("titleSourceEnumId", Value::string("AgtsGenerated"))])
}

fn runtime_mode_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let name = call.text("runtimeMode").ok_or_else(|| CommandError::invalid("runtimeMode is missing"))?;
    let mode = runtime_mode(call, &name)?;
    set_thread(call, vec![("runtimeModeEnumId", Value::string(mode))])
}

fn interaction_mode_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let name = call.text("interactionMode").unwrap_or_default();
    let mode = interaction_mode_enum(&name)
        .ok_or_else(|| CommandError::invalid(format!("{name} is not an interaction mode")))?;
    set_thread(call, vec![("interactionModeEnumId", Value::string(mode))])
}

fn model_selection_set(call: &mut Call<'_>) -> Result<Outcome, CommandError> {
    let selection = call
        .command
        .get("modelSelection")
        .ok_or_else(|| CommandError::invalid("modelSelection is missing"))?;
    let fields = model_fields(selection)?;
    set_thread(call, fields)
}

#[cfg(test)]
#[path = "thread_tests.rs"]
mod tests;
