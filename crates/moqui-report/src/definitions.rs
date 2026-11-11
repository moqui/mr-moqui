//! Versioned saved objects and component definitions. Invalid independent files do not hide valid ones.
use super::{fail, input, json, output, text, Error, ExecutionContext, Json, Parameters, ServiceInfo, Value};
use moqui_lib::sync;
use std::collections::{BTreeSet, HashMap};
const ENTITY: &str = "moqui.report.Definition";
const VERSION: &str = "moqui.report.DefinitionVersion";
const REFERENCE: &str = "moqui.report.DefinitionReference";
#[derive(Default)]
struct Shipped {
    definitions: std::sync::RwLock<Option<std::sync::Arc<[Json]>>>,
}

/// Load component files at boot, and again after a component is loaded on a running server. A bad independent file is
/// logged and skipped; named duplicates are deterministic and never replace the earlier component's definition.
pub fn load_shipped(factory: &moqui_core::context::ExecutionContextFactory) -> Result<usize, Error> {
    let registry = factory.extension(|_| Shipped::default());
    let definitions = scan_shipped(factory);
    let count = definitions.len();
    *sync::write(&registry.definitions) = Some(definitions.into());
    Ok(count)
}

fn entity(ec: &ExecutionContext) -> Result<moqui_core::entity::EntityFacade, Error> {
    ec.factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))
}
fn internal<T>(ec: &ExecutionContext, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _guard = ec.disable_authz();
    body()
}
fn viewer() -> Result<String, Error> {
    moqui_core::user::current_user_id()
        .map_err(fail)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("saved definitions require an authenticated viewer"))
}

pub(super) fn readable(definition: &Json) -> Result<bool, Error> {
    if definition["readOnly"] == true {
        return Ok(true);
    }
    let current = moqui_core::user::current_user_id().map_err(fail)?.unwrap_or_default();
    if text(definition, "ownerId", "") == current && !current.is_empty() {
        return Ok(true);
    }
    let group = text(definition, "userGroupId", "");
    Ok(!group.is_empty() && moqui_core::user::is_in_group(group).map_err(fail)?)
}
fn visible(ec: &ExecutionContext, mut definition: Json) -> Result<Json, Error> {
    let viewer = moqui_core::user::current_user_id().map_err(fail)?.unwrap_or_default();
    if text(&definition, "ownerId", "") != viewer || viewer.is_empty() {
        let kind = definition["type"].clone();
        super::alert::redact_definition_credentials(&kind, &mut definition["body"]);
    }
    let _ = ec;
    Ok(definition)
}

fn shipped(ec: &ExecutionContext) -> std::sync::Arc<[Json]> {
    let registry = ec.factory().extension(|_| Shipped::default());
    {
        if let Some(definitions) = sync::read(&registry.definitions).as_ref() {
            return definitions.clone();
        }
    }
    let mut state = sync::write(&registry.definitions);
    state.get_or_insert_with(|| scan_shipped(ec.factory()).into()).clone()
}

fn scan_shipped(factory: &moqui_core::context::ExecutionContextFactory) -> Vec<Json> {
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    for component in factory.components().iter() {
        let directory = component.location.join("report");
        if !directory.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::error!(path=%directory.display(),%error,"could not read report definitions");
                continue;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    tracing::error!(path=%directory.display(),%error,"could not read report definition entry");
                    continue;
                }
            };
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            paths.push(path);
        }
        paths.sort();
        for path in paths {
            let result = (|| -> Result<Json, Error> {
                let bytes = std::fs::read(&path).map_err(fail)?;
                if bytes.len() > 2_000_000 {
                    return Err(fail("shipped definition exceeds two megabytes"));
                }
                let mut definition: Json = serde_json::from_slice(&bytes).map_err(fail)?;
                if !definition.is_object()
                    || text(&definition, "id", "").is_empty()
                    || text(&definition, "type", "").is_empty()
                {
                    return Err(fail("report definition requires id and type"));
                }
                if definition.get("body").is_none() {
                    let body = definition.clone();
                    definition["body"] = body;
                }
                super::alert::validate_shared(text(&definition, "type", ""), &definition["body"])?;
                if definition["type"] == "alert" {
                    super::alert::validate_contacts(&definition["body"])?;
                    // The evaluator reads only saved rules, which run as their owner.
                    tracing::info!(file=%path.display(),definition=%text(&definition,"id",""),"shipped alert rules are templates and are not evaluated; save a copy to enable one");
                }
                definition["readOnly"] = json!(true);
                definition["component"] = json!(component.name);
                definition["version"] = json!(0);
                Ok(definition)
            })();
            match result {
                Ok(definition) => {
                    let id = text(&definition, "id", "").to_owned();
                    if ids.insert(id.clone()) {
                        out.push(definition);
                    } else {
                        tracing::error!(file=%path.display(),definition=%id,"duplicate report definition, skipping");
                    }
                }
                Err(error) => tracing::error!(file=%path.display(),%error,"invalid report definition, skipping"),
            }
        }
    }
    out
}

