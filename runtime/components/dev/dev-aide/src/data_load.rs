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

//! Load the changed records of a data file, three ways (plans/design-dev-aide.md 12, the part of
//! plans/proposals/Data.md that the Dev AIDE has): each record, by entity and primary key, is compared in the base (the
//! file's text as it was last loaded, a `DevDataLoadBaseline` row, else the last commit's), the file (the buffer) and
//! the database. A record unchanged in the file is not written; one changed in the file is written only where the
//! database still has the base's values; one the database changed too, or an added one the database has otherwise, is
//! a conflict the user settles with Keep database or Take file. The data loader never deletes, so a record removed
//! from the file is deleted only by the user's choice, and only where the database still has the base's values. The
//! writes are one transaction; the baseline moves to the file's text only after it committed, and a retry finds the
//! records written as already applied.

use std::collections::BTreeMap;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::load_feed::{list_with, load_entity_xml_text, LoadOptions};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{basic_convert, compare_obj, to_plain_string, Value};
use serde::Serialize;
use serde_json::{json, Value as Json};

use crate::audit::{self, Write};
use crate::entities::export_refused;
use crate::roots::{Root, RootKind};
use crate::tier::Tier;
use crate::workspace::{json_value, sha256, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(plan_data_load), plan_data_load)?;
    registry.service_function(
        moqui_core::service_location!(load_changed_records),
        load_changed_records,
    )
}

pub const BASELINE_ENTITY: &str = "moqui.dev.DevDataLoadBaseline";

/// The most records a plan lists; the counts are of them all.
const MAX_LISTED: usize = 500;

/// One record of a data file: a row of an entity by its primary key, with the fields its element names.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Record {
    /// The entity's full name.
    pub entity: String,
    pub pk: Vec<(String, Value)>,
    /// The fields the element names, without the primary key, `lastUpdatedStamp` and the fields a plan never compares
    /// (a password or encrypted field).
    pub fields: BTreeMap<String, Value>,
    /// Of an entity the caller may not read through Dev AIDE (export#Records' rule): compared with its base only, so
    /// what a plan answers of it says nothing of the database.
    pub blind: bool,
}

impl Record {
    /// Every primary key field has a value: the record names one row.
    fn complete(&self) -> bool {
        !self.pk.is_empty() && self.pk.iter().all(|(_, value)| !blank(value))
    }

    /// The entity and primary key, as the plan's choices name a record.
    pub(crate) fn key(&self) -> String {
        let pk: Vec<String> = self
            .pk
            .iter()
            .map(|(name, value)| format!("{name}={}", to_plain_string(value)))
            .collect();
        format!("{}[{}]", self.entity, pk.join(","))
    }
}

/// What the database has of a record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Db {
    Absent,
    /// The row's fields, without the primary key and `lastUpdatedStamp`.
    Present(BTreeMap<String, Value>),
    /// Not read: the record names no whole primary key, or the read failed.
    Unknown,
}

/// How the file has a record against the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Change {
    Same,
    Added,
    Changed,
    Removed,
}

/// What a load does with a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Action {
    /// Unchanged in the file: not written, whatever the database has.
    Keep,
    /// The file's values are written: the database still has the base's, or has no row of an added record.
    Write,
    /// The database has the file's values already (a retry of a load that committed): not written.
    Applied,
    /// The database changed too: Keep database or Take file (for a removed record, Take file deletes it).
    Conflict,
    /// Removed from the file while the database still has the base's values: deleted only when chosen.
    Remove,
    /// Removed from the file and not in the database: nothing to do.
    Gone,
    /// No whole primary key, or the database could not be read: not compared or written (Load this file loads it).
    Skip,
    /// A record of an entity the caller may not read, changed, added or removed in the file: written (or deleted)
    /// only when chosen, never compared with the database.
    Differs,
}

/// A field of a planned record on each side: absent where that side has no such record or field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldSides {
    pub name: String,
    pub base: Option<Value>,
    pub file: Option<Value>,
    pub db: Option<Value>,
}

/// The plan of one record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Planned {
    pub key: String,
    pub entity: String,
    pub pk: Vec<(String, Value)>,
    pub change: Change,
    pub action: Action,
    /// The fields that differ between the sides, for the user to see.
    pub fields: Vec<FieldSides>,
    /// The record's index in the file's records, when the file has it.
    pub file_index: Option<usize>,
}

