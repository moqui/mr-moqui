//! Git sync of report definitions: a `DefinitionGitSync` row ties a definition folder (`body.folder`) to a directory of
//! a git repository, `git://<repositoryId>/<path>/`, one JSON file per definition in the shipped format
//! (`{id, type, title, body}`, plus `userGroupId` when set).
//!
//! - **Push** writes each saved definition of the folder that differs from its file, as one commit. A save of a
//!   definition in a synced folder pushes that one file once the save commits.
//! - **Pull** saves each file that differs from its definition, by its id. A push or a fetch to the repository pulls
//!   (a gitECA on `ref-update`); the sync's own writes are not pulled back, and a pulled definition pushes nothing,
//!   because its file already matches.
//!
//! Both run as the sync's user, so git and definition rights are that user's. One file or definition that fails is
//! logged, named in the result and in `lastError`, and the rest still sync.
//!
//! **Deletes.** Each definition a sync wrote or read is tracked (`DefinitionGitFile`). A pull deletes a tracked
//! definition whose file is gone (as the sync's user, so only one it owns and nothing still references); an untracked
//! one, such as a definition whose first write failed, is only listed. Deleting a tracked definition removes its file
//! once the delete commits, and a push removes the file of any tracked definition that no longer exists.
use super::*;
use crate::context::ExecutionContextFactory;
use std::collections::BTreeSet;
use std::sync::Arc;

const ENTITY: &str = "moqui.report.DefinitionGitSync";
const DEFINITION: &str = "moqui.report.Definition";
const FILE: &str = "moqui.report.DefinitionGitFile";
/// The artifact a viewer needs (update) to create, push or pull a sync.
const ADMIN: &str = "org.moqui.report.gitSync";

fn entity(ec: &ExecutionContext) -> Result<crate::entity::EntityFacade, Error> {
    ec.factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))
}
fn field(row: &crate::entity::EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// The file of a definition under a sync's directory.
fn location(repository: &str, path: &str, id: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("git://{repository}/{id}.json")
    } else {
        format!("git://{repository}/{path}/{id}.json")
    }
}
fn directory(repository: &str, path: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("git://{repository}/")
    } else {
        format!("git://{repository}/{path}")
    }
}

/// A definition as its file holds it.
pub(super) fn file_json(definition: &Json) -> Json {
    let mut file =
        json!({"id":definition["id"],"type":definition["type"],"title":definition["title"],"body":definition["body"]});
    if let Some(group) = definition["userGroupId"].as_str().filter(|group| !group.is_empty()) {
        file["userGroupId"] = json!(group);
    }
    file
}
/// Whether a file holds the same definition, whatever its formatting.
pub(super) fn same(file: &Json, definition: &Json) -> bool {
    file_json(file) == file_json(definition)
}
fn file_bytes(definition: &Json) -> Result<Vec<u8>, Error> {
    let mut text = serde_json::to_string_pretty(&file_json(definition)).map_err(fail)?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// Run `body` in a fresh context signed in as a user, as scheduled reports and alerts run as their owner.
fn as_user<T>(
    factory: &Arc<ExecutionContextFactory>,
    user_id: &str,
    body: impl FnOnce(&ExecutionContext) -> Result<T, Error>,
) -> Result<T, Error> {
    let context = factory.new_context();
    let _entered = context.enter();
    let facade = entity(&context)?;
    let account = {
        let _metadata = context.disable_authz();
        facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(user_id))
            .one()?
            .ok_or_else(|| fail(format!("git sync user {user_id} does not exist")))?
    };
    crate::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or("")).map_err(fail)?;
    body(&context)
}