pub(super) fn row_json(row: &moqui_core::entity::EntityValue) -> Result<Json, Error> {
    let body = row.get("bodyJson").and_then(Value::as_str).unwrap_or("{}");
    Ok(
        json!({"id":row.get("definitionId").map(moqui_lib::object::to_plain_string),"type":row.get("definitionType").map(moqui_lib::object::to_plain_string),"title":row.get("title").map(moqui_lib::object::to_plain_string),"body":serde_json::from_str::<Json>(body).map_err(fail)?,"ownerId":row.get("ownerId").map(moqui_lib::object::to_plain_string),"userGroupId":row.get("userGroupId").and_then(Value::as_str).filter(|s|!s.is_empty()),"component":row.get("component").map(moqui_lib::object::to_plain_string),"readOnly":row.get("readOnly").and_then(Value::as_str)==Some("Y"),"version":row.get("versionNumber").map(moqui_lib::json::to_json)}),
    )
}

pub fn read(ec: &ExecutionContext, id: &str) -> Result<Json, Error> {
    read_optional(ec, id)?.ok_or_else(|| fail(format!("report definition {id} does not exist")))
}

/// A definition the viewer may read, or `None` when no shipped or saved definition has the id.
pub fn read_optional(ec: &ExecutionContext, id: &str) -> Result<Option<Json>, Error> {
    if let Some(definition) = shipped(ec).iter().find(|d| text(d, "id", "") == id) {
        return visible(ec, definition.clone()).map(Some);
    }
    let Some(row) = internal(ec, || {
        entity(ec)?.find(ENTITY)?.eq("definitionId", Value::string(id)).one()
    })?
    else {
        return Ok(None);
    };
    let definition = row_json(&row)?;
    if !readable(&definition)? {
        return Err(fail(format!("viewer may not read definition {id}")));
    }
    visible(ec, definition).map(Some)
}

/// A definition whoever asks, for the engine's own questions about a schedule (delivery.rs: whether it reads a business
/// dataset, its version). Never returned to a caller.
pub(super) fn read_unchecked(ec: &ExecutionContext, id: &str) -> Result<Option<Json>, Error> {
    if let Some(definition) = shipped(ec).iter().find(|d| text(d, "id", "") == id) {
        return Ok(Some(definition.clone()));
    }
    internal(ec, || {
        entity(ec)?.find(ENTITY)?.eq("definitionId", Value::string(id)).one()
    })?
    .map(|row| row_json(&row))
    .transpose()
}

pub fn list(ec: &ExecutionContext, kind: Option<&str>) -> Result<Vec<Json>, Error> {
    let mut definitions = shipped(ec)
        .iter()
        .map(|definition| visible(ec, definition.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let rows = internal(ec, || entity(ec)?.find(ENTITY)?.list())?;
    for row in rows.iter() {
        let definition = match row_json(row) {
            Ok(definition) => definition,
            Err(error) => {
                tracing::error!(definition=?row.get("definitionId"),%error,"invalid independent report definition, continuing");
                continue;
            }
        };
        if readable(&definition)? {
            definitions.push(visible(ec, definition)?);
        }
    }
    definitions.retain(|d| kind.is_none_or(|kind| text(d, "type", "") == kind));
    definitions.sort_by(|a, b| text(a, "title", "").cmp(text(b, "title", "")));
    Ok(definitions)
}

/// A rule body key that lists the ids of shared contact points, silences or mute timings.
fn is_ref_list(key: &str) -> bool {
    super::alert::SHARED.iter().any(|(_, refs, _)| *refs == key)
}

fn references(value: &Json, out: &mut BTreeSet<String>) {
    match value {
        Json::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "definitionId" | "panelRef" | "datasetId" | "dashboardRef") {
                    if let Some(id) = value.as_str() {
                        if !moqui_core::search::telemetry::is_telemetry_index(id) {
                            out.insert(id.into());
                        }
                    }
                } else if is_ref_list(key) {
                    out.extend(
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Json::as_str)
                            .map(str::to_owned),
                    );
                    continue;
                }
                references(value, out);
            }
        }
        Json::Array(items) => {
            for item in items {
                references(item, out)
            }
        }
        _ => {}
    }
}

