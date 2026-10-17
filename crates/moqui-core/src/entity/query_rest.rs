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

//! Dynamic view, entity data writer, and `EntityFacade.rest`.
//!
//! Parameters are an in-memory map. There is no HTTP listener.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moqui_db::model::{
    AliasAll, EntityDefinition, FieldAlias, KeyMap, MasterDefinition, MasterDetail, MemberEntity, PqAlias, Relationship,
};
use moqui_db::sql::{select_view, Condition, FieldRef, LimitOffset, OrderBy};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};

use super::exec::{after_statement, query_tx};
use super::facade::EntityFacade;
use super::field_rw;
use super::find::FindValues;
use super::params::{map_row, view_columns};
use super::sensitive;
use super::value::{EntityArtifact, EntityValue};
use crate::context::ErrorKind;

const MAX_DEPTH: u32 = 32;
/// The largest `pageSize` a REST find reads, unless the `entity_rest_max_page_size` property sets another.
const DEFAULT_MAX_PAGE_SIZE: i64 = 1_000;

/// Build a view from `parameters` and return its rows.
pub fn dynamic_view(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    if !has_members(parameters) {
        return named_view(facade, entity_name, parameters);
    }
    let built = build_view(facade, entity_name, parameters)?;
    run_view(facade, &built, true)
}

/// Write records for one entity. `directory` or `file`/`filename` on a record is the destination.
///
/// A directory with no `fileType` writes XML, JSON, and CSV. `fileType` selects one of those.
/// The shapes follow `EntityDataWriterImpl`.
pub fn write_data(facade: &EntityFacade, entity_name: &str, records: &[BTreeMap<String, Value>]) -> Result<(), Error> {
    let snap = load_snap(facade, entity_name)?;
    let targets = destinations(&snap.full_name, records)?;
    let levels = writer_levels(records);
    let master = first_text(records, "master").filter(|text| !text.is_empty());
    let prefix = first_text(records, "prefix").unwrap_or_default();
    let iso = records.iter().any(|record| flag_at(record, "isoDateTime"));
    let mut nodes = Vec::with_capacity(records.len());
    for record in records {
        nodes.push(render_record(
            facade,
            &snap.full_name,
            record,
            levels,
            master.as_deref(),
        )?);
    }
    let xml = render_xml(&nodes, &prefix, iso);
    let json = render_json(&nodes, iso);
    let csv = render_csv(&snap, records, iso);
    for (path, kind) in targets {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| Error::new(format!("could not create {}", parent.display())).with_source(err))?;
            }
        }
        if path.exists() {
            return Err(Error::new(format!("File {} already exists", path.display())));
        }
        let body = match kind {
            FileKind::Xml => xml.as_str(),
            FileKind::Json => json.as_str(),
            FileKind::Csv => csv.as_str(),
        };
        std::fs::write(&path, body)
            .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))?;
    }
    Ok(())
}

/// One record as the element `EntityValue.writeXmlText` writes inside an `<entity-facade-xml>` document, with its
/// dependents to `levels`.
pub fn xml_element(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
    levels: i64,
) -> Result<String, Error> {
    let node = render_record(facade, entity_name, fields, levels, None)?;
    let element = node.entity_key.replace('#', "-");
    let mut out = String::new();
    write_xml_node(&mut out, &node, &element, 1, "", false);
    Ok(out)
}

/// `EntityFacade.rest`. Operations are find, create, store, update, and delete.
///
/// `get`/`post`/`put`/`patch`/`delete` map the same way as `EntityFacadeImpl.rest`.
/// `path` is a list or a slash-separated string of elements after the entity name.
/// One relationship step is applied. `master`, `masterNameInPath`, `dependents`, and
/// `dependentLevels` follow the Java method.
/// Pagination numbers for the last entity REST list on this thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageInfo {
    /// Only `X-Total-Count` is sent, as for a Service REST count.
    pub count_only: bool,
    pub total: i64,
    pub index: i64,
    pub size: i64,
    pub max_index: i64,
    pub range_low: i64,
    pub range_high: i64,
}

thread_local! {
    static WANT_PAGE: Cell<bool> = const { Cell::new(false) };
    static PAGE: Cell<Option<PageInfo>> = const { Cell::new(None) };
}

/// The next list `rest` records page numbers. A one-row find does not.
pub fn begin_page_info() {
    WANT_PAGE.with(|slot| slot.set(true));
    PAGE.with(|slot| slot.set(None));
}

/// Take the page numbers recorded by the last list, and stop recording.
pub fn take_page_info() -> Option<PageInfo> {
    WANT_PAGE.with(|slot| slot.set(false));
    PAGE.with(|slot| slot.take())
}

fn want_page_info() -> bool {
    WANT_PAGE.with(|slot| slot.get())
}

/// Record the page numbers of a list, or only the total of a count, for the response headers of a REST call that
/// asked for them.
pub(crate) fn note_page_info(
    limit: Option<i64>,
    offset: Option<i64>,
    parameters: &BTreeMap<String, Value>,
    total: i64,
    count_only: bool,
) {
    if !want_page_info() {
        return;
    }
    let mut info = page_numbers(&Page { limit, offset }, parameters, total);
    info.count_only = count_only;
    PAGE.with(|slot| slot.set(Some(info)));
}

pub fn rest(
    facade: &EntityFacade,
    operation: &str,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let verb = normalize_operation(operation)?;
    let mut parameters = parameters.clone();
    let target = resolve_target(facade, entity_name, &mut parameters)?;
    // As Java's `EntityFacadeImpl.rest`, identity-admin and secret-config entities are not readable, identity-admin
    // and `UserAccount` are not writable, and secret-config is writable only by ADMIN. Results never carry the
    // `UserAccount` password fields.
    let full_name = facade
        .with_entity(&target.entity_name, |entity, _, _| Ok(entity.full_name.clone()))
        .unwrap_or_else(|_| target.entity_name.clone());
    if verb == Verb::Find {
        if sensitive::is_restricted_generic_entity(&full_name) {
            return Err(kinded(
                ErrorKind::Unauthorized,
                format!("Entity {full_name} is not available through generic entity REST"),
            ));
        }
    } else if sensitive::is_identity_admin_entity(&full_name)
        || sensitive::is_user_account_entity(&full_name)
        || (sensitive::is_secret_config_entity(&full_name) && !crate::user::is_in_group("ADMIN")?)
    {
        return Err(kinded(
            ErrorKind::Unauthorized,
            format!("Entity {full_name} is not writable through generic entity REST"),
        ));
    }
    // Java's generic entity REST never reads without a limit: with `pageNoLimit` it still takes 100.
    parameters.remove("pageNoLimit");
    let result = rest_target(facade, verb, &target, &parameters)?;
    Ok(sensitive::strip_user_account_secrets(result))
}

fn rest_target(
    facade: &EntityFacade,
    verb: Verb,
    target: &Resolved,
    parameters: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let operation = match verb {
        Verb::Find => return find_rest(facade, target, parameters),
        Verb::Create => "create",
        Verb::Store => "store",
        Verb::Update => "update",
        Verb::Delete => "delete",
    };
    write_rest(facade, operation, &target.entity_name, parameters)
}

/// Create, store, update and delete run the entity-auto service of the entity, as Java's `rest` does, so service
/// ECA rules, parameter validation and the status-flow check apply. As Java's entity REST handler, message errors
/// after the call are the error: those the call added are the caller's input; errors from before it mean it did not
/// run.
fn write_rest(
    facade: &EntityFacade,
    operation: &str,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let full_name = facade.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))?;
    let errors =
        || crate::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    let before = errors().len();
    let results = crate::service::call::sync(&format!("{operation}#{full_name}"), parameters)?;
    let after = errors();
    if !after.is_empty() {
        let kind = if before > 0 {
            ErrorKind::General
        } else {
            ErrorKind::Validation
        };
        return Err(kinded(kind, after.join("\n")));
    }
    Ok(value_map(results.into_iter().collect()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Find,
    Create,
    Store,
    Update,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Xml,
    Json,
    Csv,
}

impl FileKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Xml => "xml",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
}

