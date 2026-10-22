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
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use moqui_db::sql::{
    insert, quote_column, quote_ident, quote_physical_ident, select, Condition, FieldRef, LimitOffset,
};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::entity::FieldMap;
use super::loader::canonical_name;
use super::{error_chain, with_load_slot, with_load_slot_read, FeedServiceNames};
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
    /// primary entity from them. `None` falls back to the Postgres `xmin` probe. Shared with the write's other hooks.
    fields: Option<Arc<FieldMap>>,
    /// For an update of a feed entity, the row as it was before, so an update that changes no document field is
    /// not fed (Java's `oldValues`). `None` feeds the update.
    old: Option<FieldMap>,
    /// The agent whose run made the write (the execution context's agent origin), for the agent triggers' loop guard.
    origin_agent: Option<String>,
}

impl Touch {
    #[cfg(test)]
    pub(super) fn keeps_fields(&self) -> bool {
        self.fields.is_some()
    }
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
    /// Java's `DocumentEntityInfo.fields`: this entity's document fields, and its key fields of the relationships
    /// the paths take from it. A write that sets none of them is not fed.
    fields: HashSet<String>,
    /// Java's `DocumentEntityInfo.conditions`: field and value of each document condition on this entity. A write
    /// with another value in such a field is not fed.
    conditions: Vec<(String, String)>,
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
    fields: Option<&Arc<FieldMap>>,
    old: Option<FieldMap>,
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
    let keep_fields = !facade.flags().disable_data_feed
        && entity_name != SKIP_FEED_ENTITY
        && entity_name != PENDING_ENTITY
        && feed_may_target(facade, entity_name);
    let touch = Touch {
        entity_name: entity_name.to_string(),
        operation: operation.to_string(),
        fields: fields.filter(|_| keep_fields).cloned(),
        old: old.filter(|_| keep_fields),
        origin_agent: keep_fields
            .then(|| crate::context::ExecutionContext::with_current(|ec| ec.agent_origin()).flatten())
            .flatten()
            .map(|origin| origin.agent_id),
    };
    TOUCHES.with(|touches| touches.borrow_mut().entry(frame).or_default().push(touch));
}

/// False when the feed configuration, as last read and not older than the feed definitions, has no document on
/// `entity_name` (compared as [`insert_pending`] compares a touch's name): the commit then reads no values of the
/// write. True before the configuration is first read, or after a definition was written since.
fn feed_may_target(facade: &EntityFacade, entity_name: &str) -> bool {
    let generation = crate::entity::cache::feed_definition_generation(facade);
    with_load_slot_read(facade, |slot| {
        slot.feed_config.as_ref().is_none_or(|config| {
            config.generation != generation || config.targets.iter().any(|target| target.entity_name == entity_name)
        })
    })
}

/// Whether an update of `entity_name` should read the row first for the data feed: a known feed configuration has
/// a document on the entity. Before the configuration is first read, updates are fed without that check.
pub(crate) fn feed_wants_old(facade: &EntityFacade, entity_name: &str) -> bool {
    if facade.flags().disable_data_feed || facade.sim_active() {
        return false;
    }
    let Ok(full_name) = canonical_name(facade, entity_name) else {
        return false;
    };
    with_load_slot_read(facade, |slot| {
        slot.feed_config
            .as_ref()
            .is_some_and(|config| config.targets.iter().any(|target| target.entity_name == full_name))
    })
}