/// Save a definition read from a synced git file, as the sync's user.
pub(super) fn save_pulled(ec: &ExecutionContext, definition: &Json, note: &str) -> Result<Json, Error> {
    // A pulled business alert rule that is not already active here notifies nobody until its owner activates it, as a
    // copy and an import (alert_delivery.rs): a file does not start delivery.
    if definition["type"] == "alert" {
        let active =
            read_unchecked(ec, text(definition, "id", ""))?.is_some_and(|stored| stored["body"]["paused"] != true);
        if !active {
            let mut definition = definition.clone();
            super::alert_delivery::prepare_copy(ec, &mut definition["body"]);
            return save(ec, &definition, note);
        }
    }
    save(ec, definition, note)
}
/// Save a definition as the viewer, with the owner, reference and dataset-save checks of `save#Definition`: a business
/// dataset made or revised by its steward (business.rs), whose operation decides its admission itself.
pub(super) fn save_owned(ec: &ExecutionContext, definition: &Json, note: &str) -> Result<Json, Error> {
    save_definition_as(ec, definition, note, true)
}

/// The documents of every business dataset, shipped or saved, whoever may read it (business.rs `check_governed`). Reads with
/// authorization off and returns only document ids.
pub(super) fn business_documents(ec: &ExecutionContext) -> Result<BTreeSet<String>, Error> {
    Ok(business_datasets(ec)?
        .into_iter()
        .filter_map(|(_, body)| body["documentType"].as_str().map(str::to_owned))
        .collect())
}
/// Every business dataset, shipped or saved, whoever may read it, as its id and body (business.rs). Reads with authorization
/// off and returns no owner or group.
pub(super) fn business_datasets(ec: &ExecutionContext) -> Result<Vec<(String, Json)>, Error> {
    let mut out = Vec::new();
    let mut add = |definition: &Json| {
        if definition["type"] == "dataset" && super::business::is_business(&definition["body"]) {
            out.push((text(definition, "id", "").to_owned(), definition["body"].clone()));
        }
    };
    shipped(ec).iter().for_each(&mut add);
    let saved = internal(ec, || {
        entity(ec)?
            .find(ENTITY)?
            .eq("definitionType", Value::string("dataset"))
            .list()
    })?;
    for row in saved.iter() {
        match row_json(row) {
            Ok(definition) => add(&definition),
            Err(error) => tracing::error!(%error, "invalid report definition, continuing"),
        }
    }
    Ok(out)
}

/// Every definition that references `id`, shipped or saved, whoever may read it, as its id and body: what a change to a
/// business dataset's fields would break (business.rs). Reads with authorization off and returns no owner or group.
pub(super) fn all_referencing(ec: &ExecutionContext, id: &str) -> Result<Vec<(String, Json)>, Error> {
    let mut out = Vec::new();
    for definition in shipped(ec).iter() {
        let mut refs = BTreeSet::new();
        references(&definition["body"], &mut refs);
        if refs.contains(id) {
            out.push((text(definition, "id", "").to_owned(), definition["body"].clone()));
        }
    }
    let referencing = internal(ec, || {
        entity(ec)?
            .find(REFERENCE)?
            .eq("referencedId", Value::string(id))
            .list()
    })?;
    for reference in referencing.iter() {
        let Some(definition_id) = reference.get("definitionId").and_then(Value::as_str) else {
            continue;
        };
        let row = internal(ec, || {
            entity(ec)?
                .find(ENTITY)?
                .eq("definitionId", Value::string(definition_id))
                .one()
        })?;
        if let Some(row) = row {
            match row_json(&row) {
                Ok(definition) => out.push((definition_id.to_owned(), definition["body"].clone())),
                Err(error) => {
                    tracing::error!(definition = definition_id, %error, "invalid report definition, continuing")
                }
            }
        }
    }
    Ok(out)
}