/// The records of a side by key, the last one of a key winning as it does in a load, in the order of their first
/// appearance.
fn by_key(records: &[Record]) -> (Vec<String>, BTreeMap<String, usize>) {
    let mut order = Vec::new();
    let mut index = BTreeMap::new();
    for (at, record) in records.iter().enumerate() {
        let key = record.key();
        if index.insert(key.clone(), at).is_none() {
            order.push(key);
        }
    }
    (order, index)
}

/// An empty text is no value, as an empty attribute is.
fn blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

/// Whether two values are the same as the data check compares them: by `compareTo` where it can, so `1.50` is `1.5`,
/// else as numbers, else as text; two blanks are the same.
pub(crate) fn same(left: &Value, right: &Value) -> bool {
    match (blank(left), blank(right)) {
        (true, true) => return true,
        (true, false) | (false, true) => return false,
        (false, false) => {}
    }
    match compare_obj(left, right) {
        Ok(order) => order.is_eq(),
        Err(_) => {
            let decimal = |value: &Value| {
                let converted = basic_convert(value, "BigDecimal");
                matches!(converted.value, Value::Decimal(_)).then_some(converted.value)
            };
            match (decimal(left), decimal(right)) {
                (Some(left), Some(right)) => compare_obj(&left, &right).is_ok_and(|order| order.is_eq()),
                _ => to_plain_string(left) == to_plain_string(right),
            }
        }
    }
}

fn field<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> &'a Value {
    fields.get(name).unwrap_or(&Value::Null)
}

/// Whether `other` has every value `reference` names.
fn matches(reference: &BTreeMap<String, Value>, other: &BTreeMap<String, Value>) -> bool {
    reference.iter().all(|(name, value)| same(value, field(other, name)))
}

/// Whether two records name the same fields with the same values.
fn same_fields(left: &BTreeMap<String, Value>, right: &BTreeMap<String, Value>) -> bool {
    matches(left, right) && matches(right, left)
}

/// The fields of `names` where `differs` holds, with each side's value.
fn sides_of(
    names: impl IntoIterator<Item = String>,
    base: Option<&Record>,
    file: Option<&Record>,
    db: &Db,
    differs: impl Fn(&Value, &Value, &Value) -> bool,
) -> Vec<FieldSides> {
    let db_fields = match db {
        Db::Present(fields) => Some(fields),
        Db::Absent | Db::Unknown => None,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        let base_value = base.map(|record| field(&record.fields, &name).clone());
        let file_value = file.map(|record| field(&record.fields, &name).clone());
        let db_value = db_fields.map(|fields| field(fields, &name).clone());
        let null = Value::Null;
        if differs(
            base_value.as_ref().unwrap_or(&null),
            file_value.as_ref().unwrap_or(&null),
            db_value.as_ref().unwrap_or(&null),
        ) {
            out.push(FieldSides {
                name,
                base: base_value,
                file: file_value,
                db: db_value,
            });
        }
    }
    out
}