struct EntitySnap {
    entity_name: String,
    full_name: String,
    short_alias: Option<String>,
    entity_key: String,
    /// Field names in definition order, or alias names for a view, as Java's `allFieldNameList`.
    fields: Vec<String>,
    pk_fields: Vec<String>,
    relationships: Vec<Relationship>,
    masters: Vec<MasterDefinition>,
}

impl EntitySnap {
    fn from_entity(entity: &EntityDefinition) -> Self {
        Self {
            entity_name: entity.entity_name.clone(),
            full_name: entity.full_name.clone(),
            entity_key: entity.short_alias.clone().unwrap_or_else(|| entity.full_name.clone()),
            short_alias: entity.short_alias.clone(),
            fields: entity.field_names().into_iter().map(str::to_string).collect(),
            pk_fields: entity.pk_field_names().into_iter().map(str::to_string).collect(),
            relationships: entity.relationships.clone(),
            masters: entity.masters.clone(),
        }
    }

    fn pk_names(&self) -> Vec<&str> {
        self.pk_fields.iter().map(String::as_str).collect()
    }

    fn label(&self) -> String {
        format!("{}:{}", self.short_alias.as_deref().unwrap_or(""), self.full_name)
    }

    fn master(&self, name: &str) -> Option<&MasterDefinition> {
        self.masters.iter().find(|master| master.name == name)
    }
}

struct Resolved {
    entity_name: String,
    master: Option<String>,
    levels: i64,
}

struct RecordNode {
    entity_key: String,
    fields: Vec<(String, Value)>,
    nested: Vec<NestedRecords>,
}

struct NestedRecords {
    name: String,
    one: bool,
    records: Vec<RecordNode>,
}

enum Expand<'a> {
    None,
    Dependents(i64),
    Master(&'a str),
    Details(&'a [MasterDetail]),
}

struct RenderCtx<'a> {
    expand: Expand<'a>,
    parent_pks: &'a HashSet<String>,
    /// Script master graphs must retain identities, including self-referential child primary keys.
    preserve_keys: bool,
    seen: &'a mut HashSet<String>,
    depth: u32,
}

struct BuiltView {
    view: EntityDefinition,
    condition: Option<Condition>,
    order: Vec<OrderBy>,
    page: LimitOffset,
}

struct Page {
    limit: Option<i64>,
    offset: Option<i64>,
}

fn normalize_operation(operation: &str) -> Result<Verb, Error> {
    if operation.is_empty() {
        return Err(Error::new("Operation (method) must be specified"));
    }
    match operation.to_ascii_lowercase().as_str() {
        "get" | "find" => Ok(Verb::Find),
        "post" | "create" => Ok(Verb::Create),
        "put" | "store" => Ok(Verb::Store),
        "patch" | "update" => Ok(Verb::Update),
        "delete" => Ok(Verb::Delete),
        _ => Err(kinded(ErrorKind::MethodNotAllowed, format!(
            "Operation [{operation}] not supported, must be one of: get, post, put, patch, or delete for HTTP request methods or find, create, store, update, or delete for direct entity operations"
        ))),
    }
}

fn resolve_target(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &mut BTreeMap<String, Value>,
) -> Result<Resolved, Error> {
    let (name, mut path) = split_path(entity_name, parameters);
    if name.is_empty() {
        return Err(kinded(
            ErrorKind::BadRequest,
            "No entity name or alias specified in path",
        ));
    }
    let first = load_snap(facade, &name).map_err(|err| kinded(ErrorKind::NotFound, err.message()))?;
    let mut master = optional_text(parameters, "master");
    if flag(parameters, "masterNameInPath") {
        if master.as_ref().is_none_or(String::is_empty) {
            if path.first().is_some_and(|element| first.master(element).is_some()) {
                master = Some(path.remove(0));
            } else {
                master = Some(String::from("default"));
            }
        }
        let master_name = master.as_deref().unwrap_or("default");
        if first.master(master_name).is_none() {
            return Err(Error::new(format!(
                "Master definition not found for entity [{}], tried master name [{master_name}]",
                first.full_name
            )));
        }
    }
    take_pk_path(&first, &mut path, parameters, false);
    let mut current = first;
    // As Java: each further element names a relationship, followed by the related entity's key values.
    while !path.is_empty() {
        let relationship_name = path.remove(0);
        let relationship = current
            .relationships
            .iter()
            .find(|relationship| relationship.matches(&relationship_name))
            .cloned()
            .ok_or_else(|| {
                kinded(
                    ErrorKind::NotFound,
                    format!(
                        "No relationship found with name or alias [{relationship_name}] on entity [{}]",
                        current.label()
                    ),
                )
            })?;
        let related =
            load_snap(facade, &relationship.related).map_err(|err| kinded(ErrorKind::NotFound, err.message()))?;
        take_pk_path(&related, &mut path, parameters, true);
        current = related;
    }
    Ok(Resolved {
        entity_name: current.full_name,
        master,
        levels: dependent_levels(parameters),
    })
}

fn split_path(entity_name: &str, parameters: &BTreeMap<String, Value>) -> (String, Vec<String>) {
    let mut parts = entity_name.split('/').map(str::to_string);
    let name = parts.next().unwrap_or_default();
    let mut path: Vec<String> = parts.filter(|part| !part.is_empty()).collect();
    if let Some(extra) = parameters.get("path") {
        match extra {
            Value::List(items) => path.extend(items.iter().map(text_of)),
            Value::String(text) => path.extend(text.split('/').filter(|part| !part.is_empty()).map(str::to_string)),
            other => path.push(text_of(other)),
        }
    }
    (name, path)
}

fn take_pk_path(
    snap: &EntitySnap,
    path: &mut Vec<String>,
    parameters: &mut BTreeMap<String, Value>,
    skip_existing: bool,
) {
    for name in snap.pk_names() {
        if path.is_empty() {
            break;
        }
        if skip_existing && parameters.contains_key(name) {
            continue;
        }
        let value = path.remove(0);
        if !value.is_empty() {
            parameters.insert(name.to_string(), Value::string(value));
        }
    }
}

fn find_rest(facade: &EntityFacade, target: &Resolved, parameters: &BTreeMap<String, Value>) -> Result<Value, Error> {
    let snap = load_snap(facade, &target.entity_name)?;
    if pk_complete(&snap, parameters) {
        let mut find = facade.find(&snap.full_name)?;
        for name in snap.pk_names() {
            if let Some(value) = parameters.get(name) {
                find = find.eq(name, value.clone());
            }
        }
        let found = find.one()?.ok_or_else(|| {
            kinded(
                ErrorKind::NotFound,
                format!(
                    "No value found for entity [{}] with key {}",
                    snap.label(),
                    key_text(&snap, parameters)
                ),
            )
        })?;
        let node = render_record(
            facade,
            found.entity_name(),
            found.fields(),
            target.levels,
            target.master.as_deref(),
        )?;
        return Ok(node_to_value(&node));
    }
    let find = list_find(facade, &snap.full_name, parameters)?;
    let page = Page {
        limit: find.limit_value(),
        offset: find.offset_value(),
    };
    let counter = want_page_info().then(|| find.clone());
    let rows = find.list()?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let node = render_record(
            facade,
            row.entity_name(),
            row.fields(),
            target.levels,
            target.master.as_deref(),
        )?;
        items.push(node_to_value(&node));
    }
    if let Some(counter) = counter {
        if let Ok(total) = counter.count() {
            PAGE.with(|slot| slot.set(Some(page_numbers(&page, parameters, total))));
        }
    }
    Ok(Value::List(items))
}