/// Java's `dataFeedCheckAndRegister` for one document: a create or update is fed when it sets or changes one of the
/// document's fields of this entity, and has no other value in a field a document condition names. A delete is
/// always fed.
fn touch_feeds(target: &DocTarget, touch: &Touch, fields: &FieldMap) -> bool {
    if touch.operation == "delete" {
        return true;
    }
    let text = |value: &Value| moqui_lib::object::to_plain_string(value);
    let changed = target.fields.iter().any(|name| {
        let Some(value) = fields.get(name.as_str()) else {
            return false;
        };
        match (&touch.old, touch.operation.as_str()) {
            (Some(old), "update") => old
                .get(name.as_str())
                .map_or(!value.is_null(), |before| text(before) != text(value)),
            _ => !value.is_null(),
        }
    });
    if !changed {
        return false;
    }
    // A field the write does not set is left to the document query.
    target.conditions.iter().all(|(name, wanted)| {
        fields
            .get(name.as_str())
            .filter(|value| !value.is_null())
            .is_none_or(|value| &text(value) == wanted)
    })
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
    // As in Java's `EntityDataFeed`, a feed that fails is logged and the business transaction still commits. Every
    // statement runs in its own savepoint, so only a savepoint that cannot be rolled back stops the commit.
    tx.on_feed_insert(move |conn| match insert_pending(&insert_facade, conn, frame) {
        Ok(ids) => {
            insert_ids.lock().unwrap_or_else(|err| err.into_inner()).extend(ids);
            Ok(())
        }
        Err(err) if is_unusable(&err) => Err(err),
        Err(err) => {
            tracing::error!(
                "Data feed pending rows were not recorded, the transaction commits without them: {}",
                error_chain(&err)
            );
            Ok(())
        }
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
    // the agent that made every write of a document's record, else none (the agent triggers' loop guard)
    let mut origins: HashMap<(String, String), Option<String>> = HashMap::new();
    let mut note_origin = |key: &(String, String), origin: &Option<String>| {
        origins
            .entry(key.clone())
            .and_modify(|known| {
                if known != origin {
                    *known = None;
                }
            })
            .or_insert_with(|| origin.clone());
    };
    for touch in &touches {
        let Some(fields) = touch.fields.as_deref() else {
            continue;
        };
        for hit in targets.iter().filter(|target| target.entity_name == touch.entity_name) {
            if !touch_feeds(hit, touch, fields) {
                continue;
            }
            if hit.primary {
                let Some(id) = pk_text(facade, &touch.entity_name, fields) else {
                    continue;
                };
                let operation = if touch.operation == "delete" { "delete" } else { "index" };
                let key = (hit.data_document_id.clone(), id);
                note_origin(&key, &touch.origin_agent);
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
                        note_origin(&key, &touch.origin_agent);
                        if !resolved.contains_key(&key) {
                            resolved.insert(key, (hit.primary_entity.clone(), "index"));
                        }
                    }
                }
            }
        }
    }
    for ((document_id, id), (entity_name, operation)) in &resolved {
        let row_id = crate::entity::uuid_v4()?;
        let origin = origins
            .get(&(document_id.clone(), id.clone()))
            .cloned()
            .flatten()
            .map_or(Value::Null, Value::String);
        let params = vec![
            Value::string(&row_id),
            Value::string(document_id),
            Value::string(entity_name),
            Value::string(id),
            Value::string(*operation),
            origin,
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
                let row_id = crate::entity::uuid_v4()?;
                let params = vec![
                    Value::string(&row_id),
                    Value::string(&hit.data_document_id),
                    Value::string(entity_name),
                    Value::string(&key),
                    Value::string(operation),
                    legacy_origin(&touches, entity_name).map_or(Value::Null, Value::String),
                ];
                probe_execute(conn, &insert_sql, &params)?;
                inserted.push(row_id);
            }
        }
    }
    Ok(inserted)
}

/// The agent that made every value-less write of `entity_name` in the frame, else `None`.
fn legacy_origin(touches: &[Touch], entity_name: &str) -> Option<String> {
    let mut origins = touches
        .iter()
        .filter(|touch| touch.entity_name == entity_name && touch.fields.is_none())
        .map(|touch| touch.origin_agent.as_deref());
    let first = origins.next()??;
    origins.all(|origin| origin == Some(first)).then(|| first.to_string())
}