fn save(ec: &ExecutionContext, definition: &Json, note: &str) -> Result<Json, Error> {
    save_definition_as(ec, definition, note, false)
}

/// The engine's own revision of a saved definition, whoever asks: an alert rule paused for its owner's sake or handed to a
/// new owner (alert_delivery.rs). `change` edits the body and may name a new owner; the row gains a version with `note`
/// by `actor`. Never a caller's edit: the caller's authority is checked by the operation that calls this.
pub(super) fn engine_revise(
    ec: &ExecutionContext,
    id: &str,
    note: &str,
    actor: &str,
    change: impl FnOnce(&mut Json) -> Option<String>,
) -> Result<Json, Error> {
    let facade = entity(ec)?;
    let _storage = ec.disable_authz();
    let mut row = facade
        .find(ENTITY)?
        .eq("definitionId", Value::string(id))
        .for_update(true)
        .one()?
        .ok_or_else(|| fail(format!("report definition {id} does not exist")))?;
    let mut definition = row_json(&row)?;
    let new_owner = change(&mut definition["body"]);
    let version = definition["version"].as_i64().unwrap_or(0) + 1;
    row.set("bodyJson", Value::String(definition["body"].to_string()));
    row.set("versionNumber", Value::I64(version));
    if let Some(owner) = new_owner {
        row.set("ownerId", Value::String(owner));
    }
    row.update(&facade)?;
    let mut history = facade.make_value(VERSION)?;
    let snapshot = json!({"title":definition["title"],"type":definition["type"],"body":definition["body"],
        "userGroupId":definition["userGroupId"]});
    for (name, value) in [
        ("definitionId", Value::string(id)),
        ("versionNumber", Value::I64(version)),
        ("bodyJson", Value::String(snapshot.to_string())),
        ("note", Value::string(note)),
        ("changedBy", Value::string(actor)),
        ("changedDate", moqui_core::user::now_value()),
    ] {
        history.set(name, value);
    }
    history.create(&facade)?;
    row_json(&row)
}