/// As Java's list find: the search form conditions, order and paging of the parameters, and never more than 100 rows
/// when no page is asked for. This port also caps a page asked for at `entity_rest_max_page_size` (1000 by default).
fn list_find(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<super::find::EntityFind, Error> {
    let find = facade
        .find(entity_name)?
        .search_form_map(parameters, &BTreeMap::new(), "", "", false)?
        .find;
    Ok(match find.limit_value() {
        None => find.limit(100),
        Some(limit) if limit > max_page_size() => {
            let index = parameters.get("pageIndex").and_then(value_i64).unwrap_or(0).max(0);
            find.offset(index.saturating_mul(max_page_size()))
                .limit(max_page_size())
        }
        Some(_) => find,
    })
}

fn page_numbers(page: &Page, parameters: &BTreeMap<String, Value>, total: i64) -> PageInfo {
    let index = parameters
        .get("pageIndex")
        .and_then(value_i64)
        .unwrap_or_else(|| match (page.offset, page.limit) {
            (Some(offset), Some(limit)) if limit > 0 => offset / limit,
            _ => 0,
        })
        .max(0);
    let size = page.limit.unwrap_or(total.max(0));
    let divisor = if size > 0 { size } else { 1 };
    let max_index = if total <= 0 { 0 } else { (total - 1) / divisor };
    let range_low = if total <= 0 {
        0
    } else {
        index.saturating_mul(divisor).saturating_add(1)
    };
    let range_high = index.saturating_mul(divisor).saturating_add(size).min(total.max(0));
    PageInfo {
        count_only: false,
        total,
        index,
        size,
        max_index,
        range_low,
        range_high,
    }
}

fn max_page_size() -> i64 {
    moqui_lib::env::get_prop_or_env("entity_rest_max_page_size")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_MAX_PAGE_SIZE)
}

fn pk_complete(snap: &EntitySnap, parameters: &BTreeMap<String, Value>) -> bool {
    let names = snap.pk_names();
    !names.is_empty()
        && names
            .iter()
            .all(|name| parameters.get(*name).is_some_and(|value| !is_empty(value)))
}

fn key_text(snap: &EntitySnap, parameters: &BTreeMap<String, Value>) -> String {
    let mut parts = Vec::new();
    for name in snap.pk_names() {
        let shown = parameters.get(name).map(text_of).unwrap_or_default();
        parts.push(format!("{name}={shown}"));
    }
    parts.join(", ")
}

/// A master graph for a script find, using the same expansion and authorization as entity REST.
pub(crate) fn master_value(facade: &EntityFacade, record: &EntityValue, master: &str) -> Result<Value, Error> {
    let mut seen = HashSet::new();
    let parent_pks = HashSet::new();
    let mut ctx = RenderCtx {
        expand: Expand::Master(master),
        parent_pks: &parent_pks,
        preserve_keys: true,
        seen: &mut seen,
        depth: 0,
    };
    render(facade, record.entity_name(), record.fields(), &mut ctx).map(|node| node_to_value(&node))
}