/// Record the outcome of a push or pull on its sync, in a transaction of its own.
fn record(ec: &ExecutionContext, sync_id: &str, date_field: &str, error: Option<String>) {
    let result = (|| -> Result<(), Error> {
        let facade = entity(ec)?;
        let _metadata = ec.disable_authz();
        facade.transactions_for(ENTITY)?.run(
            crate::transaction::TransactionAttribute::ForceNew,
            None,
            || -> Result<(), moqui_lib::error::Error> {
                let Some(mut row) = facade
                    .find(ENTITY)?
                    .eq("syncId", Value::string(sync_id))
                    .for_update(true)
                    .one()?
                else {
                    return Ok(());
                };
                row.set(date_field, crate::user::now_value());
                row.set("lastError", error.clone().map_or(Value::Null, Value::String));
                row.update(&facade)?;
                Ok(())
            },
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(sync = %sync_id, %error, "git sync status could not be recorded");
    }
}

/// Mark definitions as held by a sync's directory. A failure is logged: an untracked definition is only never deleted
/// by a pull.
fn track(ec: &ExecutionContext, sync_id: &str, ids: &[String]) {
    let result = (|| -> Result<(), Error> {
        let facade = entity(ec)?;
        let _metadata = ec.disable_authz();
        for id in ids {
            let mut row = facade.make_value(FILE)?;
            row.set("syncId", Value::string(sync_id));
            row.set("definitionId", Value::string(id));
            row.set("syncedDate", crate::user::now_value());
            row.create_or_update(&facade)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(sync = %sync_id, %error, "git sync could not track its definitions; a pull will not delete them");
    }
}

/// The definitions a sync tracks.
fn tracked(ec: &ExecutionContext, sync_id: &str) -> Result<BTreeSet<String>, Error> {
    let facade = entity(ec)?;
    let _metadata = ec.disable_authz();
    Ok(facade
        .find(FILE)?
        .eq("syncId", Value::string(sync_id))
        .list()?
        .iter()
        .map(|row| field(row, "definitionId"))
        .collect())
}

fn untrack(ec: &ExecutionContext, sync_id: &str, id: &str) -> Result<(), Error> {
    let facade = entity(ec)?;
    let _metadata = ec.disable_authz();
    facade
        .find(FILE)?
        .eq("syncId", Value::string(sync_id))
        .eq("definitionId", Value::string(id))
        .delete_all()?;
    Ok(())
}

/// The saved definitions of a folder the current user may read, as their files hold them.
fn folder_definitions(ec: &ExecutionContext, folder: &str) -> Result<Vec<Json>, Error> {
    let facade = entity(ec)?;
    let rows = {
        let _metadata = ec.disable_authz();
        facade.find(DEFINITION)?.order_by("definitionId", false).list()?
    };
    let mut out = Vec::new();
    for row in rows.iter() {
        let id = field(row, "definitionId");
        let body: Json =
            serde_json::from_str(row.get("bodyJson").and_then(Value::as_str).unwrap_or("{}")).unwrap_or(Json::Null);
        if body["folder"] != folder {
            continue;
        }
        match definitions::read_optional(ec, &id) {
            Ok(Some(definition)) => out.push(definition),
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(definition = %id, %error, "git sync user may not read this definition; not pushed")
            }
        }
    }
    Ok(out)
}

/// Files to write, as (location, bytes).
type Files = Vec<(String, Vec<u8>)>;

/// The files that differ from `definitions`, and the count already the same.
fn changed_files(ec: &ExecutionContext, sync: &Json, definitions: &[Json]) -> Result<(Files, usize), Error> {
    let resource = ec.factory().resource();
    let (repository, path) = (text(sync, "repositoryId", ""), text(sync, "path", ""));
    let mut files = Vec::new();
    let mut unchanged = 0;
    for definition in definitions {
        let target = location(repository, path, text(definition, "id", ""));
        let current = match resource.location_bytes(&target) {
            Ok(bytes) => bytes.and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok()),
            Err(error) => {
                tracing::debug!(file = %target, %error, "git sync file unreadable; writing it");
                None
            }
        };
        if current.as_ref().is_some_and(|file| same(file, definition)) {
            unchanged += 1;
        } else {
            files.push((target, file_bytes(definition)?));
        }
    }
    Ok((files, unchanged))
}

fn sync_json(row: &crate::entity::EntityValue) -> Json {
    json!({"syncId":field(row,"syncId"),"folder":field(row,"folder"),"repositoryId":field(row,"repositoryId"),"path":field(row,"path"),
        "userId":field(row,"userId"),"enabled":field(row,"enabled")!="N","lastPushDate":row.get("lastPushDate").map(moqui_lib::json::to_json),
        "lastPullDate":row.get("lastPullDate").map(moqui_lib::json::to_json),"lastError":row.get("lastError").and_then(Value::as_str)})
}
fn read_sync(ec: &ExecutionContext, sync_id: &str) -> Result<Json, Error> {
    let facade = entity(ec)?;
    let _metadata = ec.disable_authz();
    facade
        .find(ENTITY)?
        .eq("syncId", Value::string(sync_id))
        .one()?
        .map(|row| sync_json(&row))
        .ok_or_else(|| fail(format!("git sync {sync_id} does not exist")))
}

/// Write every definition of the sync's folder that differs from its file, as one commit.
pub fn push(factory: &Arc<ExecutionContextFactory>, sync: &Json) -> Result<Json, Error> {
    let sync_id = text(sync, "syncId", "");
    let result = as_user(factory, text(sync, "userId", ""), |ec| {
        let definitions = folder_definitions(ec, text(sync, "folder", ""))?;
        let (files, unchanged) = changed_files(ec, sync, &definitions)?;
        if !files.is_empty() {
            ec.factory().resource().write_many(&files)?;
        }
        let ids: Vec<String> = definitions
            .iter()
            .map(|definition| text(definition, "id", "").to_owned())
            .collect();
        track(ec, sync_id, &ids);
        // A tracked definition that no longer exists was deleted here; its file goes too.
        let mut removed = Vec::new();
        for id in tracked(ec, sync_id)? {
            let exists = {
                let _metadata = ec.disable_authz();
                entity(ec)?
                    .find(DEFINITION)?
                    .eq("definitionId", Value::string(&id))
                    .one()?
                    .is_some()
            };
            if exists {
                continue;
            }
            match remove_file(ec, sync, &id) {
                Ok(()) => removed.push(Json::String(id)),
                Err(error) => {
                    tracing::error!(sync = %sync_id, definition = %id, %error, "deleted definition's git file not removed; continuing")
                }
            }
        }
        Ok(json!({"written":files.len(),"unchanged":unchanged,"removed":removed}))
    });
    with_record(factory, sync_id, "lastPushDate", result)
}

/// Save each file of the sync's directory that differs from its definition, by its id, into the sync's folder.
pub fn pull(factory: &Arc<ExecutionContextFactory>, sync: &Json) -> Result<Json, Error> {
    let sync_id = text(sync, "syncId", "");
    let result = as_user(factory, text(sync, "userId", ""), |ec| {
        let (repository, path, folder) = (
            text(sync, "repositoryId", ""),
            text(sync, "path", ""),
            text(sync, "folder", ""),
        );
        let resource = ec.factory().resource();
        let source = directory(repository, path);
        let entries = resource.list(&source)?;
        let (mut saved, mut unchanged, mut failed, mut seen) = (Vec::new(), 0, Vec::new(), BTreeSet::new());
        let files: Vec<_> = entries
            .iter()
            .filter(|entry| entry.is_file && entry.name.ends_with(".json"))
            .collect();
        for entry in &files {
            let outcome = (|| -> Result<bool, Error> {
                let bytes = resource
                    .location_bytes(&entry.location)?
                    .ok_or_else(|| fail("the file is gone"))?;
                let mut file: Json = serde_json::from_slice(&bytes).map_err(fail)?;
                if !file.is_object() || !file["body"].is_object() {
                    return Err(fail("a definition file needs an object body"));
                }
                if text(&file, "id", "").is_empty() {
                    file["id"] = json!(entry.name.trim_end_matches(".json"));
                }
                file["body"]["folder"] = json!(folder);
                let id = text(&file, "id", "").to_owned();
                seen.insert(id.clone());
                let existing = definitions::read_optional(ec, &id)?;
                if existing.as_ref().is_some_and(|definition| same(&file, definition)) {
                    track(ec, sync_id, std::slice::from_ref(&id));
                    return Ok(false);
                }
                if let Some(version) = existing.as_ref().map(|definition| definition["version"].clone()) {
                    file["version"] = version;
                }
                let note = format!("Pulled from {}", entry.location);
                let facade = entity(ec)?;
                facade.transactions_for(DEFINITION)?.run(
                    crate::transaction::TransactionAttribute::UseOrBegin,
                    None,
                    || definitions::save_pulled(ec, &file, &note),
                )?;
                track(ec, sync_id, std::slice::from_ref(&id));
                Ok(true)
            })();
            match outcome {
                Ok(true) => saved.push(json!(entry.name.trim_end_matches(".json"))),
                Ok(false) => unchanged += 1,
                Err(error) => {
                    tracing::error!(sync = %sync_id, file = %entry.location, %error, "git sync file not pulled; pulling the rest");
                    failed.push(json!({"file":entry.location,"error":error.message()}));
                }
            }
        }
        // A definition of the folder with no file: deleted when this sync tracked it (its file was removed in git),
        // only listed otherwise.
        let known = tracked(ec, sync_id)?;
        let (mut deleted, mut missing) = (Vec::new(), Vec::new());
        for id in folder_definitions(ec, folder)?
            .iter()
            .map(|definition| text(definition, "id", "").to_owned())
            .filter(|id| !seen.contains(id))
        {
            // An empty directory (a wrong path, or a branch not yet written) deletes nothing.
            if !known.contains(&id) || files.is_empty() {
                missing.push(Json::String(id));
                continue;
            }
            let facade = entity(ec)?;
            let result = facade.transactions_for(DEFINITION).and_then(|tx| {
                tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
                    definitions::delete(ec, &id)
                })
            });
            match result {
                Ok(_) => deleted.push(Json::String(id)),
                Err(error) => {
                    tracing::error!(sync = %sync_id, definition = %id, %error, "definition whose git file was removed not deleted; continuing");
                    failed.push(json!({"definition":id,"error":error.message()}));
                }
            }
        }
        Ok(json!({"saved":saved,"unchanged":unchanged,"deleted":deleted,"failed":failed,"missing":missing}))
    });
    with_record(factory, sync_id, "lastPullDate", result)
}