/// The plan of a load: each record of the file, then each the file removed, compared in the base, the file and the
/// database (`db` reads a record's row). Pure but for `db`.
pub(crate) fn plan(base: &[Record], file: &[Record], db: &mut dyn FnMut(&Record) -> Db) -> Vec<Planned> {
    let (base_order, base_index) = by_key(base);
    let (file_order, file_index) = by_key(file);
    let mut out = Vec::new();
    for key in &file_order {
        let at = file_index[key];
        let now = &file[at];
        let before = base_index.get(key).map(|&index| &base[index]);
        let names = || {
            before
                .into_iter()
                .chain(Some(now))
                .flat_map(|record| record.fields.keys().cloned())
                .collect::<Vec<_>>()
        };
        let mut planned = Planned {
            key: key.clone(),
            entity: now.entity.clone(),
            pk: now.pk.clone(),
            change: match before {
                None => Change::Added,
                Some(before) if same_fields(&before.fields, &now.fields) => Change::Same,
                Some(_) => Change::Changed,
            },
            action: Action::Keep,
            fields: Vec::new(),
            file_index: Some(at),
        };
        if planned.change == Change::Same {
            out.push(planned);
            continue;
        }
        if now.blind || before.is_some_and(|before| before.blind) {
            planned.action = Action::Differs;
            out.push(planned);
            continue;
        }
        if !now.complete() {
            planned.action = Action::Skip;
            out.push(planned);
            continue;
        }
        let row = db(now);
        let (action, fields) = match (&row, before) {
            (Db::Unknown, _) => (Action::Skip, Vec::new()),
            (Db::Present(fields), _) if matches(&now.fields, fields) => (Action::Applied, Vec::new()),
            (Db::Absent, None) => (
                Action::Write,
                sides_of(names(), None, Some(now), &row, |_, file, _| !blank(file)),
            ),
            (Db::Present(_), None) => (
                Action::Conflict,
                sides_of(names(), None, Some(now), &row, |_, file, db| !same(file, db)),
            ),
            (Db::Present(fields), Some(before)) if matches(&before.fields, fields) => (
                Action::Write,
                sides_of(names(), Some(before), Some(now), &row, |base, file, _| {
                    !same(base, file)
                }),
            ),
            // the database changed the record (or deleted it) since the base
            (_, Some(before)) => (
                Action::Conflict,
                sides_of(names(), Some(before), Some(now), &row, |base, file, db| {
                    !same(base, file) || !same(base, db)
                }),
            ),
        };
        planned.action = action;
        planned.fields = fields;
        out.push(planned);
    }
    for key in base_order.iter().filter(|key| !file_index.contains_key(*key)) {
        let before = &base[base_index[key]];
        let mut planned = Planned {
            key: key.clone(),
            entity: before.entity.clone(),
            pk: before.pk.clone(),
            change: Change::Removed,
            action: Action::Skip,
            fields: Vec::new(),
            file_index: None,
        };
        if before.blind {
            planned.action = Action::Differs;
        } else if before.complete() {
            let row = db(before);
            let names = || before.fields.keys().cloned().collect::<Vec<_>>();
            (planned.action, planned.fields) = match &row {
                Db::Unknown => (Action::Skip, Vec::new()),
                Db::Absent => (Action::Gone, Vec::new()),
                Db::Present(fields) if matches(&before.fields, fields) => (Action::Remove, Vec::new()),
                Db::Present(_) => (
                    Action::Conflict,
                    sides_of(names(), Some(before), None, &row, |base, _, db| !same(base, db)),
                ),
            };
        }
        out.push(planned);
    }
    out
}

/// What the user chose for a record: `file` (Take file: write it, or delete a removed one) or `database` (Keep
/// database).
pub(crate) type Choices = BTreeMap<String, String>;

/// The writes and deletes of a plan once the choices are taken.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Resolved<'a> {
    pub writes: Vec<&'a Planned>,
    pub deletes: Vec<&'a Planned>,
    /// Conflicts and removals kept as the database has them.
    pub kept: usize,
}

/// The writes and deletes of `planned` with `choices`, or the keys of the conflicts that have no choice (the plan the
/// user saw had fewer, or none): a conflict is never settled by default. A removal is kept unless chosen.
pub(crate) fn resolve<'a>(planned: &'a [Planned], choices: &Choices) -> Result<Resolved<'a>, Vec<String>> {
    let mut resolved = Resolved::default();
    let mut unresolved = Vec::new();
    for item in planned {
        let choice = choices.get(&item.key).map(String::as_str);
        match (item.action, choice) {
            (Action::Write, _) => resolved.writes.push(item),
            (Action::Conflict, Some("file")) if item.change == Change::Removed => resolved.deletes.push(item),
            (Action::Conflict, Some("file")) => resolved.writes.push(item),
            (Action::Conflict, Some("database")) => resolved.kept += 1,
            (Action::Conflict, _) => unresolved.push(item.key.clone()),
            (Action::Remove | Action::Differs, Some("file")) if item.change == Change::Removed => {
                resolved.deletes.push(item)
            }
            (Action::Differs, Some("file")) => resolved.writes.push(item),
            (Action::Remove | Action::Differs, _) => resolved.kept += 1,
            (Action::Keep | Action::Applied | Action::Gone | Action::Skip, _) => {}
        }
    }
    if unresolved.is_empty() {
        Ok(resolved)
    } else {
        Err(unresolved)
    }
}