/// Flatten a plain master graph to typed values, inheriting primary keys and relationship key mappings.
/// As in Java, read-only relationships and unrelated metadata maps are not persisted.
pub(crate) fn values_from_plain_map(
    facade: &EntityFacade,
    map: &indexmap::IndexMap<String, Value>,
    entity: &str,
) -> Result<Vec<EntityValue>, Error> {
    fn add(
        facade: &EntityFacade,
        map: &indexmap::IndexMap<String, Value>,
        entity: &str,
        inherited: &BTreeMap<String, Value>,
        depth: u32,
        out: &mut Vec<EntityValue>,
    ) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::new("Entity graph exceeds maximum depth"));
        }
        let (full_name, fields, pks, relationships) = facade.with_entity(entity, |ed, _, _| {
            Ok((
                ed.full_name.clone(),
                ed.field_names().into_iter().map(str::to_string).collect::<Vec<_>>(),
                ed.pk_field_names().into_iter().map(str::to_string).collect::<Vec<_>>(),
                ed.relationships.clone(),
            ))
        })?;
        let mut row = facade.make_value(&full_name)?;
        for field in &fields {
            if let Some(value) = map.get(field).or_else(|| inherited.get(field)) {
                row.set(field, value.clone());
            }
        }
        let mut shared = inherited.clone();
        for pk in &pks {
            if let Some(value) = row.get(pk) {
                shared.insert(pk.clone(), value.clone());
            }
        }
        let row_index = out.len();
        out.push(row);
        for (name, value) in map {
            if fields.contains(name) {
                continue;
            }
            let Some(rel) = relationships
                .iter()
                .find(|rel| rel.name == *name || rel.short_alias.as_deref() == Some(name))
            else {
                continue;
            };
            if !rel.mutable {
                continue;
            }
            let mut keys = shared.clone();
            for (source, target) in &rel.key_pairs {
                if let Some(value) = out[row_index].get(source) {
                    keys.insert(target.clone(), value.clone());
                }
            }
            match value {
                Value::Map(child) => add(facade, child, &rel.related, &keys, depth + 1, out)?,
                Value::List(children) => {
                    for child in children {
                        if let Value::Map(child) = child {
                            add(facade, child, &rel.related, &keys, depth + 1, out)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    add(facade, map, entity, &BTreeMap::new(), 0, &mut out)?;
    Ok(out)
}

fn render_record(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
    levels: i64,
    master: Option<&str>,
) -> Result<RecordNode, Error> {
    let parent = HashSet::new();
    let mut seen = HashSet::new();
    let expand = match master {
        Some(name) => Expand::Master(name),
        None if levels > 0 => Expand::Dependents(levels),
        None => Expand::None,
    };
    let mut ctx = RenderCtx {
        expand,
        parent_pks: &parent,
        preserve_keys: false,
        seen: &mut seen,
        depth: 0,
    };
    render(facade, entity_name, fields, &mut ctx)
}

fn render(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
    ctx: &mut RenderCtx<'_>,
) -> Result<RecordNode, Error> {
    let snap = load_snap(facade, entity_name)?;
    let mut node = RecordNode {
        entity_key: snap.entity_key.clone(),
        fields: Vec::new(),
        nested: Vec::new(),
    };
    for name in &snap.fields {
        if !ctx.preserve_keys && ctx.parent_pks.contains(name) {
            continue;
        }
        let Some(value) = fields.get(name) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        node.fields.push((name.clone(), value.clone()));
    }
    let seen_id = seen_key(&snap, fields);
    if ctx.depth >= MAX_DEPTH || !ctx.seen.insert(seen_id) {
        return Ok(node);
    }
    attach(facade, &snap, fields, &mut node, ctx)?;
    Ok(node)
}

fn seen_key(snap: &EntitySnap, fields: &BTreeMap<String, Value>) -> String {
    let mut key = snap.full_name.clone();
    for name in snap.pk_names() {
        key.push('\u{1f}');
        if let Some(value) = fields.get(name) {
            key.push_str(&text_of(value));
        }
    }
    key
}

fn attach(
    facade: &EntityFacade,
    snap: &EntitySnap,
    fields: &BTreeMap<String, Value>,
    node: &mut RecordNode,
    ctx: &mut RenderCtx<'_>,
) -> Result<(), Error> {
    match &ctx.expand {
        Expand::None => Ok(()),
        Expand::Dependents(levels) if *levels <= 0 => Ok(()),
        Expand::Dependents(levels) => {
            let levels = *levels;
            attach_dependents(facade, snap, fields, node, levels, ctx)
        }
        Expand::Master(name) => {
            let name = (*name).to_string();
            let details = snap.master(&name).map(|master| master.details.clone()).ok_or_else(|| {
                Error::new(format!(
                    "No master definition found for name [{name}] on entity [{}]",
                    snap.full_name
                ))
            })?;
            attach_detail_list(facade, snap, fields, node, &details, ctx)
        }
        Expand::Details(details) => {
            let details = details.to_vec();
            attach_detail_list(facade, snap, fields, node, &details, ctx)
        }
    }
}

fn attach_dependents(
    facade: &EntityFacade,
    snap: &EntitySnap,
    fields: &BTreeMap<String, Value>,
    node: &mut RecordNode,
    levels: i64,
    ctx: &mut RenderCtx<'_>,
) -> Result<(), Error> {
    let relationships = snap.relationships.clone();
    for relationship in relationships {
        if !is_dependent(facade, snap, &relationship)? {
            continue;
        }
        push_related(
            facade,
            snap,
            fields,
            node,
            &relationship,
            Expand::Dependents(levels - 1),
            ctx,
        )?;
    }
    Ok(())
}

fn attach_detail_list(
    facade: &EntityFacade,
    snap: &EntitySnap,
    fields: &BTreeMap<String, Value>,
    node: &mut RecordNode,
    details: &[MasterDetail],
    ctx: &mut RenderCtx<'_>,
) -> Result<(), Error> {
    for detail in details {
        let Some(relationship) = snap
            .relationships
            .iter()
            .find(|relationship| relationship.matches(&detail.relationship))
            .cloned()
        else {
            return Err(Error::new(format!(
                "No relationship found with name or alias [{}] on entity [{}]",
                detail.relationship,
                snap.label()
            )));
        };
        let combined = combined_details(facade, &relationship, detail)?;
        push_related(
            facade,
            snap,
            fields,
            node,
            &relationship,
            Expand::Details(&combined),
            ctx,
        )?;
    }
    Ok(())
}

fn combined_details(
    facade: &EntityFacade,
    relationship: &Relationship,
    detail: &MasterDetail,
) -> Result<Vec<MasterDetail>, Error> {
    let mut combined = detail.details.clone();
    if let Some(name) = &detail.use_master {
        let related = load_snap(facade, &relationship.related)?;
        let master = related.master(name).cloned().ok_or_else(|| {
            Error::new(format!(
                "Invalid use-master value [{name}], master not found on {}",
                related.full_name
            ))
        })?;
        combined.extend(master.details);
    }
    Ok(combined)
}

fn push_related(
    facade: &EntityFacade,
    snap: &EntitySnap,
    fields: &BTreeMap<String, Value>,
    node: &mut RecordNode,
    relationship: &Relationship,
    expand: Expand<'_>,
    ctx: &mut RenderCtx<'_>,
) -> Result<(), Error> {
    let one = relationship.relationship_type.starts_with("one");
    let rows = related_rows(facade, relationship, fields, one)?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut next_pks = ctx.parent_pks.clone();
    for name in snap.pk_names() {
        next_pks.insert(name.to_string());
    }
    let mut records = Vec::new();
    for row in rows {
        let child_name = row.entity_name().to_string();
        let child_fields = row.fields().clone();
        let depth = ctx.depth + 1;
        let mut child_ctx = RenderCtx {
            expand: match &expand {
                Expand::None => Expand::None,
                Expand::Dependents(levels) => Expand::Dependents(*levels),
                Expand::Master(name) => Expand::Master(name),
                Expand::Details(details) => Expand::Details(details),
            },
            parent_pks: &next_pks,
            preserve_keys: ctx.preserve_keys,
            seen: ctx.seen,
            depth,
        };
        records.push(render(facade, &child_name, &child_fields, &mut child_ctx)?);
    }
    node.nested.push(NestedRecords {
        name: relation_entry(relationship),
        one,
        records,
    });
    Ok(())
}

fn is_dependent(facade: &EntityFacade, source: &EntitySnap, relationship: &Relationship) -> Result<bool, Error> {
    let related = load_snap(facade, &relationship.related)?;
    Ok(related.relationships.iter().any(|back| {
        let one = back.relationship_type == "one" || back.relationship_type == "one-nofk";
        let points_here = back.related == source.full_name || back.related_entity_name == source.entity_name;
        one && back.title == relationship.title && points_here
    }))
}

fn relation_entry(relationship: &Relationship) -> String {
    relationship
        .short_alias
        .clone()
        .filter(|alias| !alias.is_empty())
        .unwrap_or_else(|| relationship.name.clone())
}

fn related_rows(
    facade: &EntityFacade,
    relationship: &Relationship,
    source_fields: &BTreeMap<String, Value>,
    one: bool,
) -> Result<Vec<EntityValue>, Error> {
    let target = load_snap(facade, &relationship.related)?;
    let pairs = relationship.key_pairs.clone();
    let mut find = facade.find(&target.full_name)?;
    let mut bound = false;
    for (source_name, target_name) in pairs {
        let Some(value) = source_fields.get(&source_name) else {
            continue;
        };
        if is_empty(value) {
            continue;
        }
        find = find.eq(&target_name, value.clone());
        bound = true;
    }
    for key_value in &relationship.key_values {
        find = find.eq(&key_value.related, Value::string(&key_value.value));
        bound = true;
    }
    if !bound {
        return Ok(Vec::new());
    }
    if one {
        Ok(find.one()?.into_iter().collect())
    } else {
        Ok(find.list()?.values().to_vec())
    }
}

/// The related entity's full name and the `(field, related field)` pairs of a relationship.
pub(crate) fn relationship_pairs(
    facade: &EntityFacade,
    entity_name: &str,
    relationship_name: &str,
) -> Result<(String, Vec<(String, String)>), Error> {
    let relationship = facade.with_entity(entity_name, |entity, _, _| {
        entity.relationship(relationship_name).cloned().ok_or_else(|| {
            Error::new(format!(
                "No relationship found with name or alias [{relationship_name}] on entity [{}]",
                entity.full_name
            ))
        })
    })?;
    let target = load_snap(facade, &relationship.related)?;
    Ok((target.full_name, relationship.key_pairs))
}

fn node_to_value(node: &RecordNode) -> Value {
    let mut entries = Vec::new();
    for (name, value) in &node.fields {
        entries.push((name.clone(), value.clone()));
    }
    entries.push(("_entity".to_string(), Value::string(&node.entity_key)));
    for nested in &node.nested {
        if nested.records.is_empty() {
            continue;
        }
        if nested.one {
            entries.push((nested.name.clone(), node_to_value(&nested.records[0])));
        } else {
            let items = nested.records.iter().map(node_to_value).collect();
            entries.push((nested.name.clone(), Value::List(items)));
        }
    }
    value_map(entries)
}

fn has_members(parameters: &BTreeMap<String, Value>) -> bool {
    param(parameters, &["member-entity", "memberEntity"]).is_some()
        || param(parameters, &["member-relationship", "memberRelationship"]).is_some()
}

fn named_view(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    let name = param(parameters, &["view-entity", "viewEntity"])
        .map(text_of)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| entity_name.to_string());
    let is_view = match facade.with_entity(&name, |entity, _, _| Ok(entity.is_view)) {
        Ok(is_view) => is_view,
        Err(_) => {
            return Err(Error::new(format!("dynamic view {entity_name} needs a member-entity")));
        }
    };
    if !is_view {
        return Err(Error::new(format!("dynamic view {entity_name} needs a member-entity")));
    }
    let filters = facade.with_entity(&name, |entity, _, _| Ok(named_filters(entity, parameters)))?;
    let mut find = facade.find(&name)?;
    for (field, value) in filters {
        find = find.eq(&field, value);
    }
    if let Some(value) = param(parameters, &["order-by", "orderBy", "orderByField"]) {
        for entry in parse_order(value) {
            find = find.order_by_entry(entry);
        }
    }
    let page = explicit_page(parameters);
    if let Some(limit) = page.limit {
        find = find.limit(limit);
    }
    if let Some(offset) = page.offset {
        find = find.offset(offset);
    }
    let rows = find.list()?;
    Ok(rows.iter().map(|row| row.fields().clone()).collect())
}

fn named_filters(entity: &EntityDefinition, parameters: &BTreeMap<String, Value>) -> Vec<(String, Value)> {
    let mut names = Vec::new();
    for field in &entity.fields {
        names.push(field.name.clone());
    }
    for alias in &entity.aliases {
        if alias.function.is_none() {
            names.push(alias.name.clone());
        }
    }
    let mut filters = Vec::new();
    for name in names {
        if is_control(&name) {
            continue;
        }
        if let Some(value) = parameters.get(&name) {
            if !is_empty(value) {
                filters.push((name, value.clone()));
            }
        }
    }
    filters
}

fn build_view(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BuiltView, Error> {
    let mut members = Vec::new();
    let mut conditions = Vec::new();
    if let Some(value) = param(parameters, &["member-entity", "memberEntity"]) {
        for spec in map_list(value) {
            members.push(member_from_spec(facade, &spec)?);
        }
    }
    if let Some(value) = param(parameters, &["member-relationship", "memberRelationship"]) {
        for spec in map_list(value) {
            add_relationship_member(facade, &mut members, &mut conditions, &spec)?;
        }
    }
    if members.is_empty() {
        return Err(Error::new(format!("dynamic view {entity_name} needs a member-entity")));
    }
    let mut aliases = Vec::new();
    let mut pq_aliases = Vec::new();
    if let Some(value) = param(parameters, &["alias"]) {
        for spec in map_list(value) {
            match pq_alias_from_spec(&spec)? {
                Some(pq_alias) => pq_aliases.push(pq_alias),
                None => aliases.push(alias_from_spec(&spec)?),
            }
        }
    }
    let mut alias_alls = Vec::new();
    if let Some(value) = param(parameters, &["alias-all", "aliasAll"]) {
        for spec in map_list(value) {
            alias_alls.push(alias_all_from_spec(&spec)?);
        }
    }
    if let Some(value) = parameters.get("condition") {
        conditions.extend(parse_conditions(value));
    }
    let order = param(parameters, &["order-by", "orderBy", "orderByField"])
        .map(parse_order)
        .unwrap_or_default();
    let page = explicit_page(parameters);
    let mut view = blank_view(entity_name);
    view.member_entities = members;
    view.aliases = aliases;
    view.pq_aliases = pq_aliases;
    view.alias_alls = alias_alls;
    Ok(BuiltView {
        view,
        condition: and_all(conditions),
        order,
        page: LimitOffset {
            limit: page.limit,
            offset: page.offset,
        },
    })
}

fn member_from_spec(facade: &EntityFacade, spec: &Value) -> Result<MemberEntity, Error> {
    let entity_alias = req_string(spec, &["entity-alias", "entityAlias"], "member-entity entity-alias")?;
    let entity_name = req_string(spec, &["entity-name", "entityName"], "member-entity entity-name")?;
    let full_name = load_snap(facade, &entity_name)?.full_name;
    let key_map = map_get(spec, &["key-map", "keyMap"])
        .map(parse_key_map)
        .unwrap_or_default();
    Ok(MemberEntity {
        join_from_alias: optional_string(spec, &["join-from-alias", "joinFromAlias"]),
        join_optional: map_get(spec, &["join-optional", "joinOptional"]).is_some_and(flag_value),
        key_map,
        ..MemberEntity::new(entity_alias, full_name)
    })
}

/// `select_view` joins `member-entity` only, so a relationship member is expanded here.
fn add_relationship_member(
    facade: &EntityFacade,
    members: &mut Vec<MemberEntity>,
    conditions: &mut Vec<Condition>,
    spec: &Value,
) -> Result<(), Error> {
    let entity_alias = req_string(
        spec,
        &["entity-alias", "entityAlias"],
        "member-relationship entity-alias",
    )?;
    let join_from = req_string(
        spec,
        &["join-from-alias", "joinFromAlias"],
        "member-relationship join-from-alias",
    )?;
    let relationship_name = req_string(spec, &["relationship"], "member-relationship relationship")?;
    let optional = map_get(spec, &["join-optional", "joinOptional"]).is_some_and(flag_value);
    push_relationship_member(
        facade,
        members,
        conditions,
        &entity_alias,
        &join_from,
        &relationship_name,
        optional,
    )
}

/// Join `relationship` from the member `join_from` as `entity_alias`. A key value on the
/// relationship becomes a condition.
fn push_relationship_member(
    facade: &EntityFacade,
    members: &mut Vec<MemberEntity>,
    conditions: &mut Vec<Condition>,
    entity_alias: &str,
    join_from: &str,
    relationship_name: &str,
    optional: bool,
) -> Result<(), Error> {
    let entity_alias = entity_alias.to_string();
    let join_from = join_from.to_string();
    let source = members
        .iter()
        .find(|member| member.entity_alias == join_from)
        .cloned()
        .ok_or_else(|| {
            Error::new(format!(
                "member-relationship {entity_alias} join-from-alias {join_from} was not found"
            ))
        })?;
    let relationship = facade.with_entity(&source.entity_name, |entity, _, _| {
        entity.relationship(relationship_name).cloned().ok_or_else(|| {
            Error::new(format!(
                "No relationship found with name or alias [{relationship_name}] on entity [{}:{}]",
                entity.short_alias.as_deref().unwrap_or(""),
                entity.full_name
            ))
        })
    })?;
    let target = load_snap(facade, &relationship.related)?;
    let key_map = relationship
        .key_pairs
        .iter()
        .cloned()
        .map(|(field_name, related)| KeyMap {
            field_name,
            related: Some(related),
        })
        .collect();
    members.push(MemberEntity {
        join_from_alias: Some(join_from),
        join_optional: optional,
        key_map,
        ..MemberEntity::new(entity_alias.clone(), target.full_name)
    });
    for key_value in relationship.key_values {
        conditions.push(Condition::equals(
            FieldRef::aliased(entity_alias.clone(), key_value.related),
            Value::string(key_value.value),
        ));
    }
    Ok(())
}

fn alias_from_spec(spec: &Value) -> Result<FieldAlias, Error> {
    Ok(FieldAlias {
        entity_alias: optional_string(spec, &["entity-alias", "entityAlias"]),
        name: req_string(spec, &["name"], "alias name")?,
        field: optional_string(spec, &["field"]),
        function: optional_string(spec, &["function"]),
        field_type: optional_string(spec, &["type", "field-type", "fieldType"]),
        default_display: optional_string(spec, &["default-display", "defaultDisplay"]).map(|value| value == "true"),
        ..FieldAlias::default()
    })
}

/// An alias with `pq-expression`, as Java's `addPqExprAlias`. It is not selected, and the rows a dynamic view returns
/// are plain maps, as Java's `getPlainValueMap`, so it only keeps `alias-all` from that name.
fn pq_alias_from_spec(spec: &Value) -> Result<Option<PqAlias>, Error> {
    let Some(expression) = optional_string(spec, &["pq-expression", "pqExpression"]) else {
        return Ok(None);
    };
    Ok(Some(PqAlias {
        name: req_string(spec, &["name"], "alias name")?,
        expression,
        default_display: optional_string(spec, &["default-display", "defaultDisplay"])
            .is_some_and(|value| value == "true"),
    }))
}

fn alias_all_from_spec(spec: &Value) -> Result<AliasAll, Error> {
    Ok(AliasAll {
        entity_alias: req_string(spec, &["entity-alias", "entityAlias"], "alias-all entity-alias")?,
        prefix: optional_string(spec, &["prefix"]),
        exclude: map_get(spec, &["exclude"]).map(string_list).unwrap_or_default(),
    })
}

fn parse_key_map(value: &Value) -> Vec<KeyMap> {
    match value {
        Value::Map(map) => map
            .iter()
            .map(|(field, related)| KeyMap {
                field_name: field.clone(),
                related: Some(text_of(related)).filter(|text| !text.is_empty()),
            })
            .collect(),
        Value::List(items) => items
            .iter()
            .filter_map(|item| {
                let field_name = optional_string(item, &["field-name", "fieldName"])?;
                Some(KeyMap {
                    field_name,
                    related: optional_string(item, &["related", "related-field-name", "relatedFieldName"]),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_conditions(value: &Value) -> Vec<Condition> {
    match value {
        Value::Map(map) => map
            .iter()
            .map(|(field, item)| Condition::equals(field_ref(field), item.clone()))
            .collect(),
        Value::List(items) => items.iter().filter_map(condition_item).collect(),
        _ => Vec::new(),
    }
}

fn condition_item(spec: &Value) -> Option<Condition> {
    let name = optional_string(spec, &["field-name", "fieldName", "field"])?;
    let value = map_get(spec, &["value"])?.clone();
    let field = match optional_string(spec, &["entity-alias", "entityAlias"]) {
        Some(alias) => FieldRef::aliased(alias, name),
        None => FieldRef::new(name),
    };
    let op = optional_string(spec, &["operator", "op"]).unwrap_or_else(|| String::from("equals"));
    Some(match op.as_str() {
        "not-equals" | "not-equal" => Condition::not_equals(field, value),
        "like" => Condition::like(field, value),
        "greater" | "greater-than" => Condition::greater(field, value),
        "less" | "less-than" => Condition::less(field, value),
        _ => Condition::equals(field, value),
    })
}

fn field_ref(token: &str) -> FieldRef {
    match token.split_once('.') {
        Some((alias, name)) if !alias.is_empty() && !name.is_empty() => FieldRef::aliased(alias, name),
        _ => FieldRef::new(token),
    }
}

fn and_all(conditions: Vec<Condition>) -> Option<Condition> {
    match conditions.len() {
        0 => None,
        1 => conditions.into_iter().next(),
        _ => Some(Condition::and(conditions)),
    }
}

fn explicit_page(parameters: &BTreeMap<String, Value>) -> Page {
    Page {
        limit: param(parameters, &["limit"])
            .and_then(value_i64)
            .map(|number| number.max(0)),
        offset: param(parameters, &["offset"])
            .and_then(value_i64)
            .map(|number| number.max(0)),
    }
}

fn blank_view(entity_name: &str) -> EntityDefinition {
    match entity_name.rsplit_once('.') {
        Some((package_name, name)) if !package_name.is_empty() && !name.is_empty() => {
            EntityDefinition::new_view(name, package_name)
        }
        _ => EntityDefinition::new_view(entity_name, "dynamic"),
    }
}

/// Run a built view. With `authorize`, reading it needs `view` authorization on each member entity, and the entity
/// filters of each authorizing record apply, as a find of each member would.
fn run_view(facade: &EntityFacade, built: &BuiltView, authorize: bool) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    let anchor = built
        .view
        .member_entities
        .first()
        .map(|member| member.entity_name.clone())
        .ok_or_else(|| Error::new("dynamic view needs a member-entity"))?;
    let page = built.page;
    let (view, tx) = facade.with_group(&anchor, |_, _, model, tx| {
        let mut view = built.view.clone();
        model.complete_view(&mut view)?;
        Ok((view, tx.clone()))
    })?;
    let mut condition = built.condition.clone();
    let mut artifacts = Vec::new();
    if authorize {
        for member in &view.member_entities {
            artifacts.push(EntityArtifact::push(facade, &member.entity_name, "view", "list")?);
            // The filters come from the record that authorized the frame on top, so each member adds its own.
            condition = crate::user::apply_view_filters(&view, condition);
        }
    }
    let statement = facade.with_group(&anchor, |_, dialect, model, _| {
        select_view(
            &view,
            model,
            dialect,
            &FindValues,
            condition.as_ref(),
            &built.order,
            page,
        )
    })?;
    let columns = view_columns(&view);
    let rows = query_tx(&tx, &statement.sql, &statement.params)?;
    after_statement(facade, &tx, false);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mapped = map_row(&columns, &row);
        out.push(field_rw::after_read(facade, &view, mapped)?);
    }
    Ok(out)
}

/// A view built in code: one primary member, then relationship members joined in order.
pub(crate) struct CodeView {
    pub(crate) name: String,
    pub(crate) primary_alias: String,
    pub(crate) primary_entity: String,
    /// Entity alias, the alias it joins from, the relationship name, and whether the join is optional.
    pub(crate) relationships: Vec<(String, String, String, bool)>,
    pub(crate) aliases: Vec<FieldAlias>,
}

/// Run a [`CodeView`]. Field names in `condition` and `order` are alias names, or
/// `entity_alias.field` through [`FieldRef::aliased`].
pub(crate) fn run_code_view(
    facade: &EntityFacade,
    code: &CodeView,
    condition: Option<Condition>,
    order: Vec<OrderBy>,
    page: LimitOffset,
) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    let primary = load_snap(facade, &code.primary_entity)?;
    let mut members = vec![MemberEntity::new(code.primary_alias.clone(), primary.full_name)];
    let mut conditions = Vec::new();
    for (alias, join_from, relationship, optional) in &code.relationships {
        push_relationship_member(
            facade,
            &mut members,
            &mut conditions,
            alias,
            join_from,
            relationship,
            *optional,
        )?;
    }
    conditions.extend(condition);
    let mut view = blank_view(&code.name);
    view.member_entities = members;
    view.aliases = code.aliases.clone();
    run_view(
        facade,
        &BuiltView {
            view,
            condition: and_all(conditions),
            order,
            page,
        },
        false,
    )
}

/// Comma-separated order entries as Java `FieldOrderOptions` reads them. `alias.field` names a
/// member field of a view.
fn parse_order(value: &Value) -> Vec<OrderBy> {
    let text = match value {
        Value::List(items) => items.iter().map(text_of).collect::<Vec<_>>().join(","),
        other => text_of(other),
    };
    text.split(',')
        .filter_map(OrderBy::parse)
        .map(|mut entry| {
            entry.field = field_ref(&entry.field.name);
            entry
        })
        .collect()
}

fn destinations(full_name: &str, records: &[BTreeMap<String, Value>]) -> Result<Vec<(PathBuf, FileKind)>, Error> {
    let file = records.iter().find_map(|record| {
        record
            .get("file")
            .or_else(|| record.get("filename"))
            .map(text_of)
            .filter(|text| !text.is_empty())
    });
    let directory = records
        .iter()
        .find_map(|record| record.get("directory").map(text_of).filter(|text| !text.is_empty()));
    let specified = specified_kind(records)?;
    if let Some(path) = file {
        let kind = kind_from_extension(&path).or(specified).unwrap_or(FileKind::Xml);
        return Ok(vec![(PathBuf::from(path), kind)]);
    }
    let Some(directory) = directory else {
        return Err(Error::new("data writer needs a directory or file"));
    };
    let directory = PathBuf::from(directory);
    if directory.is_file() {
        return Err(Error::new(format!("Path {} is not a directory", directory.display())));
    }
    let kinds = match specified {
        Some(kind) => vec![kind],
        None => vec![FileKind::Xml, FileKind::Json, FileKind::Csv],
    };
    Ok(kinds
        .into_iter()
        .map(|kind| (directory.join(format!("{full_name}.{}", kind.extension())), kind))
        .collect())
}

fn specified_kind(records: &[BTreeMap<String, Value>]) -> Result<Option<FileKind>, Error> {
    for record in records {
        if let Some(value) = record.get("fileType") {
            return Ok(Some(parse_kind(&text_of(value))?));
        }
    }
    Ok(None)
}

fn parse_kind(text: &str) -> Result<FileKind, Error> {
    match text.trim().to_ascii_lowercase().as_str() {
        "xml" => Ok(FileKind::Xml),
        "json" => Ok(FileKind::Json),
        "csv" => Ok(FileKind::Csv),
        other => Err(Error::new(format!("fileType {other} is not XML, JSON, or CSV"))),
    }
}

fn kind_from_extension(path: &str) -> Option<FileKind> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".json") {
        Some(FileKind::Json)
    } else if lower.ends_with(".xml") {
        Some(FileKind::Xml)
    } else if lower.ends_with(".csv") {
        Some(FileKind::Csv)
    } else {
        None
    }
}

fn writer_levels(records: &[BTreeMap<String, Value>]) -> i64 {
    if let Some(value) = records.iter().find_map(|record| record.get("dependentLevels")) {
        if let Some(number) = value_i64(value) {
            return number;
        }
    }
    if records.iter().any(|record| flag_at(record, "dependents")) {
        2
    } else {
        0
    }
}

fn first_text(records: &[BTreeMap<String, Value>], key: &str) -> Option<String> {
    records.iter().find_map(|record| record.get(key).map(text_of))
}

fn render_xml(nodes: &[RecordNode], prefix: &str, iso: bool) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entity-facade-xml>\n");
    out.push_str(&xml_body(nodes, prefix, iso));
    out.push_str("</entity-facade-xml>\n\n");
    out
}

fn xml_body(nodes: &[RecordNode], prefix: &str, iso: bool) -> String {
    let mut out = String::new();
    for node in nodes {
        let element = node.entity_key.replace('#', "-");
        write_xml_node(&mut out, node, &element, 1, prefix, iso);
    }
    out
}

/// The records of one entity as the data writer writes them between the file's start and end: XML
/// elements, or JSON objects each followed by a comma, with dependents to `levels` or the `master`
/// definition. With the text, how many records it holds: as Java's writer, in XML each nested record counts too,
/// and in JSON each top record counts once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn data_body(
    facade: &EntityFacade,
    entity_name: &str,
    records: &[BTreeMap<String, Value>],
    json: bool,
    levels: i64,
    master: Option<&str>,
    prefix: &str,
    iso: bool,
) -> Result<(String, u64), Error> {
    let mut nodes = Vec::with_capacity(records.len());
    for record in records {
        nodes.push(render_record(facade, entity_name, record, levels, master)?);
    }
    if !json {
        let count = nodes.iter().map(xml_record_count).sum();
        return Ok((xml_body(&nodes, prefix, iso), count));
    }
    let mut out = String::new();
    for node in &nodes {
        out.push_str("    ");
        write_json_node(&mut out, node, 4, iso);
        out.push_str(",\n");
    }
    Ok((out, nodes.len() as u64))
}

/// The record and the nested records written with it, as Java's `plainMapXmlWriter` counts them.
fn xml_record_count(node: &RecordNode) -> u64 {
    1 + node
        .nested
        .iter()
        .flat_map(|nested| nested.records.iter())
        .map(xml_record_count)
        .sum::<u64>()
}

/// One field as a CSV cell: base64 for bytes, ISO text for a date or time when `iso`, the CSV quoting.
pub(crate) fn csv_cell(out: &mut String, value: Option<&Value>, iso: bool) {
    let text = value.map(|value| scalar_string(value, iso)).unwrap_or_default();
    write_csv_field(out, &text);
}

fn write_xml_node(out: &mut String, node: &RecordNode, element: &str, level: usize, prefix: &str, iso: bool) {
    let indent = "    ".repeat(level);
    // `EntityValueBase` puts the prefix on the open tag only and closes the bare element name.
    let element = element.replace('#', "-");
    let open_name = format!("{prefix}{element}");
    out.push_str(&indent);
    out.push('<');
    out.push_str(&open_name);
    let mut cdata = Vec::new();
    for (field, value) in &node.fields {
        let text = scalar_string(value, iso);
        let wide = matches!(value, Value::Bytes(_)) || text.contains('\n') || text.contains('\r') || text.len() > 255;
        if text.is_empty() {
            continue;
        }
        if wide {
            cdata.push((field.as_str(), text));
            continue;
        }
        out.push(' ');
        out.push_str(field);
        out.push_str("=\"");
        out.push_str(&encode_xml(&text));
        out.push('"');
    }
    let nested: Vec<&NestedRecords> = node.nested.iter().filter(|item| !item.records.is_empty()).collect();
    if cdata.is_empty() && nested.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    for (field, text) in cdata {
        out.push_str("        <");
        out.push_str(field);
        out.push_str("><![CDATA[");
        // A `]]>` in the text would close the section early; split it across two sections.
        out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
        out.push_str("]]></");
        out.push_str(field);
        out.push_str(">\n");
    }
    for item in nested {
        let child_name = item.name.replace('#', "-");
        for child in &item.records {
            write_xml_node(out, child, &child_name, level + 1, prefix, iso);
        }
    }
    out.push_str(&indent);
    out.push_str("</");
    out.push_str(&element);
    out.push_str(">\n");
}

fn render_json(nodes: &[RecordNode], iso: bool) -> String {
    let mut out = String::from("[\n");
    for node in nodes {
        out.push_str("    ");
        write_json_node(&mut out, node, 4, iso);
        out.push_str(",\n");
    }
    out.push_str("]\n\n");
    out
}

fn write_json_node(out: &mut String, node: &RecordNode, indent: usize, iso: bool) {
    let pad = " ".repeat(indent);
    let inner = " ".repeat(indent + 4);
    out.push_str("{\n");
    let mut wrote = false;
    for (name, value) in &node.fields {
        if wrote {
            out.push_str(",\n");
        }
        wrote = true;
        out.push_str(&inner);
        out.push_str(&json_string(name));
        out.push_str(": ");
        write_field_json(out, value, iso);
    }
    for nested in &node.nested {
        if nested.records.is_empty() {
            continue;
        }
        if wrote {
            out.push_str(",\n");
        }
        wrote = true;
        out.push_str(&inner);
        out.push_str(&json_string(&nested.name));
        out.push_str(": ");
        if nested.one {
            write_json_node(out, &nested.records[0], indent + 4, iso);
        } else {
            out.push_str("[\n");
            for (index, child) in nested.records.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&" ".repeat(indent + 8));
                write_json_node(out, child, indent + 8, iso);
            }
            out.push('\n');
            out.push_str(&inner);
            out.push(']');
        }
    }
    if wrote {
        out.push_str(",\n");
    }
    out.push_str(&inner);
    out.push_str("\"_entity\": ");
    write_field_json(out, &Value::string(&node.entity_key), false);
    out.push('\n');
    out.push_str(&pad);
    out.push('}');
}

fn write_field_json(out: &mut String, value: &Value, iso: bool) {
    if iso {
        match value {
            Value::Timestamp(timestamp) => {
                out.push_str(&json_string(&timestamp.to_string()));
                return;
            }
            Value::Date(date) => {
                out.push_str(&json_string(&date.to_string()));
                return;
            }
            Value::Time(time) => {
                out.push_str(&json_string(&format!("{time}Z")));
                return;
            }
            _ => {}
        }
    }
    write_json_value(out, value);
}

/// A web response's JSON, as Java's `ContextJavaUtil.jacksonMapper` writes it: indented as Jackson's default pretty
/// printer (`INDENT_OUTPUT`), a time stamp or date as epoch millis (`WRITE_DATES_AS_TIMESTAMPS`), and a time as
/// `HH:mm:ss` text, as Jackson's `SqlTimeSerializer`.
pub(crate) fn encode_json(value: &Value) -> String {
    let mut out = String::new();
    write_response_json(&mut out, value, 0);
    out
}

/// Jackson's `DefaultPrettyPrinter`: an object's entries on their own lines, two spaces deeper for each object they
/// are in (an array adds no depth), `"name" : value`; an array on one line, `[ a, b ]`; empty ones `{ }` and `[ ]`.
fn write_response_json(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Timestamp(_) | Value::Date(_) => out.push_str(&to_plain_string(value)),
        Value::Time(time) => out.push_str(&json_string(&time.strftime("%H:%M:%S").to_string())),
        Value::List(items) if items.is_empty() => out.push_str("[ ]"),
        Value::List(items) => {
            out.push_str("[ ");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_response_json(out, item, depth);
            }
            out.push_str(" ]");
        }
        Value::Map(map) if map.is_empty() => out.push_str("{ }"),
        Value::Map(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str(&json_string(key));
                out.push_str(" : ");
                write_response_json(out, item, depth + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
        other => write_json_value(out, other),
    }
}

fn write_json_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::String(text) => out.push_str(&json_string(text)),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::I32(number) => out.push_str(&number.to_string()),
        Value::I64(number) => out.push_str(&number.to_string()),
        Value::F64(number) if number.is_finite() => out.push_str(&number.to_string()),
        Value::Decimal(number) => out.push_str(&number.to_string()),
        Value::Bytes(bytes) => out.push_str(&json_string(&STANDARD.encode(bytes))),
        Value::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_json_value(out, item);
            }
            out.push(']');
        }
        Value::Map(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&json_string(key));
                out.push_str(": ");
                write_json_value(out, item);
            }
            out.push('}');
        }
        other => out.push_str(&json_string(&to_plain_string(other))),
    }
}