fn with_record(
    factory: &Arc<ExecutionContextFactory>,
    sync_id: &str,
    date_field: &str,
    result: Result<Json, Error>,
) -> Result<Json, Error> {
    let context = factory.new_context();
    let _entered = context.enter();
    let error = match &result {
        Ok(summary) => summary["failed"]
            .as_array()
            .filter(|failed| !failed.is_empty())
            .map(|failed| format!("{} files not pulled: {}", failed.len(), Json::Array(failed.clone()))),
        Err(error) => Some(error.message().to_owned()),
    };
    record(&context, sync_id, date_field, error);
    result
}

/// After a definition is saved: once the save commits, push its file to each enabled sync of its folder. A failure is
/// logged and recorded on the sync; the save stands.
pub(super) fn after_save(ec: &ExecutionContext, definition: &Json) {
    let Some(folder) = definition["body"]["folder"]
        .as_str()
        .filter(|folder| !folder.is_empty())
    else {
        return;
    };
    let syncs = (|| -> Result<Vec<Json>, Error> {
        let facade = entity(ec)?;
        let _metadata = ec.disable_authz();
        Ok(facade
            .find(ENTITY)?
            .eq("folder", Value::string(folder))
            .list()?
            .iter()
            .map(sync_json)
            .filter(|sync| sync["enabled"] == true)
            .collect())
    })();
    let syncs = match syncs {
        Ok(syncs) if !syncs.is_empty() => syncs,
        Ok(_) => return,
        Err(error) => {
            tracing::error!(definition = ?definition["id"], %error, "git syncs could not be read; the definition is not pushed");
            return;
        }
    };
    let factory = Arc::downgrade(ec.factory());
    let definition = definition.clone();
    let push_one = move || {
        let Some(factory) = factory.upgrade() else {
            return;
        };
        for sync in &syncs {
            let sync_id = text(sync, "syncId", "");
            let result = as_user(&factory, text(sync, "userId", ""), |ec| {
                let (files, _) = changed_files(ec, sync, std::slice::from_ref(&definition))?;
                if !files.is_empty() {
                    ec.factory().resource().write_many(&files)?;
                }
                track(ec, sync_id, &[text(&definition, "id", "").to_owned()]);
                Ok(json!({"written":files.len()}))
            });
            if let Err(error) = &result {
                tracing::error!(sync = %sync_id, definition = ?definition["id"], %error, "definition not pushed to git");
            }
            let _ = with_record(&factory, sync_id, "lastPushDate", result);
        }
    };
    match entity(ec).and_then(|facade| facade.transactions_for(DEFINITION)) {
        Ok(tx) if tx.in_transaction() => {
            // The hook may be refused; the push then runs now, as a commit already happened or none will.
            let pending = std::sync::Arc::new(std::sync::Mutex::new(Some(push_one)));
            let hook = std::sync::Arc::clone(&pending);
            if let Err(error) = tx.on_commit(move || {
                if let Some(push) = hook.lock().ok().and_then(|mut slot| slot.take()) {
                    push();
                }
            }) {
                tracing::error!(%error, "could not wait for the commit to push a definition; pushing now");
                if let Some(push) = pending.lock().ok().and_then(|mut slot| slot.take()) {
                    push();
                }
            }
        }
        _ => push_one(),
    }
}

