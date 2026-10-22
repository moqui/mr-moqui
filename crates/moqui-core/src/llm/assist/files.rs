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

//! The composer's files (plans/LLM-A2A-Assist.md 4.11; docs/Assist.md `POST attachments`): a file the user attaches is
//! checked (the conf's `assist` size, count and media types; never a program) and stored as a DbResource file at
//! `dbresource://agent/files/<userId>/<artifactId>/<name>` (the system property `agent_attachment_root` names another
//! root), with an `AgentArtifact` of kind `file` whose part keeps its name, type, size, location and owner. A turn
//! names it (`attachments`): it must be the user's and not another conversation's; it becomes a `file` part of the
//! user's message, which the model reads as a line naming it, and the agent reads a text file's lines with
//! `data artifact=F<id>` ([`read_text`]), only within its conversation. An image or a PDF is named, not read.

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{Answer, Caller, DrawerError, Params};
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::run::{in_tx, now_millis, quietly, stamp};
use crate::llm::text_of;
use crate::web::multipart::UploadedFile;

const ARTIFACT: &str = "moqui.agent.AgentArtifact";
const ARTIFACT_PART: &str = "moqui.agent.AgentArtifactPart";
/// The longest file name kept.
const NAME_CHARS: usize = 200;
/// The text of a file the agent reads, at most.
pub const READ_CHARS: usize = 2_000_000;