/// How many records of a plan take each action.
fn counts(planned: &[Planned]) -> Json {
    let count = |action: Action| planned.iter().filter(|item| item.action == action).count();
    json!({
        "unchanged": count(Action::Keep), "write": count(Action::Write), "applied": count(Action::Applied),
        "conflict": count(Action::Conflict), "remove": count(Action::Remove), "gone": count(Action::Gone),
        "skip": count(Action::Skip), "differs": count(Action::Differs),
    })
}

/// What a plan keeps from the caller (export#Records' rule): a password or encrypted field, never compared or
/// answered, and every record of an entity the caller may not read through Dev AIDE, compared with its base only.
struct Guard<'a> {
    context: &'a Context,
    entities: BTreeMap<String, (bool, moqui_core::entity::sensitive::GenericAccess)>,
}

impl<'a> Guard<'a> {
    fn new(context: &'a Context) -> Self {
        Self {
            context,
            entities: BTreeMap::new(),
        }
    }

    fn entity(&mut self, entity: &str) -> &(bool, moqui_core::entity::sensitive::GenericAccess) {
        let context = self.context;
        self.entities.entry(entity.to_string()).or_insert_with(|| {
            (
                export_refused(context, entity).is_some(),
                moqui_core::entity::sensitive::generic_access(&context.facade, entity),
            )
        })
    }

    /// The caller may not read the entity's records through Dev AIDE.
    fn refused(&mut self, entity: &str) -> bool {
        self.entity(entity).0
    }

    /// A field never compared or answered.
    fn hidden(&mut self, entity: &str, name: &str) -> bool {
        self.entity(entity).1.hidden(name)
    }

    /// `fields` without the hidden ones.
    fn compared(&mut self, entity: &str, fields: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        fields
            .into_iter()
            .filter(|(name, _)| !self.hidden(entity, name))
            .collect()
    }

    fn planned(&mut self, item: &Planned) -> Json {
        let fields: Vec<Json> = item
            .fields
            .iter()
            .map(|sides| {
                let shown = |value: &Option<Value>| value.as_ref().map_or(Json::Null, |value| json!(to_plain_string(value)));
                json!({ "name": sides.name, "base": shown(&sides.base), "file": shown(&sides.file), "db": shown(&sides.db) })
            })
            .collect();
        let pk: serde_json::Map<String, Json> = item
            .pk
            .iter()
            .map(|(name, value)| (name.clone(), json!(to_plain_string(value))))
            .collect();
        json!({ "key": item.key, "entity": item.entity, "pk": pk, "change": item.change, "action": item.action,
            "fields": fields })
    }
}

/// The records of a data file's text: each top-level element read on its own as the data check reads it (its nested
/// records after it, with the keys they take from it), so an element that does not read (an unknown entity, a bad
/// value) is logged and passed over while the rest are compared. A service element is not a record. With each
/// record, its value as a load writes it, the hidden fields kept there.
fn records(guard: &mut Guard<'_>, location: &str, body: &str, skipped: &mut Vec<String>) -> Vec<(Record, EntityValue)> {
    let facade = &guard.context.facade;
    let mut out = Vec::new();
    if body.trim().is_empty() {
        return out;
    }
    let doc = match moqui_lib::xml_edit::Doc::parse(body) {
        Ok(doc) => doc,
        Err(err) => {
            skipped.push(format!("{location}: {err}"));
            return out;
        }
    };
    let Some(root) = doc.root() else {
        return out;
    };
    for element in doc.elements(root) {
        let element_text = doc.slice(&doc.nodes[element].span);
        let wrapped = format!("<entity-facade-xml>\n{element_text}\n</entity-facade-xml>\n");
        let listed = list_with(|| load_entity_xml_text(facade, location, &wrapped, &LoadOptions::default()));
        let values = match listed {
            Ok((_, result)) => result.values,
            Err(err) => {
                let head: String = element_text.chars().take(80).collect();
                tracing::warn!("Load the changed records of {location}: {head} is passed over: {err}");
                skipped.push(format!("{head}: {err}"));
                continue;
            }
        };
        for value in values {
            match record_of(facade, &value) {
                Ok(mut record) => {
                    record.blind = guard.refused(&record.entity);
                    record.fields = guard.compared(&record.entity, std::mem::take(&mut record.fields));
                    out.push((record, value));
                }
                Err(err) => {
                    tracing::warn!(
                        "Load the changed records of {location}: a {} record is passed over: {err}",
                        value.entity_name()
                    );
                    skipped.push(format!("{}: {err}", value.entity_name()));
                }
            }
        }
    }
    out
}