/// `owned`: a business dataset's own operation (business.rs), which decides its admission; any other save of a business dataset
/// whose body changed returns it to review.
fn save_definition_as(ec: &ExecutionContext, definition: &Json, note: &str, owned: bool) -> Result<Json, Error> {
    let current = viewer()?;
    let id = text(definition, "id", "");
    let kind = text(definition, "type", "");
    if id.is_empty() || kind.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c)) {
        return Err(fail(
            "definition id must contain letters, digits, _, -, . or :; type is required",
        ));
    }
    if !matches!(
        kind,
        "dashboard" | "panel" | "dataset" | "search" | "alert" | "report" | "contactPoint" | "silence" | "muteTiming"
    ) {
        return Err(fail(format!("unknown report definition type {kind}")));
    }
    if shipped(ec).iter().any(|d| text(d, "id", "") == id) {
        return Err(fail("shipped definition is read-only; save as copy"));
    }
    // Dataset/service definitions can control authority; viewers need an explicit administration grant.
    if kind == "dataset" {
        moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset", "update").map_err(fail)?;
    }
    let facade = entity(ec)?;
    let existing = internal(ec, || {
        facade
            .find(ENTITY)?
            .eq("definitionId", Value::string(id))
            .for_update(true)
            .one()
    })?;
    let previous = existing.as_ref().map(row_json).transpose()?;
    if let Some(previous) = &previous {
        if text(previous, "type", "") != kind {
            return Err(fail("a saved definition cannot change its type"));
        }
        if text(previous, "ownerId", "") != current {
            return Err(fail("only the owner may edit a report definition"));
        }
        if previous["readOnly"] == true {
            return Err(fail("definition is read-only"));
        }
        if let Some(expected) = definition["version"].as_i64() {
            if previous["version"].as_i64() != Some(expected) {
                return Err(fail("definition changed since it was read; reload before saving"));
            }
        }
    }
    let mut body = definition.get("body").cloned().unwrap_or_else(|| json!({}));
    // a business alert rule (alert_delivery.rs): its audience in place of contact points, the delivery mode the server's
    let business_rule = kind == "alert" && super::alert_delivery::prepare(ec, &mut body)?;
    let serialized = serde_json::to_string(&body).map_err(fail)?;
    if serialized.len() > 2_000_000 {
        return Err(fail("report definition exceeds two megabytes"));
    }
    let mut refs = BTreeSet::new();
    references(&body, &mut refs);
    for reference in &refs {
        if reference == id {
            return Err(fail("definition cannot reference itself"));
        }
        if read_optional(ec, reference)?.is_none() {
            super::dataset(ec, reference)?;
        }
        let mut pending = vec![reference.clone()];
        let mut visited = BTreeSet::new();
        while let Some(next) = pending.pop() {
            if next == id {
                return Err(fail("definition references would create a cycle"));
            }
            if !visited.insert(next.clone()) {
                continue;
            }
            if visited.len() > 1000 {
                return Err(fail("definition reference graph exceeds 1000 objects"));
            }
            let rows = internal(ec, || {
                facade.find(REFERENCE)?.eq("definitionId", Value::string(&next)).list()
            })?;
            for row in rows.iter() {
                if let Some(target) = row.get("referencedId").and_then(Value::as_str) {
                    pending.push(target.to_owned());
                }
            }
        }
    }
    let version = previous.as_ref().and_then(|p| p["version"].as_i64()).unwrap_or(0) + 1;
    if kind == "alert" {
        super::alert::validate_rule(ec, &body)?;
    }
    super::alert::validate_shared(kind, &body)?;
    if kind == "dataset" && !owned {
        super::business::definition_saved(ec, id, previous.as_ref().map(|previous| &previous["body"]), &body)?;
    }
    // Owner, group, reference and dataset-admin checks above run with the viewer's rights.
    // Only this storage operation bypasses entity auth; no broad CRUD grant is exposed.
    let storage = ec.disable_authz();
    let creating = existing.is_none();
    let mut row = existing.unwrap_or(facade.make_value(ENTITY)?);
    for (name, value) in [
        ("definitionId", Value::string(id)),
        ("definitionType", Value::string(kind)),
        ("title", Value::string(text(definition, "title", id))),
        ("bodyJson", Value::String(serialized.clone())),
        ("ownerId", Value::String(current.clone())),
        (
            "userGroupId",
            definition["userGroupId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map_or(Value::Null, Value::string),
        ),
        ("readOnly", Value::string("N")),
        ("versionNumber", Value::I64(version)),
    ] {
        row.set(name, value);
    }
    if creating {
        row.create(&facade)?;
    } else {
        row.update(&facade)?;
    }
    if kind == "alert"
        && facade
            .find("moqui.report.AlertRuleLock")?
            .eq("definitionId", Value::string(id))
            .one()?
            .is_none()
    {
        let mut rule_lock = facade.make_value("moqui.report.AlertRuleLock")?;
        rule_lock.set("definitionId", Value::string(id));
        rule_lock.create(&facade)?;
    }
    let mut history = facade.make_value(VERSION)?;
    let snapshot =
        json!({"title":text(definition,"title",id),"type":kind,"body":body,"userGroupId":definition["userGroupId"]});
    for (name, value) in [
        ("definitionId", Value::string(id)),
        ("versionNumber", Value::I64(version)),
        ("bodyJson", Value::String(snapshot.to_string())),
        ("note", Value::string(note)),
        ("changedBy", Value::String(current)),
        ("changedDate", moqui_core::user::now_value()),
    ] {
        history.set(name, value);
    }
    history.create(&facade)?;
    let old = facade.find(REFERENCE)?.eq("definitionId", Value::string(id)).list()?;
    for row in old.iter() {
        row.delete(&facade)?;
    }
    for reference in refs {
        let mut row = facade.make_value(REFERENCE)?;
        row.set("definitionId", Value::string(id));
        row.set("referencedId", Value::String(reference));
        row.create(&facade)?;
    }
    let saved = row_json(&row)?;
    drop(storage);
    if business_rule {
        // each recipient against the rule as it is now written, with the viewer's own rights; a refusal rolls the save back
        super::alert_delivery::check_saved(ec, &saved)?;
    }
    super::git_sync::after_save(ec, &saved);
    Ok(saved)
}

pub fn get_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    Ok(output(json!({"definition":read(ec,text(&params,"definitionId",""))?})))
}
pub fn list_definitions(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    Ok(output(
        json!({"definitions":list(ec,params["definitionType"].as_str())?}),
    ))
}