fn render_csv(snap: &EntitySnap, records: &[BTreeMap<String, Value>], iso: bool) -> String {
    let mut out = String::new();
    out.push_str(&snap.full_name);
    out.push_str(",export");
    for name in snap.pk_names() {
        out.push(',');
        out.push_str(name);
    }
    out.push('\n');
    for (index, name) in snap.fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(name);
    }
    out.push('\n');
    for record in records {
        for (index, name) in snap.fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let text = record
                .get(name)
                .map(|value| scalar_string(value, iso))
                .unwrap_or_default();
            write_csv_field(&mut out, &text);
        }
        out.push('\n');
    }
    out
}

fn write_csv_field(out: &mut String, text: &str) {
    if text.contains(',') || text.contains('"') || text.contains('\n') || text.contains('\r') {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(text);
    }
}

fn scalar_string(value: &Value, iso: bool) -> String {
    if iso {
        match value {
            Value::Timestamp(timestamp) => return timestamp.to_string(),
            Value::Date(date) => return date.to_string(),
            Value::Time(time) => return format!("{time}Z"),
            _ => {}
        }
    }
    match value {
        Value::Bytes(bytes) => STANDARD.encode(bytes),
        other => to_plain_string(other),
    }
}

fn encode_xml(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ if ch > '\u{7f}' => {
                out.push_str("&#");
                out.push_str(&u32::from(ch).to_string());
                out.push(';');
            }
            _ => out.push(ch),
        }
    }
    out
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ if ch.is_control() => {
                let code = u32::from(ch);
                out.push_str(&format!("\\u{code:04x}"));
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// A lib error with a context kind, which picks the HTTP status of a REST call.
fn kinded(kind: ErrorKind, message: impl Into<String>) -> Error {
    crate::context::Error::of_kind(kind, message).into()
}

fn load_snap(facade: &EntityFacade, name: &str) -> Result<EntitySnap, Error> {
    facade.with_entity(name, |entity, _, _| Ok(EntitySnap::from_entity(entity)))
}

fn dependent_levels(parameters: &BTreeMap<String, Value>) -> i64 {
    if let Some(number) = parameters.get("dependentLevels").and_then(value_i64) {
        return number;
    }
    if flag(parameters, "dependents") {
        2
    } else {
        0
    }
}

fn optional_text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters.get(key).map(text_of).filter(|text| !text.is_empty())
}

