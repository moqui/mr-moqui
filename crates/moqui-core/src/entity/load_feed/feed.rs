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

//! Data-feed touches of each transaction frame, and the `DataFeedPending` insert at commit. The feed and document
//! definitions it needs are read once and kept on the facade until one of them is written.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use moqui_db::sql::{
    insert, quote_column, quote_ident, quote_physical_ident, select, Condition, FieldRef, LimitOffset,
};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::loader::canonical_name;
use super::{with_load_slot, FeedServiceNames};
use crate::driver::{DbConnection, DbRow};
use crate::entity::facade::EntityFacade;
use crate::transaction::TransactionManager;

const SKIP_FEED_ENTITY: &str = "moqui.entity.SequenceValueItem";
const PENDING_ENTITY: &str = "moqui.entity.feed.DataFeedPending";
/// Most primary documents one related write may reach before the rest are left to a rebuild.
const MAX_RELATED_DOCS: usize = 10_000;
const REALTIME_PUSH: &str = "DTFDTP_RT_PUSH";
const SAVEPOINT: &str = "moqui_feed";

thread_local! {
    /// Data-feed touches of each open transaction frame on this thread, by frame token.
    pub(super) static TOUCHES: RefCell<HashMap<u64, Vec<Touch>>> = RefCell::new(HashMap::new());
}

pub(super) struct Touch {
    entity_name: String,
    operation: String,
    /// The written values. The feed reads keys from them, and a related delete walks back to the
    /// primary entity from them. `None` falls back to the Postgres `xmin` probe.
    fields: Option<BTreeMap<String, Value>>,
}

struct Agg {
    saw_write: bool,
    saw_delete: bool,
}

struct DocTarget {
    data_document_id: String,
    entity_name: String,
    primary: bool,
    primary_entity: String,
    /// Relationship steps from the primary entity to this one, `(entity, relationship)` each.
    paths: Vec<Vec<(String, String)>>,
    /// Fields of this entity whose write reaches the document, as Java `DocumentEntityInfo.fields`: the
    /// document's fields on it and the key fields of each relationship that leaves it.
    fields: HashSet<String>,
}

/// Note a write for the data feed when it ran inside a transaction. Call it after the statement succeeded, so a
/// failed write leaves nothing. The data feed uses `fields` to find documents.
///
/// The frame's first touch registers dropping the frame's touches when it ends. The feed insert has taken them
/// before a commit; a rollback, or a frame with no feed insert, leaves nothing behind either.
pub(crate) fn note_touch(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    fields: Option<&BTreeMap<String, Value>>,
) {
    let Some(tx) = facade.transactions_for(entity_name).ok() else {
        return;
    };
    let Some(frame) = tx.frame_token() else {
        return;
    };
    if !TOUCHES.with(|touches| touches.borrow().contains_key(&frame))
        && (tx.on_commit(move || drop_frame_touches(frame)).is_err()
            || tx.on_rollback(move || drop_frame_touches(frame)).is_err())
    {
        return;
    }
    let keep_fields =
        !facade.flags().disable_data_feed && entity_name != SKIP_FEED_ENTITY && entity_name != PENDING_ENTITY;
    let touch = Touch {
        entity_name: entity_name.to_string(),
        operation: operation.to_string(),
        fields: fields.filter(|_| keep_fields).cloned(),
    };
    TOUCHES.with(|touches| touches.borrow_mut().entry(frame).or_default().push(touch));
}

fn drop_frame_touches(frame: u64) {
    TOUCHES.with(|touches| touches.borrow_mut().remove(&frame));
}

/// Forget the touches of every frame on this thread. Only for a thread with no transaction frame left.
pub(in crate::entity) fn clear_frame_touches() {
    TOUCHES.with(|touches| touches.borrow_mut().clear());
}