pub fn find_definition_uses(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let id = text(&params, "definitionId", "");
    read(ec, id)?;
    let uses=list(ec,None)?.into_iter().filter(|definition|{let mut references=BTreeSet::new();self::references(&definition["body"],&mut references);references.contains(id)}).map(|definition|json!({"id":definition["id"],"title":definition["title"],"type":definition["type"],"readOnly":definition["readOnly"]})).collect::<Vec<_>>();
    Ok(output(json!({"usedIn":uses})))
}

fn version_snapshot(ec: &ExecutionContext, current: &Json, version: i64) -> Result<Json, Error> {
    if version < 0 {
        return Err(fail("version numbers are nonnegative"));
    }
    if version == 0 {
        return Ok(if current["readOnly"] == true {
            json!({"title":current["title"],"type":current["type"],"body":current["body"]})
        } else {
            json!({})
        });
    }
    let row = internal(ec, || {
        entity(ec)?
            .find(VERSION)?
            .eq("definitionId", Value::string(text(current, "id", "")))
            .eq("versionNumber", Value::I64(version))
            .one()
    })?
    .ok_or_else(|| fail("definition version does not exist"))?;
    let mut snapshot: Json =
        serde_json::from_str(row.get("bodyJson").and_then(Value::as_str).unwrap_or("{}")).map_err(fail)?;
    if text(current, "ownerId", "") != moqui_core::user::current_user_id().map_err(fail)?.unwrap_or_default() {
        let kind = snapshot["type"].clone();
        super::alert::redact_definition_credentials(&kind, &mut snapshot["body"]);
    }
    Ok(snapshot)
}

fn json_diff(path: &str, from: &Json, to: &Json, changes: &mut Vec<Json>, truncated: &mut bool) {
    if from == to {
        return;
    }
    if changes.len() >= 10_000 {
        *truncated = true;
        return;
    }
    if let (Some(from), Some(to)) = (from.as_object(), to.as_object()) {
        let keys = from.keys().chain(to.keys()).cloned().collect::<BTreeSet<_>>();
        for key in keys {
            let pointer = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
            match (from.get(&key), to.get(&key)) {
                (Some(old), Some(new)) => json_diff(&pointer, old, new, changes, truncated),
                (Some(old), None) => changes.push(json!({"op":"remove","path":pointer,"old":old})),
                (None, Some(new)) => changes.push(json!({"op":"add","path":pointer,"value":new})),
                _ => {}
            }
            if changes.len() >= 10_000 {
                *truncated = true;
                break;
            }
        }
    } else {
        changes.push(json!({"op":"replace","path":path,"old":from,"value":to}));
    }
}

pub fn get_definition_diff(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let current = read(ec, text(&params, "definitionId", ""))?;
    let latest = current["version"].as_i64().unwrap_or(0);
    let from_version = params["fromVersion"].as_i64().unwrap_or(latest.saturating_sub(1));
    let to_version = params["toVersion"].as_i64().unwrap_or(latest);
    let from = version_snapshot(ec, &current, from_version)?;
    let to = version_snapshot(ec, &current, to_version)?;
    let mut changes = Vec::new();
    let mut truncated = false;
    json_diff("", &from, &to, &mut changes, &mut truncated);
    Ok(output(
        json!({"fromVersion":from_version,"toVersion":to_version,"changes":changes,"from":from,"to":to,"truncated":truncated}),
    ))
}
pub fn save_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    Ok(output(
        json!({"definition":save(ec,&params["definition"],text(&params,"note",""))?}),
    ))
}
pub fn copy_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let mut definition = read(ec, text(&params, "definitionId", ""))?;
    definition["id"] = params["newDefinitionId"].clone();
    definition["readOnly"] = json!(false);
    if let Some(map) = definition.as_object_mut() {
        map.remove("version");
    }
    if params["title"].is_string() {
        definition["title"] = params["title"].clone();
    }
    if definition["type"] == "alert" {
        super::alert_delivery::prepare_copy(ec, &mut definition["body"]);
    }
    Ok(output(json!({"definition":save(ec,&definition,"Saved as copy")?})))
}
pub fn versions_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let id = text(&params, "definitionId", "");
    read(ec, id)?;
    let rows = internal(ec, || {
        entity(ec)?.find(VERSION)?.eq("definitionId", Value::string(id)).list()
    })?;
    let versions:Vec<_>=rows.iter().map(|row|json!({"version":row.get("versionNumber").map(moqui_lib::json::to_json),"note":row.get("note").map(moqui_lib::json::to_json),"changedBy":row.get("changedBy").map(moqui_lib::json::to_json),"changedDate":row.get("changedDate").map(moqui_lib::json::to_json)})).collect();
    Ok(output(json!({"versions":versions})))
}
pub fn restore_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let id = text(&params, "definitionId", "");
    let mut definition = read(ec, id)?;
    let row = internal(ec, || {
        entity(ec)?
            .find(VERSION)?
            .eq("definitionId", Value::string(id))
            .eq("versionNumber", moqui_lib::json::from_json(&params["version"]))
            .one()
    })?
    .ok_or_else(|| fail("definition version does not exist"))?;
    let snapshot: Json =
        serde_json::from_str(row.get("bodyJson").and_then(Value::as_str).unwrap_or("{}")).map_err(fail)?;
    for key in ["body", "title", "type", "userGroupId"] {
        if let Some(value) = snapshot.get(key) {
            definition[key] = value.clone();
        }
    }
    Ok(output(
        json!({"definition":save(ec,&definition,text(&params,"note","Restored previous version"))?}),
    ))
}