/// The entity's full name and primary key field names.
fn entity_keys(facade: &EntityFacade, entity: &str) -> Result<(String, Vec<String>), LibError> {
    facade.with_entity(entity, |definition, _, _| {
        let pk_names: Vec<String> = definition.pk_field_names().into_iter().map(str::to_string).collect();
        Ok((definition.full_name.clone(), pk_names))
    })
}

fn record_of(facade: &EntityFacade, value: &EntityValue) -> Result<Record, LibError> {
    let (entity, pk_names) = entity_keys(facade, value.entity_name())?;
    let pk = pk_names
        .iter()
        .map(|name| (name.clone(), value.get(name).cloned().unwrap_or(Value::Null)))
        .collect();
    let fields = value
        .fields()
        .iter()
        .filter(|(name, _)| {
            !pk_names.iter().any(|pk| pk.as_str() == name.as_ref()) && name.as_ref() != "lastUpdatedStamp"
        })
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect();
    Ok(Record {
        entity,
        pk,
        fields,
        blind: false,
    })
}

/// The row of `record` in the database, without the fields a plan never compares. A read that fails is logged and
/// the record is not compared.
fn db_row(guard: &mut Guard<'_>, record: &Record) -> Db {
    if !record.complete() {
        return Db::Unknown;
    }
    let found = guard.context.facade.find(&record.entity).and_then(|mut find| {
        for (name, value) in &record.pk {
            find = find.eq(name, value.clone());
        }
        find.one()
    });
    match found {
        Ok(Some(row)) => {
            let fields: BTreeMap<String, Value> = row
                .fields()
                .iter()
                .filter(|(name, _)| {
                    !record.pk.iter().any(|(pk, _)| pk.as_str() == name.as_ref()) && name.as_ref() != "lastUpdatedStamp"
                })
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect();
            Db::Present(guard.compared(&record.entity, fields))
        }
        Ok(None) => Db::Absent,
        Err(err) => {
            tracing::warn!("Load the changed records: {} could not be read: {err}", record.key());
            Db::Unknown
        }
    }
}

/// Where the base came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BaseFrom {
    /// The text the last load of the file loaded.
    Baseline,
    /// The file at the last commit.
    Commit,
    /// No baseline and no history: every record is added.
    None,
}

impl BaseFrom {
    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Commit => "commit",
            Self::None => "none",
        }
    }
}

/// The base of a file: its text as last loaded from this root (with the zone that load read its time stamps in), else
/// as last committed.
struct Base {
    text: String,
    from: BaseFrom,
    zone: Option<String>,
}

/// The time zone a file's records are read in: the last load's, so a time stamp's text names the instant it named then,
/// else the user's, as Load this file reads it.
fn zone_of(base: &Base) -> String {
    base.zone
        .clone()
        .filter(|zone| !zone.is_empty())
        .or_else(|| moqui_core::l10n::time_zone().iana_name().map(str::to_string))
        .unwrap_or_else(moqui_core::l10n::server_time_zone_name)
}

/// The file's text as last loaded from this root, else as last committed.
fn base_of(context: &Context, root: &Root, relative: &str) -> Base {
    let baseline = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        context.facade.find(BASELINE_ENTITY).and_then(|find| {
            find.eq("rootId", Value::string(&root.id))
                .eq("filePath", Value::string(relative))
                .one()
        })
    };
    match baseline {
        Ok(Some(row)) => {
            let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            return Base {
                text: field("baselineText"),
                from: BaseFrom::Baseline,
                zone: Some(field("timeZone")),
            };
        }
        Ok(None) => {}
        Err(err) => tracing::warn!("The data load baseline of {} {relative}: {err}", root.id),
    }
    if root.kind == RootKind::Git {
        let committed = context
            .git()
            .read(&root.path, &["show", &format!("HEAD:{relative}")])
            .unwrap_or_default();
        return Base {
            text: committed,
            from: BaseFrom::Commit,
            zone: None,
        };
    }
    Base {
        text: String::new(),
        from: BaseFrom::None,
        zone: None,
    }
}

