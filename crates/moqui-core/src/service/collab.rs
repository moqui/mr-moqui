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

//! `org.moqui.impl.CollabServices`: a co-edited document (`crate::collab`) saved as a version of its record now, and
//! its open state, for the editor's screen.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, save_document)?;
    rust_service!(map, get_document)?;
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
    match crate::collab::save(&key, text(parameters, "versionNote").as_deref()) {
        Ok(version) => {
            out.insert("versionName".to_string(), version.map_or(Value::Null, Value::string));
        }
        Err(refusal) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(refusal.message));
        }
    }
    Ok(out)
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
    if let Err(refusal) = crate::collab::check_view(&key) {
        ExecutionContext::with_current(|ec| ec.message().add_error(refusal.message));
        return Ok(out);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find("moqui.resource.CollabDocument")?
        .eq("documentKey", Value::string(&key))
        .one()?;
    let open = crate::collab::snapshot(&key);
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