/// Delete a saved definition the viewer owns, with its versions, its own references and an alert rule's state. A
/// shipped or read-only definition cannot be deleted, nor one other definitions still reference (they are named).
/// Files of git syncs that hold it are removed once the delete commits.
pub(super) fn delete(ec: &ExecutionContext, id: &str) -> Result<Json, Error> {
    let current = viewer()?;
    if shipped(ec).iter().any(|d| text(d, "id", "") == id) {
        return Err(fail("shipped definition is read-only and cannot be deleted"));
    }
    let facade = entity(ec)?;
    let row = internal(ec, || {
        facade
            .find(ENTITY)?
            .eq("definitionId", Value::string(id))
            .for_update(true)
            .one()
    })?
    .ok_or_else(|| fail(format!("report definition {id} does not exist")))?;
    let definition = row_json(&row)?;
    if text(&definition, "ownerId", "") != current {
        return Err(fail("only the owner may delete a report definition"));
    }
    if definition["readOnly"] == true {
        return Err(fail("definition is read-only"));
    }
    let users = internal(ec, || {
        facade.find(REFERENCE)?.eq("referencedId", Value::string(id)).list()
    })?;
    let users: Vec<String> = users
        .iter()
        .filter_map(|row| row.get("definitionId").and_then(Value::as_str))
        .filter(|user| *user != id)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !users.is_empty() {
        let named: Vec<String> = users
            .iter()
            .map(|user| {
                let title = internal(ec, || {
                    facade.find(ENTITY)?.eq("definitionId", Value::string(user)).one()
                })
                .ok()
                .flatten()
                .and_then(|row| row.get("title").and_then(Value::as_str).map(str::to_owned))
                .filter(|title| !title.is_empty() && title != user);
                title.map_or_else(|| user.clone(), |title| format!("{user} ({title})"))
            })
            .collect();
        return Err(fail(format!(
            "definition {id} is still used by {}; remove those references first",
            named.join(", ")
        )));
    }
    // The owner and reference checks above ran with the viewer's rights; only the storage bypasses entity auth.
    let _storage = ec.disable_authz();
    for dependent in [
        VERSION,
        REFERENCE,
        "moqui.report.AlertInstance",
        "moqui.report.AlertNotification",
        "moqui.report.AlertRecipientDelivery",
        "moqui.report.AlertRuleLock",
    ] {
        facade
            .find(dependent)?
            .eq("definitionId", Value::string(id))
            .delete_all()?;
    }
    row.delete(&facade)?;
    if definition["type"] == "dataset" {
        super::business::definition_deleted(ec, id)?;
    }
    let user = current;
    tracing::info!(target: "moqui.report.audit", user = %user, kind = %text(&definition, "type", ""),
        definition = id, "report definition deleted");
    super::git_sync::after_delete(ec, &definition);
    Ok(json!({"id":id,"type":definition["type"],"title":definition["title"]}))
}

pub fn delete_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    Ok(output(json!({"deleted":delete(ec,text(&params,"definitionId",""))?})))
}

