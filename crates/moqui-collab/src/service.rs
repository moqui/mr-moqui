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

//! `org.moqui.resource.CollabServices`: a co-edited document (the crate root) saved as a version of its record now, and
//! its open state, for the editor's screen.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, save_document)?;
    rust_service!(map, save_meeting_board)?;
    rust_service!(map, get_document)?;
    rust_service!(map, recheck_meeting_boards)?;
    Ok(())
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

/// `save#CollabDocument`: save the document as a version of its record, as the current user. Out: `versionName`.
fn save_document(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(key) = text(parameters, "documentKey") else {
        ExecutionContext::with_current(|ec| ec.message().add_error("documentKey is required"));
        return Ok(out);
    };
    match crate::save(&key, text(parameters, "versionNote").as_deref()) {
        Ok(version) => {
            out.insert("versionName".to_string(), version.map_or(Value::Null, Value::string));
        }
        Err(refusal) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(refusal.message));
        }
    }
    Ok(out)
}

/// The call engine finalizes an open board before it closes the call's board rows.
fn save_meeting_board(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let call = text(parameters, "callSessionId").unwrap_or_default();
    let page = text(parameters, "wikiPageId").unwrap_or_default();
    let key = format!("WikiPage:{page}");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let board = facade
        .find("moqui.call.CallBoard")?
        .eq("callSessionId", Value::string(&call))
        .eq("wikiPageId", Value::string(&page))
        .one()?;
    if board.is_none() {
        return Err(LibError::new("board does not belong to this call"));
    }
    let session = facade
        .find("moqui.call.CallSession")?
        .eq("callSessionId", Value::string(&call))
        .one()?;
    let writer = session
        .as_ref()
        .and_then(|s| s.get("startedByUserId"))
        .map(to_plain_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| LibError::new("no host to save the meeting board"))?;
    let note = meeting_note(facade, session.as_ref());
    let result = (|| -> Result<Option<String>, crate::Refusal> {
        let room = match crate::room_of(&key) {
            Some(room) => room,
            None => {
                let Some(state) = crate::stored(&key)? else {
                    return Ok(None);
                };
                if state.get("editorUserIds").is_none_or(is_empty) {
                    return Ok(None);
                }
                let access = crate::access(&page, &writer)?;
                if !access.can_edit {
                    return Err(crate::Refusal::new(403, "host cannot save this board"));
                }
                crate::load(&key, &page, access.on_database)?
            }
        };
        if crate::lock(&room).unsaved.is_empty() {
            return Ok(None);
        }
        crate::save_room(&key, &room, &writer, Some(&note)).map_err(Into::into)
    })();
    let version = result.map_err(|err| LibError::new(err.message))?;
    Ok(BTreeMap::from([(
        "versionName".into(),
        version.map_or(Value::Null, Value::string),
    )]))
}

/// The version note of a board saved as its meeting ends: the meeting by its link's or its room's name, in the
/// session's language.
fn meeting_note(facade: &EntityFacade, session: Option<&moqui_core::entity::EntityValue>) -> String {
    let named = |entity: &str, key: &str, field: &str| {
        let id = session?.get(key).map(to_plain_string).filter(|id| !id.is_empty())?;
        let row = facade.find(entity).ok()?.eq(key, Value::string(&id)).one().ok()??;
        row.get(field)
            .map(to_plain_string)
            .filter(|name| !name.trim().is_empty())
    };
    let localize = |text: &str| moqui_core::l10n::localize(text).unwrap_or_else(|_| text.to_string());
    match named("moqui.call.CallLink", "callLinkId", "name")
        .or_else(|| named("moqui.chat.ChatRoom", "chatRoomId", "roomName"))
    {
        Some(name) => localize("Meeting ${name} ended").replace("${name}", name.trim()),
        None => localize("Meeting ended"),
    }
}