/// Move the baseline of a file to `body`, the text a load just committed, in a transaction of its own (a failure
/// leaves the service's, the audit row, as it is). A failure is answered to the user: the next load then compares with
/// the older base, and finds the records written as applied.
fn store_baseline(context: &Context, root: &Root, relative: &str, body: &str, zone: &str) -> Result<(), String> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let stored = context.facade.default_transactions().and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let mut row = context.facade.make_value(BASELINE_ENTITY)?;
            row.set("rootId", Value::string(&root.id));
            row.set("filePath", Value::string(relative));
            row.set("textHash", Value::string(sha256(body.as_bytes())));
            row.set("baselineText", Value::string(body));
            row.set("timeZone", Value::string(zone));
            let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
            row.set("loadedByUserId", Value::string(user));
            row.set("loadedDate", moqui_core::user::now_value());
            row.create_or_update(&context.facade)
        })
    });
    stored.map_err(|err| {
        tracing::error!("The data load baseline of {} {relative} was not moved: {err}", root.id);
        err.to_string()
    })
}

/// The three sides of a load: the root, the file's path, the base and the file's text and their records.
struct Sides<'a> {
    root: &'a Root,
    relative: String,
    path: std::path::PathBuf,
    location: String,
    base_text: String,
    base_from: BaseFrom,
    /// The zone both sides' time stamps are read in ([`zone_of`]).
    zone: String,
    file_text: String,
    base: Vec<Record>,
    file: Vec<(Record, EntityValue)>,
    /// What of the file did not read, with why.
    skipped: Vec<String>,
}

/// The sides of the file `path` of `rootId`: the buffer's `text` when `buffer` (a plan's preview), else the file.
fn sides<'a>(
    context: &'a Context,
    guard: &mut Guard<'_>,
    parameters: &Parameters,
    buffer: bool,
) -> Result<Sides<'a>, LibError> {
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let file_text = Some(text(parameters, "text"))
        .filter(|body| buffer && !body.is_empty())
        .map_or_else(|| std::fs::read_to_string(&resolved.path), Ok)
        .map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    let base = base_of(context, root, &resolved.relative);
    let zone = zone_of(&base);
    let location = resolved.path.display().to_string();
    let mut skipped = Vec::new();
    let (base_records, file) = moqui_core::l10n::with_time_zone(&zone, || {
        let base_records: Vec<Record> = records(guard, &location, &base.text, &mut Vec::new())
            .into_iter()
            .map(|(record, _)| record)
            .collect();
        (base_records, records(guard, &location, &file_text, &mut skipped))
    });
    Ok(Sides {
        root,
        relative: resolved.relative,
        path: resolved.path,
        location,
        base_text: base.text,
        base_from: base.from,
        zone,
        file_text,
        base: base_records,
        file,
        skipped,
    })
}

/// The plan's records a user reads (every one but those unchanged or gone), at most [`MAX_LISTED`].
fn listed(guard: &mut Guard<'_>, planned: &[Planned]) -> Vec<Json> {
    planned
        .iter()
        .filter(|item| !matches!(item.action, Action::Keep | Action::Gone))
        .take(MAX_LISTED)
        .map(|item| guard.planned(item))
        .collect()
}