pub fn export_definitions(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let mut pending = vec![text(&params, "definitionId", "").to_string()];
    let mut visited = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(definition) = read_optional(ec, &id)? else {
            super::dataset(ec, &id)?;
            continue;
        };
        let mut refs = BTreeSet::new();
        references(&definition["body"], &mut refs);
        pending.extend(refs);
        out.push(definition);
        if out.len() > 1000 {
            return Err(fail("definition export exceeds 1000 objects"));
        }
    }
    viewer()?;
    super::audit_export("definitions", "json", text(&params, "definitionId", ""), out.len(), &[]);
    Ok(output(json!({"bundle":{"schemaVersion":1,"definitions":out}})))
}

fn remap(value: &mut Json, ids: &HashMap<String, String>) {
    match value {
        Json::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "definitionId" | "panelRef" | "datasetId" | "dashboardRef") {
                    if let Some(id) = value.as_str().and_then(|id| ids.get(id)) {
                        *value = json!(id);
                    }
                } else if is_ref_list(key) {
                    for item in value.as_array_mut().into_iter().flatten() {
                        if let Some(id) = item.as_str().and_then(|id| ids.get(id)) {
                            *item = json!(id);
                        }
                    }
                } else {
                    remap(value, ids);
                }
            }
        }
        Json::Array(items) => {
            for value in items {
                remap(value, ids)
            }
        }
        _ => {}
    }
}
pub fn import_definitions(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let definitions = params["bundle"]["definitions"]
        .as_array()
        .ok_or_else(|| fail("definition bundle requires definitions"))?;
    if definitions.len() > 1000 {
        return Err(fail("definition import exceeds 1000 objects"));
    }
    if definitions.iter().any(|d| !d.is_object()) {
        return Err(fail("each bundle definition must be an object"));
    }
    let prefix = text(&params, "idPrefix", "import-");
    let ids: HashMap<_, _> = definitions
        .iter()
        .map(|d| (text(d, "id", "").to_owned(), format!("{prefix}{}", text(d, "id", ""))))
        .collect();
    let mut pending: Vec<_> = definitions
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d["id"] = json!(ids[text(&d, "id", "")]);
            if let Some(map) = d.as_object_mut() {
                map.remove("version");
            }
            remap(&mut d["body"], &ids);
            if d["type"] == "alert" {
                // an imported business rule notifies nobody until its owner activates it
                super::alert_delivery::prepare_copy(ec, &mut d["body"]);
            }
            d
        })
        .collect();
    let mut saved = Vec::new();
    while !pending.is_empty() {
        let before = pending.len();
        let mut remaining = Vec::new();
        for d in pending {
            let mut refs = BTreeSet::new();
            references(&d["body"], &mut refs);
            if refs.iter().any(|id| {
                ids.values().any(|mapped| mapped == id) && !saved.iter().any(|s: &Json| text(s, "id", "") == id)
            }) {
                remaining.push(d);
                continue;
            }
            saved.push(save(ec, &d, "Imported")?);
        }
        if remaining.len() == before {
            return Err(fail("definition bundle has cyclic or missing references"));
        }
        pending = remaining;
    }
    Ok(output(json!({"definitions":saved})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remaps_named_references_only() {
        let mut value = json!({"datasetId":"sales","title":"sales","panels":[{"panelRef":"panel"}]});
        remap(
            &mut value,
            &HashMap::from([
                ("sales".into(), "copy-sales".into()),
                ("panel".into(), "copy-panel".into()),
            ]),
        );
        assert_eq!(value["datasetId"], "copy-sales");
        assert_eq!(value["title"], "sales");
        assert_eq!(value["panels"][0]["panelRef"], "copy-panel");
    }
    #[test]
    fn alert_rules_reference_shared_items_by_id_lists() {
        let mut body = json!({"contactPointRefs":["ops"],"silenceRefs":["window"],"muteTimingRefs":["weekend"],"labels":{"silenceRefs":"not-a-list"}});
        let mut found = BTreeSet::new();
        references(&body, &mut found);
        assert_eq!(found, BTreeSet::from(["ops".into(), "window".into(), "weekend".into()]));
        remap(
            &mut body,
            &HashMap::from([
                ("ops".into(), "import-ops".into()),
                ("weekend".into(), "import-weekend".into()),
            ]),
        );
        assert_eq!(body["contactPointRefs"], json!(["import-ops"]));
        assert_eq!(body["silenceRefs"], json!(["window"]));
        assert_eq!(body["muteTimingRefs"], json!(["import-weekend"]));
    }
}