/// Register the hooks for the open frame: before commit, insert `DataFeedPending` on the
/// enlisted connection; after commit, hand the inserted rows to the feed receive. The touches
/// drop themselves at rollback (see [`note_touch`]).
pub fn register_feed_insert(facade: &EntityFacade, tx: &TransactionManager) -> Result<(), Error> {
    let frame = tx.frame_token();
    let inserted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let insert_facade = facade.clone();
    let insert_ids = std::sync::Arc::clone(&inserted);
    tx.on_feed_insert(move |conn| {
        let ids = insert_pending(&insert_facade, conn, frame)?;
        insert_ids.lock().unwrap_or_else(|err| err.into_inner()).extend(ids);
        Ok(())
    })?;
    let commit_facade = facade.clone();
    tx.on_commit(move || {
        let ids = std::mem::take(&mut *inserted.lock().unwrap_or_else(|err| err.into_inner()));
        if let Err(err) = crate::service::schedule_feed_receive(&commit_facade, ids) {
            tracing::warn!(error = %err, "data feed receive was not scheduled; pending rows stay");
        }
    })
}

/// Insert `DataFeedPending` rows for this frame's writes. Returns the inserted row ids.
fn insert_pending(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    frame: Option<u64>,
) -> Result<Vec<String>, Error> {
    let touches = take_frame_touches(frame);
    if facade.sim_active() || facade.flags().disable_data_feed {
        return Ok(Vec::new());
    }
    let aggs = aggregate_touches(&touches);
    if aggs.is_empty() {
        return Ok(Vec::new());
    }
    // A frame that wrote a feed definition reads its own uncommitted rows, which no other frame may see.
    let own_definitions = aggs.keys().any(|entity_name| {
        canonical_name(facade, entity_name).is_ok_and(|name| crate::entity::cache::is_feed_definition(&name))
    });
    let config = feed_config(facade, conn, !own_definitions)?;
    let targets = &config.targets;
    if !aggs
        .keys()
        .any(|entity_name| targets.iter().any(|target| &target.entity_name == entity_name))
    {
        return Ok(Vec::new());
    }
    let insert_sql = pending_insert_sql(facade)?;
    let mut inserted = Vec::new();
    // Writes that carry their values: the document id is found from them, and the last write to
    // a document decides between index and delete.
    let mut resolved: indexmap::IndexMap<(String, String), (String, &'static str)> = indexmap::IndexMap::new();
    for touch in &touches {
        let Some(fields) = &touch.fields else {
            continue;
        };
        let pk_names = facade
            .with_entity(&touch.entity_name, |entity, _, _| {
                Ok(entity.pk_field_names().into_iter().map(str::to_string).collect::<HashSet<_>>())
            })
            .unwrap_or_default();
        for hit in targets
            .iter()
            .filter(|target| target.entity_name == touch.entity_name)
            .filter(|target| touch.operation == "delete" || writes_document_field(touch, fields, &pk_names, target))
        {
            if hit.primary {
                let Some(id) = pk_text(facade, &touch.entity_name, fields) else {
                    continue;
                };
                let operation = if touch.operation == "delete" { "delete" } else { "index" };
                let key = (hit.data_document_id.clone(), id);
                resolved.shift_remove(&key);
                resolved.insert(key, (hit.primary_entity.clone(), operation));
            } else {
                let row = if touch.operation == "delete" {
                    fields.clone()
                } else {
                    reread(facade, conn, &touch.entity_name, fields)?.unwrap_or_else(|| fields.clone())
                };
                for path in &hit.paths {
                    for id in walk_to_primary(facade, conn, path, vec![row.clone()])? {
                        let key = (hit.data_document_id.clone(), id);
                        if !resolved.contains_key(&key) {
                            resolved.insert(key, (hit.primary_entity.clone(), "index"));
                        }
                    }
                }
            }
        }
    }
    for ((document_id, id), (entity_name, operation)) in &resolved {
        let row_id = crate::entity::uuid_v4_for("data-feed-row")?;
        let params = vec![
            Value::string(&row_id),
            Value::string(document_id),
            Value::string(entity_name),
            Value::string(id),
            Value::string(*operation),
        ];
        probe_execute(conn, &insert_sql, &params)?;
        inserted.push(row_id);
    }
    // Writes recorded without values, which only the Postgres xmin probe can key.
    let legacy: HashSet<&str> = touches
        .iter()
        .filter(|touch| touch.fields.is_none())
        .map(|touch| touch.entity_name.as_str())
        .collect();
    let mut seen = HashSet::new();
    for (entity_name, agg) in &aggs {
        if !legacy.contains(entity_name.as_str()) {
            continue;
        }
        let hits: Vec<&DocTarget> = targets
            .iter()
            .filter(|target| &target.entity_name == entity_name)
            .collect();
        if hits.is_empty() {
            continue;
        }
        let Some(keys) = primary_keys(facade, conn, entity_name)? else {
            continue;
        };
        for key in keys {
            for hit in &hits {
                let operation = if agg.saw_write {
                    "index"
                } else if hit.primary {
                    "delete"
                } else {
                    "index"
                };
                let dedup = (
                    hit.data_document_id.clone(),
                    entity_name.clone(),
                    key.clone(),
                    operation.to_string(),
                );
                if !seen.insert(dedup) {
                    continue;
                }
                let row_id = crate::entity::uuid_v4_for("data-feed-row")?;
                let params = vec![
                    Value::string(&row_id),
                    Value::string(&hit.data_document_id),
                    Value::string(entity_name),
                    Value::string(&key),
                    Value::string(operation),
                ];
                probe_execute(conn, &insert_sql, &params)?;
                inserted.push(row_id);
            }
        }
    }
    Ok(inserted)
}

/// Whether a create or update wrote a field of the target, as Java `dataFeedCheckAndRegister`: a create any of
/// them, an update one that is not a primary key. Java also skips an update that sets a field to its old value;
/// the update here keeps no old values.
fn writes_document_field(
    touch: &Touch,
    fields: &BTreeMap<String, Value>,
    pk_names: &HashSet<String>,
    target: &DocTarget,
) -> bool {
    target
        .fields
        .iter()
        .any(|name| fields.contains_key(name) && (touch.operation != "update" || !pk_names.contains(name)))
}

/// Primary key values joined with `::`, or `None` when one is missing.
fn pk_text(facade: &EntityFacade, entity_name: &str, fields: &BTreeMap<String, Value>) -> Option<String> {
    let names = facade
        .with_entity(entity_name, |entity, _, _| {
            Ok(entity
                .pk_field_names()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>())
        })
        .ok()?;
    let mut parts = Vec::with_capacity(names.len());
    for name in &names {
        let text = scalar_text(fields.get(name)?)?;
        if text.is_empty() {
            return None;
        }
        parts.push(text);
    }
    (!parts.is_empty()).then(|| parts.join("::"))
}

/// Every field of the row with these primary key values, read on the enlisted connection.
fn reread(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
) -> Result<Option<BTreeMap<String, Value>>, Error> {
    let (names, pk_names) = facade.with_entity(entity_name, |entity, _, _| {
        Ok((
            entity.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>(),
            entity
                .pk_field_names()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ))
    })?;
    let mut conds = Vec::new();
    for name in &pk_names {
        match fields.get(name) {
            Some(value) if !value.is_null() => {
                conds.push(Condition::equals(FieldRef::new(name.clone()), value.clone()))
            }
            _ => return Ok(None),
        }
    }
    let rows = select_rows(facade, conn, entity_name, &names, Condition::and(conds))?;
    Ok(rows.into_iter().next())
}

fn select_rows(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    entity_name: &str,
    names: &[String],
    condition: Condition,
) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    let columns: Vec<&str> = names.iter().map(String::as_str).collect();
    let (sql, params) = select_sql(facade, entity_name, &columns, Some(condition))?;
    let rows = probe_query(conn, &sql, &params)?;
    Ok(rows
        .iter()
        .map(|row| {
            names
                .iter()
                .map(|name| (name.clone(), cell_value(row, name).cloned().unwrap_or(Value::Null)))
                .collect()
        })
        .collect())
}

