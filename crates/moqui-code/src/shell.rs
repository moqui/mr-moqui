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

//! The shell snapshot (T3 `getShellSnapshot`, plans/agent-code/09-Wire-API.md section 3): the sidebar's threads for
//! one user with their per-user state, and the change number the snapshot is at, so the shell stream goes on from
//! there. A thread's summary here is also the payload of a `thread.upserted` change.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Map};

use crate::support::{int_of, iso, json_of, millis_of, text_of};

pub(crate) const CONVERSATION: &str = "moqui.agent.AgentConversation";
pub(crate) const CONVERSATION_USER: &str = "moqui.agent.code.AgentConversationUser";
pub(crate) const TAG: &str = "moqui.agent.code.AgentConversationTag";

/// The Agent Code surface of a conversation.
pub const SURFACE: &str = "AgsfCode";

/// Threads one snapshot carries at most (T3 keeps the sidebar to the latest; older ones load by search).
pub const SNAPSHOT_LIMIT: i64 = 1000;

/// The wire name of a runtime mode enum.
#[must_use]
pub fn runtime_mode_name(enum_id: &str) -> &'static str {
    match enum_id {
        "AgrmReadOnly" => "read-only",
        "AgrmAutoEdits" => "auto-accept-edits",
        "AgrmAuto" => "auto",
        "AgrmFull" => "full-access",
        _ => "approval-required",
    }
}

/// The enum of a runtime mode wire name.
#[must_use]
pub fn runtime_mode_enum(name: &str) -> Option<&'static str> {
    Some(match name {
        "read-only" => "AgrmReadOnly",
        "approval-required" => "AgrmApproval",
        "auto-accept-edits" => "AgrmAutoEdits",
        "auto" => "AgrmAuto",
        "full-access" => "AgrmFull",
        _ => return None,
    })
}

/// The rank of a runtime mode, lowest first, to check a raise against the ceiling.
#[must_use]
pub fn runtime_mode_rank(enum_id: &str) -> u8 {
    match enum_id {
        "AgrmReadOnly" => 0,
        "AgrmAutoEdits" => 2,
        "AgrmAuto" => 3,
        "AgrmFull" => 4,
        _ => 1,
    }
}

#[must_use]
pub fn interaction_mode_name(enum_id: &str) -> &'static str {
    if enum_id == "AgimPlan" {
        "plan"
    } else {
        "default"
    }
}

#[must_use]
pub fn interaction_mode_enum(name: &str) -> Option<&'static str> {
    match name {
        "default" => Some("AgimDefault"),
        "plan" => Some("AgimPlan"),
        _ => None,
    }
}

fn date(row: Option<&EntityValue>, field: &str) -> serde_json::Value {
    row.and_then(|row| millis_of(row, field))
        .map_or(serde_json::Value::Null, |millis| json!(iso(millis)))
}

fn field(row: &EntityValue, field: &str) -> serde_json::Value {
    text_of(row, field).map_or(serde_json::Value::Null, serde_json::Value::String)
}

/// The summary of one thread for `user`.
#[must_use]
pub fn thread_summary(conversation: &EntityValue, user: Option<&EntityValue>, tags: &[String]) -> serde_json::Value {
    let status = text_of(conversation, "statusId").unwrap_or_default();
    let engine = text_of(conversation, "engineEnumId").unwrap_or_else(|| "AgengNative".into());
    let instance = text_of(conversation, "providerInstanceId");
    let mut model = Map::new();
    model.insert(
        "instanceId".into(),
        json!(instance.clone().unwrap_or_else(|| "native".into())),
    );
    model.insert(
        "model".into(),
        json!(text_of(conversation, "harnessModel").or_else(|| text_of(conversation, "modelId"))),
    );
    if let Some(level) = text_of(conversation, "reasoningLevel") {
        model.insert("effort".into(), json!(level));
    }
    let options = json_of(conversation, "modelOptionsJson");
    if !options.is_null() {
        model.insert("options".into(), options);
    }
    let created = millis_of(conversation, "createdDate").unwrap_or_default();
    let updated = millis_of(conversation, "lastMessageDate").unwrap_or(created);
    json!({
        "id": text_of(conversation, "conversationId"),
        "projectId": field(conversation, "projectId"),
        "title": text_of(conversation, "title").unwrap_or_else(|| "New thread".into()),
        "titleSource": match text_of(conversation, "titleSourceEnumId").as_deref() {
            Some("AgtsUser") => "user",
            Some("AgtsGenerated") => "generated",
            _ => "default",
        },
        "engine": if engine == "AgengHarness" { "harness" } else { "native" },
        "modelSelection": model,
        "runtimeMode": runtime_mode_name(&text_of(conversation, "runtimeModeEnumId").unwrap_or_default()),
        "interactionMode": interaction_mode_name(&text_of(conversation, "interactionModeEnumId").unwrap_or_default()),
        "branch": field(conversation, "branch"),
        "workRootId": field(conversation, "workRootId"),
        "parentThreadId": field(conversation, "parentConversationId"),
        "latestRunId": field(conversation, "activeRunId"),
        "lastMessageSeq": int_of(conversation, "lastMessageSeq").unwrap_or(0),
        "autoSettleDisabled": text_of(conversation, "autoSettleDisabled").as_deref() == Some("Y"),
        "status": match status.as_str() {
            "AgcArchived" => "archived",
            "AgcDeleted" => "deleted",
            _ => "active",
        },
        "createdAt": iso(created),
        "updatedAt": iso(updated),
        "archivedAt": date(Some(conversation), "archivedDate"),
        "settledAt": date(user, "settledDate"),
        "snoozedUntil": date(user, "snoozeUntilDate"),
        "lastVisitedAt": date(user, "lastVisitedDate"),
        "pinnedOrderKey": user.map_or(serde_json::Value::Null, |row| field(row, "pinnedOrderKey")),
        "activeOrderKey": user.map_or(serde_json::Value::Null, |row| field(row, "activeOrderKey")),
        "readThroughSeq": user.and_then(|row| int_of(row, "readThroughSeq")).unwrap_or(0),
        "markedUnread": user.and_then(|row| text_of(row, "markedUnread")).as_deref() == Some("Y"),
        "notify": match user.and_then(|row| text_of(row, "notifyEnumId")).as_deref() {
            Some("AgcnAll") => "all",
            Some("AgcnNone") => "none",
            _ => "default",
        },
        "tags": tags,
    })
}