/// `plan#DataLoad`: what Load the changed records would do with a data file (the buffer's `text`, or the file): the
/// counts, and each record it writes, removes or finds in conflict with the fields that differ (12). A password or
/// encrypted field is left out of the compare and the answer, and a record of an entity the caller may not read
/// through Dev AIDE is answered only as differing from its base. Writes nothing.
fn plan_data_load(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut guard = Guard::new(&context);
    let sides = sides(&context, &mut guard, parameters, true)?;
    let file: Vec<Record> = sides.file.iter().map(|(record, _)| record.clone()).collect();
    let planned = plan(&sides.base, &file, &mut |record| db_row(&mut guard, record));
    let mut out = Parameters::new();
    out.insert("baseFrom".into(), Value::string(sides.base_from.name()));
    out.insert("baseHash".into(), Value::string(sha256(sides.base_text.as_bytes())));
    out.insert("fileHash".into(), Value::string(sha256(sides.file_text.as_bytes())));
    out.insert("counts".into(), json_value(&counts(&planned)));
    out.insert("records".into(), json_value(&listed(&mut guard, &planned)));
    out.insert(
        "truncated".into(),
        Value::Bool(
            planned
                .iter()
                .filter(|item| !matches!(item.action, Action::Keep | Action::Gone))
                .count()
                > MAX_LISTED,
        ),
    );
    out.insert("skipped".into(), json_value(&sides.skipped));
    Ok(out)
}

/// The `choices` parameter: a map, or its JSON text, of a record's key to `file` or `database`.
fn choices_of(parameters: &Parameters) -> Result<Choices, LibError> {
    match parameters.get("choices") {
        Some(Value::Map(map)) => Ok(map
            .iter()
            .map(|(key, value)| (key.clone(), to_plain_string(value)))
            .collect()),
        Some(Value::String(raw)) if !raw.trim().is_empty() => serde_json::from_str::<BTreeMap<String, String>>(raw)
            .map_err(|err| LibError::new(format!("choices: {err}"))),
        _ => Ok(Choices::new()),
    }
}

/// What a load wrote.
#[derive(Default)]
struct Loaded {
    written: usize,
    deleted: usize,
    kept: usize,
    applied: usize,
    unchanged: usize,
    conflicts: usize,
    /// Records of the file not compared or written (no whole primary key, a read that failed).
    passed_over: usize,
}

/// Write the file's record `value` as the data loader writes an XML record: the entity-auto store (`store#<entity>`,
/// Java's `storeEntity`, with its status checks), as the user.
fn store_record(value: &EntityValue) -> Result<(), LibError> {
    let parameters = moqui_core::entity::string_map(value.fields());
    moqui_core::service::call::sync_checked(
        &format!("store#{}", value.entity_name()),
        &parameters,
        &moqui_core::service::call::CallOptions::default(),
    )
    .map(|_| ())
    .map_err(|err| LibError::new(err.to_string()))
}