fn flag(parameters: &BTreeMap<String, Value>, key: &str) -> bool {
    parameters.get(key).is_some_and(flag_value)
}

fn flag_at(record: &BTreeMap<String, Value>, key: &str) -> bool {
    record.get(key).is_some_and(flag_value)
}

fn flag_value(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => matches!(text.as_str(), "true" | "Y" | "y" | "yes" | "YES"),
        _ => false,
    }
}

fn value_i64(value: &Value) -> Option<i64> {
    match basic_convert(value, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => to_plain_string(other),
    }
}

fn is_control(key: &str) -> bool {
    matches!(
        key,
        "dependents"
            | "dependentLevels"
            | "master"
            | "masterNameInPath"
            | "path"
            | "pageIndex"
            | "pageSize"
            | "pageNoLimit"
            | "orderByField"
            | "directory"
            | "file"
            | "filename"
            | "fileType"
            | "prefix"
            | "isoDateTime"
            | "tableColumnNames"
            | "_entity"
            | "ec"
            | "authUsername"
            | "authPassword"
            | "member-entity"
            | "memberEntity"
            | "member-relationship"
            | "memberRelationship"
            | "alias"
            | "alias-all"
            | "aliasAll"
            | "condition"
            | "order-by"
            | "orderBy"
            | "limit"
            | "offset"
            | "view-entity"
            | "viewEntity"
    ) || key.ends_with("_op")
        || key.ends_with("_not")
        || key.ends_with("_ic")
        || key.ends_with("_from")
        || key.ends_with("_thru")
        || key.ends_with("_period")
}