/// Walk relationship steps back from rows of the last entity to the primary entity's ids.
fn walk_to_primary(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    path: &[(String, String)],
    rows: Vec<BTreeMap<String, Value>>,
) -> Result<Vec<String>, Error> {
    let mut rows = rows;
    for (parent, relationship) in path.iter().rev() {
        let (_, pairs) = crate::entity::query_rest::relationship_pairs(facade, parent, relationship)?;
        let names = facade.with_entity(parent, |entity, _, _| {
            Ok(entity.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>())
        })?;
        let mut next = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows {
            let mut conds = Vec::with_capacity(pairs.len());
            for (field, related) in &pairs {
                match row.get(related) {
                    Some(value) if !value.is_null() => {
                        conds.push(Condition::equals(FieldRef::new(field.clone()), value.clone()))
                    }
                    _ => {
                        conds.clear();
                        break;
                    }
                }
            }
            if conds.is_empty() {
                continue;
            }
            let key: Vec<String> = conds.iter().map(|cond| format!("{cond:?}")).collect();
            if !seen.insert(key) {
                continue;
            }
            next.extend(select_rows(facade, conn, parent, &names, Condition::and(conds))?);
            if next.len() > MAX_RELATED_DOCS {
                tracing::warn!(
                    entity = %parent,
                    limit = MAX_RELATED_DOCS,
                    "a related write reaches too many documents; rebuild the feed to index the rest"
                );
                next.truncate(MAX_RELATED_DOCS);
                break;
            }
        }
        rows = next;
    }
    let primary = path.first().map(|(entity, _)| entity.clone()).unwrap_or_default();
    let mut ids = Vec::new();
    for row in &rows {
        if let Some(id) = pk_text(facade, &primary, row) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

fn aggregate_touches(touches: &[Touch]) -> HashMap<String, Agg> {
    let mut aggs = HashMap::new();
    for touch in touches {
        if touch.entity_name == SKIP_FEED_ENTITY {
            continue;
        }
        let agg = aggs.entry(touch.entity_name.clone()).or_insert(Agg {
            saw_write: false,
            saw_delete: false,
        });
        match touch.operation.as_str() {
            "create" | "update" => agg.saw_write = true,
            "delete" => agg.saw_delete = true,
            _ => {}
        }
    }
    aggs.retain(|_, agg| agg.saw_write || agg.saw_delete);
    aggs
}

/// Take this frame's touches, so a `force-new` commit takes only its own.
fn take_frame_touches(frame: Option<u64>) -> Vec<Touch> {
    let Some(frame) = frame else {
        return Vec::new();
    };
    TOUCHES
        .with(|touches| touches.borrow_mut().remove(&frame))
        .unwrap_or_default()
}

fn remember_feed_services(facade: &EntityFacade, services: &[FeedServiceNames]) {
    if services.is_empty() {
        return;
    }
    with_load_slot(facade, |slot| {
        for service in services {
            slot.feed_services.insert(service.data_feed_id.clone(), service.clone());
        }
    });
}

/// The real-time feeds and the document targets they index, read once per feed definition generation (see
/// [`crate::entity::cache::feed_definition_generation`]) and kept on the facade.
pub(super) struct FeedConfig {
    generation: u64,
    targets: Vec<DocTarget>,
}

/// The feed configuration, from the facade when no definition was written since it was read. Otherwise it is read
/// on the enlisted connection, and kept when `keep`.
fn feed_config(facade: &EntityFacade, conn: &mut dyn DbConnection, keep: bool) -> Result<Arc<FeedConfig>, Error> {
    let generation = crate::entity::cache::feed_definition_generation(facade);
    if keep {
        let cached = with_load_slot(facade, |slot| slot.feed_config.clone());
        if let Some(config) = cached.filter(|config| config.generation == generation) {
            return Ok(config);
        }
    }
    let (feed_sql, feed_params) = select_sql(
        facade,
        "moqui.entity.feed.DataFeed",
        &["dataFeedId", "feedReceiveServiceName", "feedDeleteServiceName"],
        Some(Condition::equals("dataFeedTypeEnumId", Value::string(REALTIME_PUSH))),
    )?;
    let feed_rows = probe_query(conn, &feed_sql, &feed_params)?;
    let mut services = Vec::new();
    for row in &feed_rows {
        let data_feed_id = cell_string(row, "dataFeedId");
        if data_feed_id.is_empty() {
            continue;
        }
        services.push(FeedServiceNames {
            data_feed_id,
            receive_service: cell_string(row, "feedReceiveServiceName"),
            delete_service: cell_string(row, "feedDeleteServiceName"),
        });
    }
    let targets = if services.is_empty() {
        Vec::new()
    } else {
        let feed_ids: Vec<String> = services.iter().map(|service| service.data_feed_id.clone()).collect();
        feed_targets(facade, conn, &feed_ids)?
    };
    remember_feed_services(facade, &services);
    let config = Arc::new(FeedConfig { generation, targets });
    if keep {
        with_load_slot(facade, |slot| {
            if slot
                .feed_config
                .as_ref()
                .is_none_or(|kept| kept.generation <= generation)
            {
                slot.feed_config = Some(Arc::clone(&config));
            }
        });
    }
    Ok(config)
}

fn feed_targets(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    feed_ids: &[String],
) -> Result<Vec<DocTarget>, Error> {
    let (link_sql, link_params) = select_sql(
        facade,
        "moqui.entity.feed.DataFeedDocument",
        &["dataFeedId", "dataDocumentId"],
        None,
    )?;
    let links = probe_query(conn, &link_sql, &link_params)?;
    let (doc_sql, doc_params) = select_sql(
        facade,
        "moqui.entity.document.DataDocument",
        &["dataDocumentId", "primaryEntityName"],
        None,
    )?;
    let documents = probe_query(conn, &doc_sql, &doc_params)?;
    let (field_sql, field_params) = select_sql(
        facade,
        "moqui.entity.document.DataDocumentField",
        &["dataDocumentId", "fieldPath"],
        None,
    )?;
    let fields = probe_query(conn, &field_sql, &field_params)?;
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    for row in &fields {
        let document = cell_string(row, "dataDocumentId");
        let path = cell_string(row, "fieldPath");
        if document.is_empty() || path.is_empty() || path.contains('(') {
            continue;
        }
        paths.entry(document).or_default().push(path);
    }
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for link in &links {
        let feed_id = cell_string(link, "dataFeedId");
        if !feed_ids.iter().any(|id| id == &feed_id) {
            continue;
        }
        let document_id = cell_string(link, "dataDocumentId");
        let Some(primary) = documents.iter().find_map(|row| {
            if cell_string(row, "dataDocumentId") == document_id {
                let name = cell_string(row, "primaryEntityName");
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            } else {
                None
            }
        }) else {
            continue;
        };
        let Ok(primary_full) = canonical_name(facade, &primary) else {
            tracing::warn!(entity = %primary, document = %document_id, "data document primary entity is not defined");
            continue;
        };
        let document_paths = paths.get(&document_id).map(Vec::as_slice).unwrap_or(&[]);
        let mut fields = document_fields(facade, &primary_full, document_paths);
        let mut take = |name: &str| fields.remove(name).unwrap_or_default();
        push_target(
            &mut targets,
            &mut seen,
            &document_id,
            &primary_full,
            &primary_full,
            None,
            take(&primary_full),
        );
        for (name, steps) in related_entities(facade, &primary_full, document_paths) {
            let entity_fields = take(&name);
            push_target(&mut targets, &mut seen, &document_id, &name, &primary_full, Some(steps), entity_fields);
        }
    }
    Ok(targets)
}

/// Add a target, or add a path to an existing one. `steps` is `None` for the primary entity.
fn push_target(
    targets: &mut Vec<DocTarget>,
    seen: &mut HashSet<(String, String)>,
    document_id: &str,
    entity_name: &str,
    primary_entity: &str,
    steps: Option<Vec<(String, String)>>,
    fields: HashSet<String>,
) {
    if seen.insert((document_id.to_string(), entity_name.to_string())) {
        targets.push(DocTarget {
            data_document_id: document_id.to_string(),
            entity_name: entity_name.to_string(),
            primary: steps.is_none(),
            primary_entity: primary_entity.to_string(),
            paths: steps.into_iter().collect(),
            fields,
        });
    } else if let Some(target) = targets
        .iter_mut()
        .find(|target| target.data_document_id == document_id && target.entity_name == entity_name)
    {
        target.fields.extend(fields);
        if let Some(steps) = steps {
            if !target.paths.contains(&steps) {
                target.paths.push(steps);
            }
        }
    }
}

/// The fields of each entity of a document whose write reaches it, as Java `getDataDocumentEntityInfo`: the last
/// element of a field path on the entity it reaches, and the key fields of each relationship on the entity it leaves.
fn document_fields(facade: &EntityFacade, primary: &str, paths: &[String]) -> HashMap<String, HashSet<String>> {
    let mut fields: HashMap<String, HashSet<String>> = HashMap::new();
    for path in paths {
        let parts: Vec<&str> = path.split(':').collect();
        let Some((leaf, relationships)) = parts.split_last() else {
            continue;
        };
        let mut current = primary.to_string();
        let mut reached = true;
        for rel_name in relationships {
            let Ok((next, pairs)) = crate::entity::query_rest::relationship_pairs(facade, &current, rel_name) else {
                reached = false;
                break;
            };
            fields
                .entry(current.clone())
                .or_default()
                .extend(pairs.into_iter().map(|(field, _)| field));
            current = next;
        }
        if reached {
            fields.entry(current).or_default().insert((*leaf).to_string());
        }
    }
    fields
}

/// Each entity on a field path below the primary, with the `(entity, relationship)` steps that
/// reach it.
fn related_entities(facade: &EntityFacade, primary: &str, paths: &[String]) -> Vec<(String, Vec<(String, String)>)> {
    let mut names: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for path in paths {
        let parts: Vec<&str> = path.split(':').collect();
        if parts.len() < 2 {
            continue;
        }
        let mut current = primary.to_string();
        let mut steps: Vec<(String, String)> = Vec::new();
        for rel_name in &parts[..parts.len() - 1] {
            let next = facade
                .with_entity(&current, |entity, _, _| {
                    Ok(entity.relationship(rel_name).map(|rel| rel.related.clone()))
                })
                .ok()
                .flatten();
            let Some(next) = next else {
                break;
            };
            let Ok(full) = canonical_name(facade, &next) else {
                break;
            };
            steps.push((current.clone(), (*rel_name).to_string()));
            let entry = (full.clone(), steps.clone());
            if !names.contains(&entry) {
                names.push(entry);
            }
            current = full;
        }
    }
    names
}

fn primary_keys(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    entity_name: &str,
) -> Result<Option<Vec<String>>, Error> {
    let (sql, pk_names) = match xmin_query(facade, entity_name)? {
        Some(query) => query,
        None => {
            tracing::warn!(entity = %entity_name, "data feed primary keys need postgres xmin");
            return Ok(Some(Vec::new()));
        }
    };
    let rows = probe_query(conn, &sql, &[])?;
    let mut keys = Vec::new();
    for row in &rows {
        let mut parts = Vec::new();
        let mut complete = true;
        for name in &pk_names {
            match cell_pk(row, name) {
                Some(text) => parts.push(text),
                None => complete = false,
            }
        }
        if complete && !parts.is_empty() {
            keys.push(parts.join("::"));
        }
    }
    Ok(Some(keys))
}

fn xmin_query(facade: &EntityFacade, entity_name: &str) -> Result<Option<(String, Vec<String>)>, Error> {
    facade.with_entity(entity_name, |entity, dialect, _| {
        if entity.is_view || !dialect.name.to_ascii_lowercase().starts_with("postgres") {
            return Ok(None);
        }
        let names: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
        if names.is_empty() {
            return Ok(None);
        }
        let columns = names
            .iter()
            .map(|name| {
                let field = entity.field(name).expect("primary key field");
                format!(
                    "{} AS {}",
                    quote_column(dialect, &field.column_name),
                    quote_ident(dialect, name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {columns} FROM {} WHERE xmin::text = txid_current()::text",
            quote_physical_ident(dialect, &entity.table_name)
        );
        Ok(Some((sql, names)))
    })
}

fn pending_insert_sql(facade: &EntityFacade) -> Result<String, Error> {
    facade.with_entity("moqui.entity.feed.DataFeedPending", |entity, dialect, _| {
        insert(
            entity,
            dialect,
            &[
                "dataFeedPendingId",
                "dataDocumentId",
                "entityName",
                "primaryKey",
                "operation",
            ],
        )
    })
}

fn select_sql(
    facade: &EntityFacade,
    entity_name: &str,
    columns: &[&str],
    condition: Option<Condition>,
) -> Result<(String, Vec<Value>), Error> {
    facade.with_entity(entity_name, |entity, dialect, _| {
        let fields: Vec<FieldRef> = columns.iter().copied().map(FieldRef::new).collect();
        let statement = select(
            entity,
            dialect,
            &fields,
            condition.as_ref(),
            &[],
            LimitOffset::default(),
        )?;
        Ok((statement.sql, statement.params))
    })
}

pub(super) fn probe_query(conn: &mut dyn DbConnection, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
    probe(conn, Vec::new(), |conn| conn.query(sql, params))
}

fn probe_execute(conn: &mut dyn DbConnection, sql: &str, params: &[Value]) -> Result<(), Error> {
    probe(conn, (), |conn| conn.execute(sql, params).map(|_| ()))
}

/// Runs `run` inside a savepoint. A missing table gives `missing`, once the savepoint is rolled
/// back; if that rollback fails the transaction is unusable, so the rollback error is returned.
fn probe<T>(
    conn: &mut dyn DbConnection,
    missing: T,
    run: impl FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
) -> Result<T, Error> {
    conn.savepoint(SAVEPOINT)?;
    match run(conn) {
        Ok(value) => {
            conn.release_savepoint(SAVEPOINT)?;
            Ok(value)
        }
        Err(err) => {
            let undone = conn
                .rollback_to_savepoint(SAVEPOINT)
                .and_then(|()| conn.release_savepoint(SAVEPOINT));
            if relation_missing(&err) {
                undone.map(|()| missing)
            } else {
                if let Err(undo) = undone {
                    tracing::warn!("Could not roll back savepoint {SAVEPOINT} after a failed probe: {undo}");
                }
                Err(err)
            }
        }
    }
}

fn relation_missing(err: &Error) -> bool {
    let text = error_chain(err).to_ascii_lowercase();
    let missing = text.contains("does not exist")
        || text.contains("doesn't exist")
        || text.contains("unknown table")
        || text.contains("42p01");
    missing && (text.contains("relation") || text.contains("table") || text.contains("42p01"))
}

fn error_chain(err: &Error) -> String {
    let mut text = String::new();
    let mut current = Some(err as &dyn std::error::Error);
    while let Some(item) = current {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&item.to_string());
        current = item.source();
    }
    text
}

fn cell_value<'a>(row: &'a DbRow, name: &str) -> Option<&'a Value> {
    row.columns
        .iter()
        .find(|(column, _)| column.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn cell_string(row: &DbRow, name: &str) -> String {
    match cell_value(row, name) {
        Some(value) => scalar_text(value).unwrap_or_default(),
        None => String::new(),
    }
}

fn cell_pk(row: &DbRow, name: &str) -> Option<String> {
    let text = cell_string(row, name);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::I32(number) => Some(number.to_string()),
        Value::I64(number) => Some(number.to_string()),
        Value::F64(number) => Some(number.to_string()),
        Value::Decimal(number) => Some(number.to_string()),
        Value::Timestamp(timestamp) => Some(timestamp.to_string()),
        Value::Date(date) => Some(date.to_string()),
        Value::Time(time) => Some(time.to_string()),
        Value::Bytes(_) | Value::List(_) | Value::Map(_) | Value::Object(_) => None,
    }
}