/// The tags of the threads `ids`, by thread.
fn tags_of(facade: &EntityFacade, user_id: &str, ids: &[String]) -> Result<BTreeMap<String, Vec<String>>, Error> {
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if ids.is_empty() {
        return Ok(tags);
    }
    for chunk in ids.chunks(500) {
        let rows = facade
            .find(TAG)?
            .condition(Condition::is_in(
                "conversationId",
                chunk.iter().map(Value::string).collect(),
            ))
            .eq("userId", Value::string(user_id))
            .order_by("tag", false)
            .disable_authz()
            .list()?;
        for row in rows.iter() {
            if let (Some(id), Some(tag)) = (text_of(row, "conversationId"), text_of(row, "tag")) {
                tags.entry(id).or_default().push(tag);
            }
        }
    }
    Ok(tags)
}

/// The summary of the thread `conversation_id` for `user_id`, `None` when it is not one of the user's threads.
///
/// # Errors
///
/// When a find fails.
pub fn thread_for(facade: &EntityFacade, conversation_id: &str, user_id: &str) -> Result<Option<serde_json::Value>, Error> {
    let Some(conversation) = facade
        .find(CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let user = facade
        .find(CONVERSATION_USER)?
        .eq("conversationId", Value::string(conversation_id))
        .eq("userId", Value::string(user_id))
        .disable_authz()
        .one()?;
    let tags = tags_of(facade, user_id, &[conversation_id.to_string()])?
        .remove(conversation_id)
        .unwrap_or_default();
    Ok(Some(thread_summary(&conversation, user.as_ref(), &tags)))
}

/// The shell snapshot for `user_id`: its Agent Code threads that are not deleted, newest first, and the change number.
///
/// # Errors
///
/// When a find fails.
pub fn snapshot(facade: &EntityFacade, user_id: &str) -> Result<serde_json::Value, Error> {
    // the number first: a change after it is sent on the stream, maybe also in the snapshot, which the client merges
    let sequence = crate::change::latest(facade)?;
    let conversations = facade
        .find(CONVERSATION)?
        .eq("userId", Value::string(user_id))
        .eq("surfaceEnumId", Value::string(SURFACE))
        .condition(Condition::not_equals("statusId", Value::string("AgcDeleted")))
        .order_by("createdDate", true)
        .limit(SNAPSHOT_LIMIT)
        .disable_authz()
        .list()?;
    let ids: Vec<String> = conversations
        .iter()
        .filter_map(|row| text_of(row, "conversationId"))
        .collect();
    let mut users: BTreeMap<String, EntityValue> = BTreeMap::new();
    for chunk in ids.chunks(500) {
        let rows = facade
            .find(CONVERSATION_USER)?
            .condition(Condition::is_in(
                "conversationId",
                chunk.iter().map(Value::string).collect(),
            ))
            .eq("userId", Value::string(user_id))
            .disable_authz()
            .list()?;
        for row in rows.into_values() {
            if let Some(id) = text_of(&row, "conversationId") {
                users.insert(id, row);
            }
        }
    }
    let tags = tags_of(facade, user_id, &ids)?;
    let threads: Vec<serde_json::Value> = conversations
        .iter()
        .map(|row| {
            let id = text_of(row, "conversationId").unwrap_or_default();
            thread_summary(
                row,
                users.get(&id),
                tags.get(&id).map(Vec::as_slice).unwrap_or_default(),
            )
        })
        .collect();
    Ok(json!({
        "sequence": sequence,
        "threads": threads,
        "projects": [],
    }))
}