/// Remove a definition's file from a sync's directory, if it is there, and stop tracking it.
fn remove_file(ec: &ExecutionContext, sync: &Json, id: &str) -> Result<(), Error> {
    let target = location(text(sync, "repositoryId", ""), text(sync, "path", ""), id);
    let resource = ec.factory().resource();
    if resource.location_bytes(&target).ok().flatten().is_some() {
        resource.delete(&target)?;
    }
    untrack(ec, text(sync, "syncId", ""), id)
}

/// After a definition is deleted: once the delete commits, remove its file from each enabled sync that tracks it. A
/// failure is logged and recorded on the sync, and the next push tries again.
pub(super) fn after_delete(ec: &ExecutionContext, definition: &Json) {
    let id = text(definition, "id", "").to_owned();
    let syncs = (|| -> Result<Vec<Json>, Error> {
        let facade = entity(ec)?;
        let _metadata = ec.disable_authz();
        let mut syncs = Vec::new();
        for row in facade.find(FILE)?.eq("definitionId", Value::string(&id)).list()?.iter() {
            if let Some(sync) = facade
                .find(ENTITY)?
                .eq("syncId", Value::string(field(row, "syncId")))
                .one()?
                .map(|row| sync_json(&row))
                .filter(|sync| sync["enabled"] == true)
            {
                syncs.push(sync);
            }
        }
        Ok(syncs)
    })();
    let syncs = match syncs {
        Ok(syncs) if !syncs.is_empty() => syncs,
        Ok(_) => return,
        Err(error) => {
            tracing::error!(definition = %id, %error, "git syncs could not be read; the next push removes the file");
            return;
        }
    };
    let factory = Arc::downgrade(ec.factory());
    let remove = move || {
        let Some(factory) = factory.upgrade() else {
            return;
        };
        for sync in &syncs {
            let sync_id = text(sync, "syncId", "");
            let result = as_user(&factory, text(sync, "userId", ""), |ec| {
                remove_file(ec, sync, &id)?;
                Ok(json!({"removed":[id.clone()]}))
            });
            if let Err(error) = &result {
                tracing::error!(sync = %sync_id, definition = %id, %error, "deleted definition's git file not removed");
            }
            let _ = with_record(&factory, sync_id, "lastPushDate", result);
        }
    };
    match entity(ec).and_then(|facade| facade.transactions_for(DEFINITION)) {
        Ok(tx) if tx.in_transaction() => {
            let pending = std::sync::Arc::new(std::sync::Mutex::new(Some(remove)));
            let hook = std::sync::Arc::clone(&pending);
            if let Err(error) = tx.on_commit(move || {
                if let Some(remove) = hook.lock().ok().and_then(|mut slot| slot.take()) {
                    remove();
                }
            }) {
                tracing::error!(%error, "could not wait for the commit to remove a git file; removing now");
                if let Some(remove) = pending.lock().ok().and_then(|mut slot| slot.take()) {
                    remove();
                }
            }
        }
        _ => remove(),
    }
}