/// Primary key values joined with `::`, or `None` when one is missing.
fn pk_text(facade: &EntityFacade, entity_name: &str, fields: &FieldMap) -> Option<String> {
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
        let text = scalar_text(fields.get(name.as_str())?)?;
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
    fields: &FieldMap,
) -> Result<Option<FieldMap>, Error> {
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
        match fields.get(name.as_str()) {
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
) -> Result<Vec<FieldMap>, Error> {
    let columns: Vec<&str> = names.iter().map(String::as_str).collect();
    let (sql, params) = select_sql(facade, entity_name, &columns, Some(condition))?;
    let rows = probe_query(conn, &sql, &params)?;
    Ok(rows
        .iter()
        .map(|row| {
            names
                .iter()
                .map(|name| (Arc::from(name.as_str()), cell_value(row, name).cloned().unwrap_or(Value::Null)))
                .collect()
        })
        .collect())
}

/// Walk relationship steps back from rows of the last entity to the primary entity's ids.
/// How many related keys one query of [`walk_to_primary`] looks up.
const RELATED_BATCH: usize = 200;

fn walk_to_primary(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    path: &[(String, String)],
    rows: Vec<FieldMap>,
) -> Result<Vec<String>, Error> {
    let mut rows = rows;
    for (parent, relationship) in path.iter().rev() {
        let (_, pairs) = crate::entity::query_rest::relationship_pairs(facade, parent, relationship)?;
        let names = facade.with_entity(parent, |entity, _, _| {
            Ok(entity.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>())
        })?;
        // The distinct keys of the parent rows, looked up up to 200 at a time.
        let mut keys: Vec<Vec<Value>> = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows {
            let key: Option<Vec<Value>> = pairs
                .iter()
                .map(|(_, related)| row.get(related.as_str()).filter(|value| !value.is_null()).cloned())
                .collect();
            let Some(key) = key.filter(|key| !key.is_empty()) else {
                continue;
            };
            if seen.insert(format!("{key:?}")) {
                keys.push(key);
            }
        }
        let mut next = Vec::new();
        for batch in keys.chunks(RELATED_BATCH) {
            let condition = if pairs.len() == 1 {
                Condition::is_in(
                    FieldRef::new(pairs[0].0.clone()),
                    batch.iter().map(|key| key[0].clone()).collect(),
                )
            } else {
                Condition::or(
                    batch
                        .iter()
                        .map(|key| {
                            Condition::and(
                                pairs
                                    .iter()
                                    .zip(key)
                                    .map(|((field, _), value)| {
                                        Condition::equals(FieldRef::new(field.clone()), value.clone())
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            };
            next.extend(select_rows(facade, conn, parent, &names, condition)?);
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
    let mut known = HashSet::new();
    for row in &rows {
        if let Some(id) = pk_text(facade, &primary, row) {
            if known.insert(id.clone()) {
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
        &["dataDocumentId", "fieldPath", "fieldNameAlias"],
        None,
    )?;
    let fields = probe_query(conn, &field_sql, &field_params)?;
    let (condition_sql, condition_params) = select_sql(
        facade,
        "moqui.entity.document.DataDocumentCondition",
        &["dataDocumentId", "fieldNameAlias", "fieldValue"],
        None,
    )?;
    let condition_rows = probe_query(conn, &condition_sql, &condition_params)?;
    let mut paths: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for row in &fields {
        let document = cell_string(row, "dataDocumentId");
        let path = cell_string(row, "fieldPath");
        if document.is_empty() || path.is_empty() || path.contains('(') {
            continue;
        }
        paths
            .entry(document)
            .or_default()
            .push((path, cell_string(row, "fieldNameAlias")));
    }
    let mut conditions: HashMap<(String, String), String> = HashMap::new();
    for row in &condition_rows {
        conditions.insert(
            (cell_string(row, "dataDocumentId"), cell_string(row, "fieldNameAlias")),
            cell_string(row, "fieldValue"),
        );
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
        let document_paths: Vec<(String, String)> = paths.get(&document_id).cloned().unwrap_or_default();
        let field_paths: Vec<String> = document_paths.iter().map(|(path, _)| path.clone()).collect();
        push_target(
            &mut targets,
            &mut seen,
            &document_id,
            &primary_full,
            &primary_full,
            None,
        );
        for (name, steps) in related_entities(facade, &primary_full, &field_paths) {
            push_target(&mut targets, &mut seen, &document_id, &name, &primary_full, Some(steps));
        }
        for (entity_name, names, entity_conditions) in
            document_fields(facade, &primary_full, &document_id, &document_paths, &conditions)
        {
            if let Some(target) = targets
                .iter_mut()
                .find(|target| target.data_document_id == document_id && target.entity_name == entity_name)
            {
                target.fields.extend(names);
                target.conditions.extend(entity_conditions);
            }
        }
    }
    Ok(targets)
}

/// An entity of a document, its fields, and the conditions on them.
type EntityFields = (String, HashSet<String>, Vec<(String, String)>);

/// Java's `getDataDocumentEntityInfo`: walking each field path from the primary entity, the fields of each entity
/// that make its documents change (the field at the end of a path, and the key fields of each relationship taken
/// from an entity), and the conditions on those fields, by the alias of the document field.
fn document_fields(
    facade: &EntityFacade,
    primary: &str,
    document_id: &str,
    paths: &[(String, String)],
    conditions: &HashMap<(String, String), String>,
) -> Vec<EntityFields> {
    let mut out: Vec<EntityFields> = Vec::new();
    let mut add = |entity: &str, field: &str, condition: Option<(String, String)>| {
        let index = match out.iter().position(|(name, _, _)| name == entity) {
            Some(index) => index,
            None => {
                out.push((entity.to_string(), HashSet::new(), Vec::new()));
                out.len() - 1
            }
        };
        out[index].1.insert(field.to_string());
        if let Some(condition) = condition {
            out[index].2.push(condition);
        }
    };
    for (path, alias) in paths {
        let parts: Vec<&str> = path.split(':').collect();
        let Some((field, relationships)) = parts.split_last() else {
            continue;
        };
        let mut current = primary.to_string();
        let mut reached = true;
        for rel_name in relationships {
            let step = facade
                .with_entity(&current, |entity, _, _| {
                    Ok(entity
                        .relationship(rel_name)
                        .map(|rel| (rel.related.clone(), rel.key_pairs.clone())))
                })
                .ok()
                .flatten();
            let Some((related, pairs)) = step else {
                reached = false;
                break;
            };
            for (key, _) in &pairs {
                add(&current, key, None);
            }
            let Ok(full) = canonical_name(facade, &related) else {
                reached = false;
                break;
            };
            current = full;
        }
        if reached {
            let condition = (!alias.is_empty())
                .then(|| conditions.get(&(document_id.to_string(), alias.clone())))
                .flatten()
                .map(|value| ((*field).to_string(), value.clone()));
            add(&current, field, condition);
        }
    }
    out
}

/// Add a target, or add a path to an existing one. `steps` is `None` for the primary entity.
fn push_target(
    targets: &mut Vec<DocTarget>,
    seen: &mut HashSet<(String, String)>,
    document_id: &str,
    entity_name: &str,
    primary_entity: &str,
    steps: Option<Vec<(String, String)>>,
) {
    if seen.insert((document_id.to_string(), entity_name.to_string())) {
        targets.push(DocTarget {
            data_document_id: document_id.to_string(),
            entity_name: entity_name.to_string(),
            primary: steps.is_none(),
            primary_entity: primary_entity.to_string(),
            paths: steps.into_iter().collect(),
            fields: HashSet::new(),
            conditions: Vec::new(),
        });
    } else if let Some(steps) = steps {
        if let Some(target) = targets
            .iter_mut()
            .find(|target| target.data_document_id == document_id && target.entity_name == entity_name)
        {
            if !target.paths.contains(&steps) {
                target.paths.push(steps);
            }
        }
    }
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
                "originAgentId",
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

/// A savepoint that could not be rolled back: the transaction is unusable and must not commit.
#[derive(Debug)]
struct Unusable(Error);

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Unusable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn unusable(undo: Error) -> Error {
    Error::new(format!(
        "could not roll back savepoint {SAVEPOINT}, the transaction is unusable: {undo}"
    ))
    .with_source(Unusable(undo))
}

fn is_unusable(err: &Error) -> bool {
    let mut current = Some(err as &dyn std::error::Error);
    while let Some(item) = current {
        if item.is::<Unusable>() {
            return true;
        }
        current = item.source();
    }
    false
}

/// Runs `run` inside a savepoint. A missing table gives `missing`, once the savepoint is rolled
/// back; if that rollback fails the transaction is unusable, so that error is returned.
fn probe<T>(
    conn: &mut dyn DbConnection,
    missing: T,
    run: impl FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
) -> Result<T, Error> {
    conn.savepoint(SAVEPOINT).map_err(unusable)?;
    match run(conn) {
        Ok(value) => {
            conn.release_savepoint(SAVEPOINT).map_err(unusable)?;
            Ok(value)
        }
        Err(err) => {
            let undone = conn
                .rollback_to_savepoint(SAVEPOINT)
                .and_then(|()| conn.release_savepoint(SAVEPOINT));
            match undone {
                Err(undo) => {
                    tracing::warn!("Could not roll back savepoint {SAVEPOINT} after a failed probe: {err}");
                    Err(unusable(undo))
                }
                Ok(()) if relation_missing(&err) => Ok(missing),
                Ok(()) => Err(err),
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

#[cfg(test)]
mod filter_tests {
    use std::collections::HashSet;

    use crate::entity::FieldMap;

    use moqui_lib::object::Value;

    use super::{touch_feeds, DocTarget, Touch};

    fn map(pairs: &[(&str, &str)]) -> FieldMap {
        pairs
            .iter()
            .map(|(name, value)| ((*name).into(), Value::string(*value)))
            .collect()
    }

    /// Java's `dataFeedCheckAndRegister`: fed only when a document field is set or changed and no document condition
    /// is contradicted.
    #[test]
    fn a_write_is_fed_only_when_a_document_field_changes_and_the_conditions_hold() {
        let target = DocTarget {
            data_document_id: "D".to_string(),
            entity_name: "E".to_string(),
            primary: true,
            primary_entity: "E".to_string(),
            paths: Vec::new(),
            fields: HashSet::from(["name".to_string(), "kind".to_string()]),
            conditions: vec![("kind".to_string(), "Good".to_string())],
        };
        let touch = |operation: &str, fields: FieldMap, old: Option<FieldMap>| Touch {
            entity_name: "E".to_string(),
            operation: operation.to_string(),
            fields: Some(std::sync::Arc::new(fields)),
            old,
            origin_agent: None,
        };
        let feeds = |touch: &Touch| touch_feeds(&target, touch, touch.fields.as_ref().unwrap());
        assert!(feeds(&touch("create", map(&[("id", "1"), ("name", "a")]), None)));
        assert!(!feeds(&touch("create", map(&[("id", "1"), ("other", "x")]), None)));
        assert!(!feeds(&touch("create", map(&[("name", "a"), ("kind", "Bad")]), None)));
        let before = map(&[("id", "1"), ("name", "a"), ("other", "x")]);
        assert!(!feeds(&touch(
            "update",
            map(&[("id", "1"), ("name", "a"), ("other", "y")]),
            Some(before.clone())
        )));
        assert!(feeds(&touch(
            "update",
            map(&[("id", "1"), ("name", "b")]),
            Some(before)
        )));
        assert!(feeds(&touch("update", map(&[("id", "1"), ("name", "a")]), None)));
        assert!(feeds(&touch("delete", map(&[("id", "1")]), None)));
    }
}