/// `recheck#MeetingBoards`: once the caller's transaction commits (at once outside one), every server asks again the
/// meeting-board grants of the call (`callSessionId`), or of each call of the room (`chatRoomId`) not ended, at its
/// next co-editing tick ([`moqui_core::notify::COLLAB_RECHECK`]). A call is announced once per transaction on this
/// thread. Nothing is refused: a lost announcement only waits for the regular recheck.
fn recheck_meeting_boards(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut calls: Vec<String> = text(parameters, "callSessionId").into_iter().collect();
    if let Some(room) = text(parameters, "chatRoomId") {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let sessions = facade
            .find("moqui.call.CallSession")?
            .eq("chatRoomId", Value::string(&room))
            .select_field("callSessionId,statusId")
            .list()?;
        calls.extend(
            sessions
                .iter()
                .filter(|row| row.get("statusId").map(to_plain_string).as_deref() != Some("CssEnded"))
                .filter_map(|row| row.get("callSessionId").map(to_plain_string)),
        );
    }
    for call in calls {
        announce_after_commit(facade, call);
    }
    Ok(BTreeMap::new())
}

thread_local! {
    /// The calls to announce when this thread's transaction commits, so a call is announced once per transaction.
    static PENDING: std::cell::RefCell<std::collections::BTreeSet<String>> = const {
        std::cell::RefCell::new(std::collections::BTreeSet::new())
    };
}

fn announce_after_commit(facade: &EntityFacade, call: String) {
    let send = {
        let facade = facade.clone();
        let call = call.clone();
        move || {
            let body = serde_json::json!({"callSessionId": call});
            let sent = moqui_core::cluster::prepare_message(body).and_then(|message| {
                moqui_core::cluster::publish(&facade, moqui_core::notify::COLLAB_RECHECK, &message)
            });
            if let Err(err) = sent {
                tracing::warn!(
                    "co-editing: the meeting boards of call {call} were not announced: {}",
                    err.message()
                );
            }
        }
    };
    let tx = match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => tx,
        _ => return send(),
    };
    if !PENDING.with(|pending| pending.borrow_mut().insert(call.clone())) {
        return;
    }
    let forget = |call: String| {
        move || {
            PENDING.with(|pending| pending.borrow_mut().remove(&call));
        }
    };
    let on_commit = {
        let forget = forget(call.clone());
        move || {
            forget();
            send();
        }
    };
    if let Err(err) = tx
        .on_commit(on_commit)
        .and_then(|()| tx.on_rollback(forget(call.clone())))
    {
        tracing::warn!("co-editing: the meeting boards of call {call} were not announced after the commit: {err}");
        forget(call)();
    }
}

/// `get#CollabDocument`: the stored state of a document the current user may view: its markdown, who changed it last
/// and when, and whose changes are in no version yet.
fn get_document(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(key) = text(parameters, "documentKey") else {
        ExecutionContext::with_current(|ec| ec.message().add_error("documentKey is required"));
        return Ok(out);
    };
    if let Err(refusal) = crate::check_view(&key) {
        ExecutionContext::with_current(|ec| ec.message().add_error(refusal.message));
        return Ok(out);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find("moqui.resource.CollabDocument")?
        .eq("documentKey", Value::string(&key))
        .one()?;
    let open = crate::snapshot(&key);
    let active = crate::room_of(&key)
        .map(|room| {
            crate::lock(&room)
                .can_edit
                .iter()
                .filter(|(_, grant)| grant.can_edit && grant.clients > 0)
                .filter_map(|(user, grant)| {
                    let user = grant.meeting.as_ref().map_or(user, |scope| &scope.user_id);
                    (!user.is_empty()).then_some(user)
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(Value::string)
                .collect()
        })
        .unwrap_or_default();
    out.insert("activeEditorIdList".to_string(), Value::List(active));
    let field = |name: &str| {
        row.as_ref()
            .and_then(|row| row.get(name).cloned())
            .unwrap_or(Value::Null)
    };
    out.insert("exists".to_string(), Value::Bool(row.is_some()));
    out.insert("isOpen".to_string(), Value::Bool(open.is_some()));
    out.insert(
        "snapshotText".to_string(),
        open.map_or_else(|| field("snapshotText"), Value::string),
    );
    let editors: Vec<Value> = to_plain_string(&field("editorUserIds"))
        .split(',')
        .filter(|id| !id.trim().is_empty())
        .map(|id| Value::string(id.trim()))
        .collect();
    out.insert("unsavedUserIdList".to_string(), Value::List(editors));
    for name in ["lastChangeDate", "lastChangeUserId", "lastSaveDate"] {
        out.insert(name.to_string(), field(name));
    }
    Ok(out)
}