/// Where the files of the composer go.
fn root() -> String {
    std::env::var("agent_attachment_root")
        .ok()
        .filter(|root| !root.trim().is_empty())
        .unwrap_or_else(|| "dbresource://agent/files".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The name the model reads for an attachment.
pub fn attachment_ref(artifact_id: &str) -> String {
    format!("F{artifact_id}")
}

/// Whether the agent can read a file of `media_type` as text.
pub fn readable(media_type: &str) -> bool {
    let media_type = media_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    media_type.starts_with("text/")
        || ["application/json", "application/xml", "application/csv", "application/x-yaml", "application/yaml"]
            .contains(&media_type.as_str())
}

/// `1234567` bytes as `1,234,567`.
fn bytes_text(size: u64) -> String {
    crate::llm::tools::spill::grouped(usize::try_from(size).unwrap_or(usize::MAX))
}

/// The line the model reads for a file part of a user message.
pub fn attached_line(data: &Json) -> String {
    let field = |key: &str| data.get(key).and_then(Json::as_str).unwrap_or_default();
    let reference = field("ref");
    let size = data.get("size").and_then(Json::as_u64).unwrap_or(0);
    let what = format!("{} ({}, {} bytes)", field("fileName"), field("mediaType"), bytes_text(size));
    if data.get("readable").and_then(Json::as_bool).unwrap_or(false) {
        format!("[attached {reference}: {what}; read it with data artifact={reference}]")
    } else {
        format!("[attached {reference}: {what}; its content cannot be read as text]")
    }
}

/// A file name as kept: the last segment, no control characters, at most [`NAME_CHARS`].
fn clean_name(name: &str) -> String {
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return "file".to_string();
    }
    let count = name.chars().count();
    name.chars().skip(count.saturating_sub(NAME_CHARS)).collect()
}

/// The uploaded file of the request's field `name` (or of the call's parameters, as a test passes it).
pub(super) fn upload_of(params: &Params, name: &str) -> Option<Arc<UploadedFile>> {
    let of = |value: &Value| match value {
        Value::Object(object) => object.downcast::<UploadedFile>(),
        Value::List(items) => items.iter().find_map(|item| match item {
            Value::Object(object) => object.downcast::<UploadedFile>(),
            _ => None,
        }),
        _ => None,
    };
    crate::web::multipart_parameters()
        .get(name)
        .and_then(of)
        .or_else(|| params.raw(name).and_then(of))
}

/// `POST attachments`: store the request's `file` for the user (and the conversation `conversationId`, when given and
/// theirs). Out: `{attachmentId, ref, fileName, mediaType, size, readable}`.
pub(super) fn attachments(caller: &Caller, params: &Params) -> Answer {
    let conf = crate::llm::facade(&caller.factory).conf().assist.clone();
    let Some(file) = upload_of(params, "file") else {
        return Err(DrawerError::invalid("file is required: a multipart form with one file"));
    };
    let conversation_id = match params.text("conversationId") {
        Some(id) => Some(text_of(&caller.conversation(&id)?, "conversationId").unwrap_or(id)),
        None => None,
    };
    let media_type = if file.content_type.trim().is_empty() {
        "application/octet-stream".to_string()
    } else {
        file.content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase()
    };
    if file.size() > conf.max_attachment_bytes {
        return Err(DrawerError::invalid(format!(
            "the file is {} bytes, over the {} bytes a file may have",
            bytes_text(file.size()),
            bytes_text(conf.max_attachment_bytes)
        )));
    }
    if file.size() == 0 {
        return Err(DrawerError::invalid("the file is empty"));
    }
    if !conf.allows_type(&media_type) {
        return Err(DrawerError::invalid(format!(
            "a file of type {media_type} cannot be attached; these can: {}",
            conf.attachment_types.join(", ")
        )));
    }
    if file.is_executable() {
        return Err(DrawerError::invalid("a program cannot be attached"));
    }
    let bytes = file.read_bytes()?;
    let name = clean_name(&file.file_name);
    let facade = &caller.facade;
    let stored = quietly(|| {
        in_tx(facade, || -> Result<Json, Error> {
            let now = now_millis();
            let mut row = facade.make_value(ARTIFACT)?;
            row.set_sequenced_id_primary(facade)?;
            let artifact_id = text_of(&row, "artifactId").unwrap_or_default();
            let location = format!("{}/{}/{artifact_id}/{name}", root(), caller.user_id);
            facade.write_db_resource(&location, &bytes)?;
            row.set("artifactKind", Value::string("file"));
            row.set("artifactName", Value::string(&name));
            if let Some(id) = &conversation_id {
                row.set("conversationId", Value::string(id));
            }
            row.set("versionNum", Value::I64(1));
            row.set("createdDate", stamp(now));
            row.set("lastUpdatedDate", stamp(now));
            row.create(facade)?;
            let data = json!({
                "attachmentId": artifact_id,
                "ref": attachment_ref(&artifact_id),
                "fileName": name,
                "mediaType": media_type,
                "size": bytes.len(),
                "readable": readable(&media_type),
                "location": location,
                "userId": caller.user_id,
                "sha256": crate::resource::blob::blob_id(&bytes),
            });
            let mut part = facade.make_value(ARTIFACT_PART)?;
            part.set("artifactId", Value::string(&artifact_id));
            part.set("partSeq", Value::I64(1));
            part.set("partKindEnumId", Value::string("AgpkFile"));
            part.set("dataJson", Value::string(data.to_string()));
            part.set("mediaType", Value::string(&media_type));
            part.set("fileName", Value::string(&name));
            part.set("fileLocation", Value::string(&location));
            part.create(facade)?;
            Ok(data)
        })
    })?;
    Ok(public(&stored))
}

/// What the client gets of an attachment: no location, no owner.
fn public(data: &Json) -> Json {
    json!({
        "attachmentId": data.get("attachmentId"),
        "ref": data.get("ref"),
        "fileName": data.get("fileName"),
        "mediaType": data.get("mediaType"),
        "size": data.get("size"),
        "readable": data.get("readable"),
    })
}

/// The file artifact `artifact_id` with its part's data, when it is one.
fn load(facade: &EntityFacade, artifact_id: &str) -> Result<Option<(EntityValue, Json)>, Error> {
    let Some(row) = facade
        .find(ARTIFACT)?
        .eq("artifactId", Value::string(artifact_id))
        .disable_authz()
        .one()?
        .filter(|row| text_of(row, "artifactKind").as_deref() == Some("file"))
    else {
        return Ok(None);
    };
    let data: Json = facade
        .find(ARTIFACT_PART)?
        .eq("artifactId", Value::string(artifact_id))
        .eq("partSeq", Value::I64(1))
        .disable_authz()
        .one()?
        .and_then(|part| text_of(&part, "dataJson"))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Json::Null);
    Ok(Some((row, data)))
}

/// The attachments `ids` of a turn: each the user's and not another conversation's than `conversation_id`, at most
/// the conf's count. Out: the data of their `file` parts.
///
/// # Errors
///
/// 400 for too many, 404 for one that is not the user's or is another conversation's.
pub(super) fn for_turn(caller: &Caller, ids: &[String], conversation_id: Option<&str>) -> Result<Vec<Json>, DrawerError> {
    let max = crate::llm::facade(&caller.factory).conf().assist.max_attachments;
    if ids.len() > usize::try_from(max).unwrap_or(usize::MAX) {
        return Err(DrawerError::invalid(format!("a message carries at most {max} files")));
    }
    let mut out = Vec::new();
    for id in ids {
        let id = id.trim().trim_start_matches('F');
        let (row, data) = quietly(|| load(&caller.facade, id))?.ok_or_else(|| DrawerError::not_found("attachment", id))?;
        let mine = data.get("userId").and_then(Json::as_str) == Some(caller.user_id.as_str());
        let bound = text_of(&row, "conversationId");
        if !mine || bound.as_deref().is_some_and(|bound| Some(bound) != conversation_id) {
            return Err(DrawerError::not_found("attachment", id));
        }
        let mut part = data.clone();
        if let Some(object) = part.as_object_mut() {
            object.remove("userId");
            object.remove("sha256");
        }
        out.push(part);
    }
    Ok(out)
}