fn authorize() -> Result<(), Error> {
    crate::user::authorize_own("AT_SERVICE", ADMIN, "update").map_err(fail)
}

pub fn store_sync(ec: &ExecutionContext, _service: &ServiceInfo<'_>, params: &Parameters) -> Result<Parameters, Error> {
    authorize()?;
    let params = input(params);
    let facade = entity(ec)?;
    let sync_id = text(&params, "syncId", "").to_owned();
    for name in ["folder", "repositoryId", "userId"] {
        if text(&params, name, "").is_empty() && sync_id.is_empty() {
            return Err(fail(format!("a git sync needs {name}")));
        }
    }
    let _metadata = ec.disable_authz();
    let existing = if sync_id.is_empty() {
        None
    } else {
        facade
            .find(ENTITY)?
            .eq("syncId", Value::string(&sync_id))
            .for_update(true)
            .one()?
    };
    let creating = existing.is_none();
    let mut row = existing.unwrap_or(facade.make_value(ENTITY)?);
    if creating {
        if sync_id.is_empty() {
            row.set_sequenced_id_primary(&facade)?;
        } else {
            row.set("syncId", Value::string(sync_id));
        }
    }
    for name in ["folder", "repositoryId", "path", "userId"] {
        if let Some(value) = params[name].as_str() {
            row.set(name, Value::string(value));
        }
    }
    if let Some(enabled) = params.get("enabled").filter(|value| !value.is_null()) {
        let on = enabled == true || enabled == "Y" || enabled == "true";
        row.set("enabled", Value::string(if on { "Y" } else { "N" }));
    } else if creating {
        row.set("enabled", Value::string("Y"));
    }
    if creating {
        row.create(&facade)?;
    } else {
        row.update(&facade)?;
    }
    Ok(output(json!({"sync":sync_json(&row)})))
}