/// `load#ChangedRecords`: the plan of [`plan_data_load`] for the file as saved, carried out in one transaction: the
/// writes through the entity-auto store in the file's order, each conflict and each differing record as `choices`
/// settles them (a conflict needs one; a plan that now has a conflict the choices do not name is refused, so the user
/// sees it first), and the removals chosen deleted, the file's last record first so a record goes before the one it
/// names. `baseHash` and `fileHash` are the plan's, checked in the transaction: a load since, or another text, is
/// refused. Once it committed, the file's text is the baseline of the next load, unless something of it was passed
/// over (an element that did not read, a record not compared), whose change would be lost; audited.
fn load_changed_records(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let mut guard = Guard::new(&context);
    let sides = sides(&context, &mut guard, parameters, false)?;
    context.may_write(sides.root, &sides.relative)?;
    let (base_hash, file_hash) = (text(parameters, "baseHash"), text(parameters, "fileHash"));
    if base_hash.is_empty() || file_hash.is_empty() {
        return Err(LibError::new(
            "Load the changed records needs the plan's baseHash and fileHash",
        ));
    }
    let choices = choices_of(parameters)?;
    let facade = &context.facade;
    let file: Vec<Record> = sides.file.iter().map(|(record, _)| record.clone()).collect();
    let done = facade
        .default_transactions()?
        .run(TransactionAttribute::ForceNew, None, || {
            // the plan's sides as they are now, read in the transaction that writes
            let base_now = base_of(&context, sides.root, &sides.relative).text;
            if sha256(base_now.as_bytes()) != base_hash || base_hash != sha256(sides.base_text.as_bytes()) {
                return Err(LibError::new(format!(
                    "{} was loaded since the plan was made: check it again",
                    sides.relative
                )));
            }
            let file_now = std::fs::read_to_string(&sides.path).unwrap_or_default();
            if sha256(file_now.as_bytes()) != file_hash || file_hash != sha256(sides.file_text.as_bytes()) {
                return Err(LibError::new(format!(
                    "{} changed since the plan was made: check it again",
                    sides.relative
                )));
            }
            let planned = plan(&sides.base, &file, &mut |record| db_row(&mut guard, record));
            let resolved = resolve(&planned, &choices).map_err(|unresolved| {
                LibError::new(format!(
                    "{} conflict{} the plan did not show ({}): check it again",
                    unresolved.len(),
                    if unresolved.len() == 1 { "" } else { "s" },
                    unresolved.join(", ")
                ))
            })?;
            for item in &resolved.writes {
                let Some((_, value)) = item.file_index.and_then(|at| sides.file.get(at)) else {
                    continue;
                };
                store_record(value).map_err(|err| LibError::new(format!("{}: {err}", item.key)))?;
            }
            // the base's last record first: a nested or later record goes before the one it names
            for item in resolved.deletes.iter().rev() {
                let mut value = facade.make_value(&item.entity)?;
                for (name, key) in &item.pk {
                    value.set(name, key.clone());
                }
                value
                    .delete(facade)
                    .map_err(|err| LibError::new(format!("{}: {err}", item.key)))?;
            }
            let count = |action: Action| planned.iter().filter(|item| item.action == action).count();
            Ok(Loaded {
                written: resolved.writes.len(),
                deleted: resolved.deletes.len(),
                kept: resolved.kept,
                applied: count(Action::Applied),
                unchanged: count(Action::Keep),
                conflicts: count(Action::Conflict),
                passed_over: count(Action::Skip),
            })
        });
    audit::record(
        facade,
        &Write {
            action: "DAA_LOAD",
            root: &sides.root.id,
            path: &sides.relative,
            hash_before: Some(&sha256(sides.base_text.as_bytes())),
            hash_after: Some(&sha256(sides.file_text.as_bytes())),
            detail: Some(match &done {
                Ok(loaded) => json!({ "threeWay": true, "baseFrom": sides.base_from.name(), "written": loaded.written,
                    "deleted": loaded.deleted, "kept": loaded.kept, "applied": loaded.applied,
                    "conflicts": loaded.conflicts, "passedOver": loaded.passed_over + sides.skipped.len() }),
                Err(err) => json!({ "threeWay": true, "error": err.to_string() }),
            }),
        },
    );
    let loaded = done.map_err(|err| LibError::new(err.to_string()))?;
    // the baseline moves only over a file all of whose records were settled: a change passed over stays to load
    let baseline = if loaded.passed_over > 0 || !sides.skipped.is_empty() {
        Err(format!(
            "{} record{} of the file were passed over, so the next load compares with the same base",
            loaded.passed_over + sides.skipped.len(),
            if loaded.passed_over + sides.skipped.len() == 1 {
                ""
            } else {
                "s"
            }
        ))
    } else {
        store_baseline(&context, sides.root, &sides.relative, &sides.file_text, &sides.zone)
            .map_err(|err| format!("the baseline was not moved ({err}): the next load compares with the same base"))
    };
    let mut out = Parameters::new();
    let as_long = |count: usize| Value::I64(i64::try_from(count).unwrap_or(i64::MAX));
    out.insert("written".into(), as_long(loaded.written));
    out.insert("deleted".into(), as_long(loaded.deleted));
    out.insert("kept".into(), as_long(loaded.kept));
    out.insert("applied".into(), as_long(loaded.applied));
    out.insert("unchanged".into(), as_long(loaded.unchanged));
    out.insert("conflicts".into(), as_long(loaded.conflicts));
    out.insert("records".into(), as_long(loaded.written + loaded.deleted));
    out.insert("baselineMoved".into(), Value::Bool(baseline.is_ok()));
    out.insert("baselineNote".into(), baseline.err().map_or(Value::Null, Value::string));
    out.insert("skipped".into(), json_value(&sides.skipped));
    out.insert("location".into(), Value::string(&sides.location));
    Ok(out)
}

#[cfg(test)]
#[path = "data_load_tests.rs"]
mod tests;