fn param<'a>(parameters: &'a BTreeMap<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| parameters.get(*key))
}

fn map_get<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let Value::Map(map) = value else {
        return None;
    };
    keys.iter().find_map(|key| map.get(*key))
}

fn map_list(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.clone(),
        Value::Map(_) => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn optional_string(spec: &Value, keys: &[&str]) -> Option<String> {
    map_get(spec, keys).map(text_of).filter(|text| !text.is_empty())
}

fn req_string(spec: &Value, keys: &[&str], label: &str) -> Result<String, Error> {
    optional_string(spec, keys).ok_or_else(|| Error::new(format!("{label} is required")))
}

fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items.iter().map(text_of).filter(|text| !text.is_empty()).collect(),
        Value::String(text) => text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn value_map(entries: Vec<(String, Value)>) -> Value {
    let stack = moqui_lib::context_stack::ContextStack::new();
    let frame = stack.shared_map();
    {
        let mut map = frame.borrow_mut();
        for (key, value) in entries {
            map.insert(key, value);
        }
    }
    drop(stack);
    Value::Map(match Rc::try_unwrap(frame) {
        Ok(cell) => cell.into_inner(),
        Err(frame) => frame.borrow().clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_of(pairs: &[(&str, i64)]) -> Page {
        let parameters: BTreeMap<String, Value> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), Value::I64(*value)))
            .collect();
        let find = list_find(&crate::testing::open(), "moqui.basic.Enumeration", &parameters).unwrap();
        Page {
            limit: find.limit_value(),
            offset: find.offset_value(),
        }
    }

    #[test]
    fn a_rest_page_is_capped_and_never_negative() {
        let page = page_of(&[("pageSize", i64::MAX), ("pageIndex", 2)]);
        assert_eq!(page.limit, Some(max_page_size()));
        assert_eq!(page.offset, Some(2 * max_page_size()));
        let page = page_of(&[("pageSize", 50), ("pageIndex", -3)]);
        assert_eq!((page.limit, page.offset), (Some(50), Some(0)));
        let page = page_of(&[("pageSize", -5), ("pageIndex", 4)]);
        assert_eq!((page.limit, page.offset), (Some(0), Some(0)));
        assert_eq!(page_of(&[]).limit, Some(100));
        let parameters = BTreeMap::from([("pageIndex".to_string(), Value::I64(i64::MAX))]);
        let page = page_of(&[("pageIndex", i64::MAX)]);
        assert_eq!(page.offset, Some(i64::MAX));
        let info = page_numbers(&page, &parameters, 10);
        assert_eq!((info.range_low, info.range_high), (i64::MAX, 10));
    }

    /// The text Java's `jacksonMapper` gives for the same map: Jackson's pretty printer, dates as millis numbers.
    #[test]
    fn response_json_is_indented_as_jackson_with_dates_as_millis() {
        let stamp: jiff::Timestamp = "2024-01-02T03:04:05.123Z".parse().unwrap();
        let date = jiff::civil::date(2024, 1, 2);
        let time = jiff::civil::time(3, 4, 5, 0);
        let child = Value::Map(indexmap::IndexMap::from([
            ("c".to_string(), Value::I64(1)),
            ("d".to_string(), Value::List(vec![Value::I64(1), Value::I64(2)])),
        ]));
        let value = Value::Map(indexmap::IndexMap::from([
            ("stamp".to_string(), Value::Timestamp(stamp)),
            (
                "before".to_string(),
                Value::Timestamp("1960-01-02T03:04:05Z".parse().unwrap()),
            ),
            ("date".to_string(), Value::Date(date)),
            ("time".to_string(), Value::Time(time)),
            ("rows".to_string(), Value::List(vec![child.clone(), Value::string("x")])),
            ("one".to_string(), child),
            ("none".to_string(), Value::List(Vec::new())),
            ("empty".to_string(), Value::Map(indexmap::IndexMap::new())),
            ("text".to_string(), Value::string("a\"b")),
            ("null".to_string(), Value::Null),
        ]));
        let expected = r#"{
  "stamp" : 1704164645123,
  "before" : -315521755000,
  "date" : 1704153600000,
  "time" : "03:04:05",
  "rows" : [ {
    "c" : 1,
    "d" : [ 1, 2 ]
  }, "x" ],
  "one" : {
    "c" : 1,
    "d" : [ 1, 2 ]
  },
  "none" : [ ],
  "empty" : { },
  "text" : "a\"b",
  "null" : null
}"#;
        assert_eq!(encode_json(&value), expected);
        assert_eq!(encode_json(&Value::List(Vec::new())), "[ ]");
    }
}