/// Bind the attachments of a turn to its conversation (a new conversation's id is known only once its run is made).
/// A failure is logged: the message has them.
pub(super) fn bind(caller: &Caller, parts: &[Json], conversation_id: &str) {
    for part in parts {
        let Some(id) = part.get("attachmentId").and_then(Json::as_str) else {
            continue;
        };
        let bound = quietly(|| {
            in_tx(&caller.facade, || {
                caller
                    .facade
                    .find(ARTIFACT)?
                    .eq("artifactId", Value::string(id))
                    .disable_authz()
                    .update_all([("conversationId".to_string(), Value::string(conversation_id))])
            })
        });
        if let Err(error) = bound {
            tracing::warn!("attachment {id} was not bound to agent conversation {conversation_id}: {error}");
        }
    }
}

/// `GET attachment`: the file `attachmentId` of the user's, sent as a download.
pub(super) fn download(caller: &Caller, params: &Params) -> Result<(), DrawerError> {
    let id = params.required("attachmentId")?;
    let id = id.trim_start_matches('F').to_string();
    let (_, data) = quietly(|| load(&caller.facade, &id))?.ok_or_else(|| DrawerError::not_found("attachment", &id))?;
    if data.get("userId").and_then(Json::as_str) != Some(caller.user_id.as_str()) {
        return Err(DrawerError::not_found("attachment", &id));
    }
    let location = data.get("location").and_then(Json::as_str).unwrap_or_default();
    let bytes = quietly(|| caller.facade.read_db_resource_bytes(location))?
        .ok_or_else(|| DrawerError::not_found("attachment", &id))?;
    let name = data.get("fileName").and_then(Json::as_str).unwrap_or("file");
    let ascii: String = name.chars().map(|ch| if ch.is_ascii_graphic() && ch != '"' && ch != '\\' || ch == ' ' { ch } else { '_' }).collect();
    let media_type = data.get("mediaType").and_then(Json::as_str).unwrap_or("application/octet-stream");
    crate::web::send_bytes_with_headers(
        200,
        vec![
            ("Content-Type".to_string(), media_type.to_string()),
            ("Content-Disposition".to_string(), format!("attachment; filename=\"{ascii}\"")),
            ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
            ("Cache-Control".to_string(), "private, no-store".to_string()),
        ],
        bytes,
    );
    Ok(())
}

/// The text of the attachment `name` (`F<id>`) of the conversation `conversation_id`: `None` when it is not a file of
/// that conversation; an error the model reads when it cannot be read as text.
///
/// # Errors
///
/// Returns why it is not text, or a failed read.
pub fn read_text(facade: &EntityFacade, conversation_id: &str, name: &str) -> Result<Option<String>, Error> {
    let Some(id) = name.trim().strip_prefix('F').or_else(|| name.trim().strip_prefix('f')) else {
        return Ok(None);
    };
    let Some((row, data)) = load(facade, id)? else {
        return Ok(None);
    };
    if text_of(&row, "conversationId").as_deref() != Some(conversation_id) {
        return Ok(None);
    }
    if !data.get("readable").and_then(Json::as_bool).unwrap_or(false) {
        return Err(Error::new(format!(
            "{name} is a {} file; its content cannot be read as text",
            data.get("mediaType").and_then(Json::as_str).unwrap_or("binary")
        )));
    }
    let location = data.get("location").and_then(Json::as_str).unwrap_or_default();
    Ok(facade.read_db_resource_bytes(location)?.map(|bytes| {
        let text = String::from_utf8_lossy(&bytes);
        text.chars().take(READ_CHARS).collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_types_and_lines() {
        assert_eq!(clean_name("../notes/plan.txt"), "plan.txt");
        assert_eq!(clean_name("..\\x\\a\u{7}b.csv"), "ab.csv");
        assert_eq!(clean_name(".."), "file");
        assert!(readable("text/csv; charset=utf-8") && readable("application/json") && !readable("image/png"));
        let conf = crate::llm::conf::AssistConf::default();
        assert!(conf.allows_type("text/markdown") && conf.allows_type("image/png") && !conf.allows_type("application/zip"));
        let line = attached_line(&json!({"ref": "F12", "fileName": "a.csv", "mediaType": "text/csv", "size": 12345, "readable": true}));
        assert_eq!(line, "[attached F12: a.csv (text/csv, 12,345 bytes); read it with data artifact=F12]");
        let image = attached_line(&json!({"ref": "F13", "fileName": "p.png", "mediaType": "image/png", "size": 9, "readable": false}));
        assert!(image.ends_with("its content cannot be read as text]"), "{image}");
    }
}