pub fn list_syncs(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _params: &Parameters,
) -> Result<Parameters, Error> {
    authorize()?;
    let facade = entity(ec)?;
    let _metadata = ec.disable_authz();
    let rows = facade.find(ENTITY)?.order_by("folder", false).list()?;
    Ok(output(json!({"syncs":rows.iter().map(sync_json).collect::<Vec<_>>()})))
}

pub fn push_sync(ec: &ExecutionContext, _service: &ServiceInfo<'_>, params: &Parameters) -> Result<Parameters, Error> {
    authorize()?;
    let sync = read_sync(ec, text(&input(params), "syncId", ""))?;
    push(ec.factory(), &sync).map(output)
}

pub fn pull_sync(ec: &ExecutionContext, _service: &ServiceInfo<'_>, params: &Parameters) -> Result<Parameters, Error> {
    authorize()?;
    let sync = read_sync(ec, text(&input(params), "syncId", ""))?;
    pull(ec.factory(), &sync).map(output)
}

/// For the gitECA of a push or fetch: pull every enabled sync of the repository, when its default branch changed. A
/// sync that fails is logged and recorded, and the others still pull.
pub fn pull_repository(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    if params["isDefaultBranch"] == false || params["isDefaultBranch"] == "false" {
        return Ok(output(json!({"pulled":0})));
    }
    let repository = text(&params, "repositoryId", "");
    let syncs: Vec<Json> = {
        let facade = entity(ec)?;
        let _metadata = ec.disable_authz();
        facade
            .find(ENTITY)?
            .eq("repositoryId", Value::string(repository))
            .list()?
            .iter()
            .map(sync_json)
            .filter(|sync| sync["enabled"] == true)
            .collect()
    };
    let mut pulled = 0;
    for sync in &syncs {
        match pull(ec.factory(), sync) {
            Ok(_) => pulled += 1,
            Err(error) => {
                tracing::error!(sync = ?sync["syncId"], %repository, %error, "git sync pull failed; pulling the other syncs")
            }
        }
    }
    Ok(output(json!({"pulled":pulled})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_compare_by_content_and_keep_the_shipped_shape() {
        let definition = json!({"id":"sales","type":"dashboard","title":"Sales","body":{"folder":"Shop","panels":{}},"ownerId":"u1","version":3,"userGroupId":null});
        let file = file_json(&definition);
        assert_eq!(
            file,
            json!({"id":"sales","type":"dashboard","title":"Sales","body":{"folder":"Shop","panels":{}}})
        );
        let reformatted: Json = serde_json::from_str("{\"title\":\"Sales\",\"body\":{\"panels\":{},\"folder\":\"Shop\"},\"type\":\"dashboard\",\"id\":\"sales\"}").unwrap();
        assert!(same(&reformatted, &definition));
        let mut changed = definition.clone();
        changed["title"] = json!("Sales by month");
        assert!(!same(&file, &changed));
        assert!(String::from_utf8(file_bytes(&definition).unwrap())
            .unwrap()
            .ends_with("}\n"));
        assert_eq!(location("repo", "/dash/", "sales"), "git://repo/dash/sales.json");
        assert_eq!(location("repo", "", "sales"), "git://repo/sales.json");
        assert_eq!(directory("repo", ""), "git://repo/");
    }
}
