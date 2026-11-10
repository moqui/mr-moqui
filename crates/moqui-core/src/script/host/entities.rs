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

//! `ec.entity`: its functions, the find builder `find(name)` returns ([`FIND`]), and the methods of an entity list
//! ([`LIST`]) and of an entity value ([`ENTITY_VALUE`]).
//!
//! A list and a value are exotic objects on each engine (their fields and rows read as properties, which a
//! [`HostClass`] cannot say): the engine makes one from a host value that holds a [`ScriptList`] or an [`EntityCell`],
//! with the class here as its prototype, and gives a method the list's [`ListState`] or the value's cell as its
//! receiver. A method that returns a list or a value returns that host value. A find is an ordinary object of its
//! class over a [`FindState`]; its `iterator()` is Java's `EntityListIterator` ([`super::list_iterator`]).
//!
//! Every method that reads or writes the database, or reads an entity definition, may run a nested script (an entity
//! ECA rule, an authorization filter), so it is not a leaf; the builder steps and the list's reads are. `find` and
//! `condition` are leaves that lend the engine only for the step that may nest: a definition made at run time from rows.
//! `list` and `one` are leaves that answer a hit of the shared entity cache without the lend, when no authorization
//! filter applies and nothing reads the database (`EntityFind::list_from_cache`), and lend it for any other find.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use indexmap::IndexMap;
use moqui_lib::object::{HostObject, Value};
use moqui_lib::sync::lock;

use super::conditions::{comparison, condition_in, date_range_condition, now_timestamp, timestamp_of};
use super::{entity_facade, host_texts, method, Call, HostClass, HostError, Method, Path, Ret};
use crate::entity::{EntityFacade, EntityFind, EntityList, EntityValue};

/// An entity value a script and the context stack share by reference.
pub(crate) type EntityCell = Mutex<EntityValue>;

/// The functions of `ec.entity` the table installs, each its own path.
pub(super) static PATHS: &[Path] = &[
    Path::function("ec.entity.find", &FIND_FN),
    Path::function("ec.entity.makeValue", &MAKE_VALUE),
    Path::function("ec.entity.startTxCacheDb", &START_TX_CACHE_DB),
    Path::function("ec.entity.stopTxCache", &STOP_TX_CACHE),
    Path::function("ec.entity.isTxCacheActive", &IS_TX_CACHE_ACTIVE),
    Path::function("ec.entity.sequencedIdPrimary", &SEQUENCED_ID_PRIMARY),
    Path::function("ec.entity.getDatabaseConfName", &GET_DATABASE_CONF_NAME),
    Path::function("ec.entity.tempSetSequencedIdPrimary", &TEMP_SET_SEQUENCED_ID_PRIMARY),
    Path::function(
        "ec.entity.tempResetSequencedIdPrimary",
        &TEMP_RESET_SEQUENCED_ID_PRIMARY,
    ),
    Path::function("ec.entity.getDataDocuments", &super::conditions::GET_DATA_DOCUMENTS),
    Path::object("ec.entity.conditionFactory", &super::conditions::CONDITION_FACTORY),
];

static FIND_FN: Method = method!("ec.entity", "find", [Text], None, leaf, find);
static MAKE_VALUE: Method = method!("ec.entity", "makeValue", [Text], make_value);
static START_TX_CACHE_DB: Method = method!("ec.entity", "startTxCacheDb", [Bool], start_tx_cache_db);
static STOP_TX_CACHE: Method = method!("ec.entity", "stopTxCache", [], stop_tx_cache);
static IS_TX_CACHE_ACTIVE: Method = method!("ec.entity", "isTxCacheActive", [], None, leaf, is_tx_cache_active);
static SEQUENCED_ID_PRIMARY: Method = method!(
    "ec.entity",
    "sequencedIdPrimary",
    [Text, OptWhole, OptWhole],
    sequenced_id_primary
);
static GET_DATABASE_CONF_NAME: Method = method!("ec.entity", "getDatabaseConfName", [Text], get_database_conf_name);
static TEMP_SET_SEQUENCED_ID_PRIMARY: Method = method!(
    "ec.entity",
    "tempSetSequencedIdPrimary",
    [Text, Whole, Whole],
    temp_set_sequenced_id_primary
);
static TEMP_RESET_SEQUENCED_ID_PRIMARY: Method = method!(
    "ec.entity",
    "tempResetSequencedIdPrimary",
    [Text],
    temp_reset_sequenced_id_primary
);

/// The find builder (`ec.entity.find(name)`). A chained call changes its find and returns the same builder, and a run
/// (`list()`, `one()`, `count()`) can be repeated, as on Java's `EntityFind`.
pub(crate) static FIND: HostClass = HostClass {
    name: "host:EntityFind",
    methods: &[
        method!("EntityFind", "condition", [Value, OptValue, OptValue], None, leaf, condition),
        method!("EntityFind", "orderBy", [Text], None, leaf, order_by),
        method!("EntityFind", "limit", [Value], None, leaf, limit),
        method!("EntityFind", "offset", [Value, OptValue], None, leaf, offset),
        method!(
            "EntityFind",
            "conditionDate",
            [Text, Text, OptValue],
            None,
            leaf,
            condition_date
        ),
        method!("EntityFind", "getPageIndex", [], None, leaf, get_page_index),
        method!("EntityFind", "getPageSize", [], None, leaf, get_page_size),
        method!("EntityFind", "updateAll", [Value], update_all),
        method!("EntityFind", "useCache", [Value], None, leaf, use_cache),
        method!("EntityFind", "forUpdate", [Value], None, leaf, for_update),
        method!("EntityFind", "forNoKeyUpdate", [Value], None, leaf, for_no_key_update),
        method!("EntityFind", "distinct", [Value], None, leaf, distinct),
        method!("EntityFind", "disableAuthz", [], None, leaf, disable_authz),
        method!("EntityFind", "useClone", [Value], None, leaf, use_clone),
        method!("EntityFind", "selectField", [Text], None, leaf, select_field),
        method!("EntityFind", "selectFormFields", [Text], None, leaf, select_form_fields),
        method!("EntityFind", "list", [], None, leaf, list),
        method!("EntityFind", "one", [], None, leaf, one),
        method!("EntityFind", "oneMaster", [OptText], one_master),
        method!("EntityFind", "count", [], count),
        method!("EntityFind", "getQueryTextList", [], None, leaf, get_query_text_list),
        method!("EntityFind", "getLimit", [], None, leaf, get_limit),
        method!("EntityFind", "getOffset", [], None, leaf, get_offset),
        method!("EntityFind", "copy", [], None, leaf, copy),
        method!("EntityFind", "deleteAll", [], delete_all),
        method!("EntityFind", "purge", [OptValue, OptValue], purge),
        method!("EntityFind", "iterator", [], super::list_iterator::find_iterator),
        method!("EntityFind", "resultSetType", [Int], None, leaf, result_set_type),
        method!("EntityFind", "getResultSetType", [], None, leaf, get_result_set_type),
        method!(
            "EntityFind",
            "resultSetConcurrency",
            [Int],
            None,
            leaf,
            result_set_concurrency
        ),
        method!("EntityFind", "havingCondition", [Value], None, leaf, having_condition),
        method!(
            "EntityFind",
            "searchFormMap",
            [OptValue, OptValue, OptValue, OptValue, OptValue],
            Release,
            search_form_map
        ),
        method!(
            "EntityFind",
            "requireSearchFormParameters",
            [OptBool],
            None,
            leaf,
            require_search_form_parameters
        ),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// Members of an entity list that are simplest in JavaScript: iteration, Groovy's `size()` and `isEmpty()`, and the
/// array methods the type declarations name. Not enumerable, as the host methods are not.
const LIST_PROTOTYPE_JS: &str = r#"(function (list) {
    const define = (name, value) => Object.defineProperty(list, name, { value, writable: true, configurable: true });
    define(Symbol.iterator, function* () { for (let i = 0; i < this.length; i++) yield this.get(i); });
    define("size", function () { return this.length; });
    define("isEmpty", function () { return this.length === 0; });
    define("map", function (callback, self) { return Array.from(this).map(callback, self); });
    define("filter", function (callback, self) { return Array.from(this).filter(callback, self); });
    define("forEach", function (callback, self) { Array.from(this).forEach(callback, self); });
    define("toJSON", function () { return Array.from(this); });
    // The rest of the Array prototype runs on an array copy of the rows. The rows are the same values, so a field set
    // on one is kept in the list; a method that reorders (`sort`, `reverse`) returns the reordered copy and leaves the
    // list as it was.
    for (const name of ["find", "findIndex", "findLast", "findLastIndex", "some", "every", "reduce", "reduceRight",
            "sort", "reverse", "includes", "indexOf", "lastIndexOf", "slice", "concat", "join", "flatMap", "flat",
            "entries", "keys", "values", "at", "toSorted", "toReversed"]) {
        define(name, function (...args) { return Array.prototype[name].apply(Array.from(this), args); });
    }
})"#;

/// An entity list (a find's `list()`), as Java's `EntityList`. A cursor over the rows is `iterator()`'s
/// ([`super::list_iterator`]), not the list's.
pub(crate) static LIST: HostClass = HostClass {
    name: "host:EntityList",
    methods: &[
        method!("EntityList", "first", [], None, leaf, list_first),
        method!("EntityList", "last", [], None, leaf, list_last),
        method!("EntityList", "get", [Int], None, leaf, list_get),
        method!(
            "EntityList",
            "getPartialList",
            [Int, Int, OptBool],
            None,
            leaf,
            list_partial
        ),
        method!(
            "EntityList",
            "filterByDate",
            [Text, Text, Value, OptBool],
            None,
            leaf,
            list_filter_by_date
        ),
    ],
    getters: &[],
    hidden: &[],
    shim_js: Some(LIST_PROTOTYPE_JS),
    shim_members: &[],
};

/// What `entrySet()` makes of the host method's pairs: entries with `key`, `value`, `getKey()` and `getValue()`.
const VALUE_PROTOTYPE_JS: &str = r#"(function (proto, hidden) {
    const pairs = hidden.entryPairs;
    Object.defineProperty(proto, "entrySet", { writable: true, configurable: true, value: function entrySet() {
        return pairs.call(this).map(([key, value]) => ({ key, value, getKey: () => key, getValue: () => value }));
    } });
})"#;

/// An entity value's methods, as Java's `EntityValue`.
pub(crate) static ENTITY_VALUE: HostClass = HostClass {
    name: "host:EntityValue",
    methods: &[
        method!("EntityValue", "get", [Text], value_get),
        method!("EntityValue", "resolveEntityName", [], None, leaf, value_entity_name),
        method!("EntityValue", "getEntityName", [], None, leaf, value_entity_name),
        method!("EntityValue", "getEntityNamePretty", [], value_entity_name_pretty),
        method!("EntityValue", "resolveEntityNamePretty", [], value_entity_name_pretty),
        method!("EntityValue", "refresh", [], value_refresh),
        method!("EntityValue", "findRelatedFk", [OptValue], value_find_related_fk),
        method!("EntityValue", "cloneValue", [], None, leaf, value_clone),
        method!("EntityValue", "equals", [Value], None, leaf, value_equals),
        method!("EntityValue", "getMap", [], value_get_map),
        method!("EntityValue", "getPlainValueMap", [Int], value_plain_value_map),
        method!("EntityValue", "set", [Text, Value], value_set),
        method!(
            "EntityValue",
            "setFields",
            [Value, OptBool, Value, Value],
            value_set_fields
        ),
        method!("EntityValue", "create", [], value_create),
        method!("EntityValue", "update", [], value_update),
        method!("EntityValue", "delete", [], value_delete),
        method!("EntityValue", "createOrUpdate", [], value_create_or_update),
        method!("EntityValue", "getPrimaryKeys", [], value_primary_keys),
        method!("EntityValue", "setSequencedIdPrimary", [], value_sequenced_id_primary),
        method!(
            "EntityValue",
            "setSequencedIdSecondary",
            [],
            value_sequenced_id_secondary
        ),
        method!(
            "EntityValue",
            "findRelated",
            [Text, OptValue, OptValue, OptValue, OptValue, OptValue],
            value_find_related
        ),
        method!(
            "EntityValue",
            "findRelatedOne",
            [Text, OptValue, OptValue, OptValue],
            value_find_related_one
        ),
        method!(
            "EntityValue",
            "deleteWithRelated",
            [OptValue],
            value_delete_with_related
        ),
        method!("EntityValue", "deleteRelated", [Text], value_delete_related),
    ],
    getters: &[],
    hidden: &[method!("EntityValue", "entryPairs", [], value_entry_pairs)],
    shim_js: Some(VALUE_PROTOTYPE_JS),
    shim_members: &["entrySet"],
};

pub(super) fn lib(err: moqui_lib::error::Error) -> HostError {
    HostError::Lib(err)
}

/// A host value of a new entity value.
pub(crate) fn value_of(value: EntityValue) -> Value {
    Value::Object(HostObject(Arc::new(Mutex::new(value))))
}

/// A host value of a new list over `cells`.
fn list_of_cells(cells: Vec<Arc<EntityCell>>) -> Value {
    Value::Object(HostObject::new(ScriptList::of_cells(cells)))
}

/// A host value of a new list of a find's result.
fn list_of(list: EntityList) -> Value {
    Value::Object(HostObject::new(ScriptList::of_list(list)))
}

/// A host value of a new list of `rows` ([`super::cursor`]).
pub(super) fn list_value(rows: Vec<EntityValue>) -> Value {
    list_of(EntityList::new(rows))
}

// ec.entity

/// `find(name)`, a leaf: the builder over an entity of the model is made from its definition, with nothing that could
/// run a script. Another name (a DB view entity, a data document's view) is defined at run time from rows, whose find
/// may run a nested script (an entity ECA rule), so it is made with the engine lent.
fn find(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let facade = entity_facade()?;
    if facade.with_model(|model| model.entity(&name).is_ok()) {
        return Ok(find_object(facade.find(&name).map_err(lib)?));
    }
    let mut found = None;
    call.ops.lend(&mut || found = Some(facade.find(&name)))?;
    let find = found.ok_or_else(|| HostError::message("entity find was not made"))?;
    Ok(find_object(find.map_err(lib)?))
}

/// A new builder over `find`.
fn find_object(find: EntityFind) -> Ret {
    Ret::Object(HostObject::new(FindState::of(find)), &FIND)
}

fn make_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let value = entity_facade()?.make_value(&name).map_err(lib)?;
    Ok(Ret::Value(value_of(value)))
}

/// `ec.entity.startTxCacheDb(hold)`: enter sim on this thread, as Java's `TransactionCacheDb`. Only `hold=true` exists;
/// a second start while one is active is refused, as in Java.
fn start_tx_cache_db(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if call.bool(0) != Some(true) {
        return Err(HostError::message(
            "A TX cache DB that flushes to production is no longer supported. Use hold=true, which never writes to production.",
        ));
    }
    if crate::entity::cache::sim_entered() {
        return Err(HostError::message(
            "A TX cache is already active on this ExecutionContext",
        ));
    }
    let facade = entity_facade()?;
    crate::entity::cache::enter_sim(&facade).map_err(lib)?;
    Ok(Ret::Undefined)
}

/// `ec.entity.stopTxCache()`: drop this thread's sim schema. No active cache is a no-op.
fn stop_tx_cache(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    if !crate::entity::cache::sim_entered() {
        return Ok(Ret::Undefined);
    }
    let facade = entity_facade()?;
    crate::entity::cache::exit_sim(&facade).map_err(lib)?;
    Ok(Ret::Undefined)
}

fn is_tx_cache_active(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(crate::entity::cache::sim_entered()))
}

/// The whole number of an [`super::ArgKind::Whole`] argument; `None` when it was not passed or null.
fn whole_arg(call: &Call<'_>, index: usize) -> Option<i64> {
    call.int(index).map(i64::from)
}

/// `ec.entity.sequencedIdPrimary(seqName, staggerMax, bankSize)`: the next id of a sequence, as text.
fn sequenced_id_primary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let facade = entity_facade()?;
    let number = |index| whole_arg(call, index).and_then(|number| u64::try_from(number).ok());
    let id = crate::entity::sequenced_id_primary(&facade, &name, number(1), number(2)).map_err(lib)?;
    Ok(Ret::from(id))
}

/// `ec.entity.getDatabaseConfName(entityName)`: the database an entity's group is on, as `postgres`.
fn get_database_conf_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    Ok(Ret::from(entity_facade()?.database_conf_name(&name).map_err(lib)?))
}

fn temp_set_sequenced_id_primary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let facade = entity_facade()?;
    let next = whole_arg(call, 1).unwrap_or(0);
    let bank_size = whole_arg(call, 2).unwrap_or(0);
    facade.temp_set_sequenced_id_primary(&name, next, bank_size);
    Ok(Ret::Undefined)
}

fn temp_reset_sequenced_id_primary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    entity_facade()?.temp_reset_sequenced_id_primary(&name);
    Ok(Ret::Undefined)
}

// the find builder

/// The state of a find builder: its find, which each chained call changes in place, as Java's `EntityFind` builder
/// returns `this`. A run (`list()`, `one()`, `count()`) leaves it as it was, so it can run again.
pub(crate) struct FindState {
    /// `None` only while the find runs: a nested script that reaches the same builder then is refused, not blocked.
    find: Mutex<Option<EntityFind>>,
    /// The SQL the find ran, shared with its clones.
    query_texts: Arc<Mutex<Vec<String>>>,
}

impl FindState {
    pub(crate) fn of(find: EntityFind) -> Self {
        FindState {
            query_texts: find.query_texts(),
            find: Mutex::new(Some(find)),
        }
    }

    /// A clone of the find; `None` only while it runs.
    pub(crate) fn peek(&self) -> Option<EntityFind> {
        lock(&self.find).as_ref().cloned()
    }
}

fn running() -> HostError {
    HostError::message("entity find is running")
}

/// Change the receiver's find in place with `step` and return the receiver.
fn chain(call: &Call<'_>, step: impl FnOnce(EntityFind) -> EntityFind) -> Result<Ret, HostError> {
    let state = call.this::<FindState>()?;
    let mut slot = lock(&state.find);
    let find = slot.take().ok_or_else(running)?;
    *slot = Some(step(find));
    Ok(Ret::This)
}

/// The receiver's find, taken out of its builder while it runs; [`put_back`] returns it.
fn take_find(call: &Call<'_>) -> Result<(Arc<FindState>, EntityFind), HostError> {
    let state = call.this::<FindState>()?;
    let taken = lock(&state.find).take();
    let find = taken.ok_or_else(running)?;
    Ok((state, find))
}

fn put_back(state: &FindState, find: EntityFind) {
    *lock(&state.find) = Some(find);
}

/// `body` over the receiver's find, which stays the builder's: a run, as Java's, does not use the find up. No lock is
/// held while it runs, so a nested script it runs may use other finds.
fn run_find<T>(call: &Call<'_>, body: impl FnOnce(&EntityFind) -> T) -> Result<T, HostError> {
    let (state, find) = take_find(call)?;
    let result = body(&find);
    put_back(&state, find);
    Ok(result)
}

/// A clone of the receiver's find, which stays usable.
fn peek_find(call: &Call<'_>) -> Result<EntityFind, HostError> {
    call.this::<FindState>()?.peek().ok_or_else(running)
}

/// A clone of the receiver's find, for a method kept in another module ([`super::cursor`]).
pub(super) fn peek_find_of(call: &Call<'_>) -> Result<EntityFind, HostError> {
    peek_find(call)
}

/// The text of a value argument that must be a string.
fn string_arg(value: &Value) -> Result<String, HostError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        _ => Err(HostError::message("expected a string")),
    }
}

fn condition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let first = call.value_or_null(0);
    // `condition(field, operator, value)`: the operator is a name Java's factory knows, an empty one is equals.
    if call.passed(1) && call.passed(2) {
        let field = string_arg(&first)?;
        let operator = match call.value(1) {
            None | Some(Value::Null) => String::new(),
            Some(operator) => string_arg(operator)?,
        };
        let condition = comparison(&field, &operator, call.value_or_null(2)).map_err(lib)?;
        return chain(call, |find| find.condition(condition));
    }
    if !call.passed(1) {
        if first.is_null() {
            return chain(call, |find| find);
        }
        if let Some(condition) = condition_in(&first) {
            return chain(call, |find| find.condition(condition));
        }
    }
    if call.passed(1) {
        let field = string_arg(&first)?;
        let condition = comparison(&field, "equals", call.value_or_null(1)).map_err(lib)?;
        return chain(call, |find| find.condition(condition));
    }
    let entries = match &first {
        Value::List(_) => IndexMap::new(),
        other => crate::script::value_map(other)
            .ok_or_else(|| HostError::message("entity condition needs a field and a value"))?,
    };
    // A map such as `context` carries names that are not fields. Only fields (a view's aliases) apply. The definition
    // of an entity defined at run time may be read from rows, so it is read with the engine lent (`find`).
    let facade = entity_facade()?;
    let (state, mut find) = take_find(call)?;
    let mut fields = None;
    let lent = call.ops.lend(&mut || {
        fields = Some(facade.with_entity(find.entity_name(), |entity, _, _| {
            Ok(entity.field_names().into_iter().map(str::to_string).collect())
        }));
    });
    let known: Result<std::collections::BTreeSet<String>, HostError> = lent.and_then(|()| {
        fields
            .ok_or_else(|| HostError::message("entity fields were not read"))?
            .map_err(lib)
    });
    let known = match known {
        Ok(known) => known,
        Err(err) => {
            put_back(&state, find);
            return Err(err);
        }
    };
    let mut failed = None;
    for (key, value) in entries {
        if !known.contains(&key) {
            continue;
        }
        match comparison(&key, "equals", value) {
            Ok(condition) => find = find.condition_from_map(condition),
            Err(err) => {
                failed = Some(err);
                break;
            }
        }
    }
    put_back(&state, find);
    match failed {
        Some(err) => Err(lib(err)),
        None => Ok(Ret::This),
    }
}

/// Comma separated fields, each with an optional `-` for descending, `+`, or `^` for ignore case.
fn order_by(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let fields = call.text_or_empty(0);
    chain(call, |mut find| {
        for part in fields.split(',') {
            find = find.order_by_text(part);
        }
        find
    })
}

/// The integer of a `limit` or `offset` argument, `what`. A number is itself. Text is what an entity-find's
/// `limit="${expr}"` expands to, read as Java's `as Integer`: trimmed and parsed, a whole number in decimal form too.
/// Null, empty text or text that is not a number is `None`, no limit, logged at debug: the find still runs.
fn find_count(what: &str, value: &Value) -> Result<Option<i64>, HostError> {
    let text = match value {
        Value::Null => return Ok(None),
        Value::I32(number) => return Ok(Some(i64::from(*number))),
        Value::I64(number) => return Ok(Some(*number)),
        Value::F64(_) => return Err(HostError::message("limit is not an integer")),
        Value::String(text) => text,
        _ => return Err(HostError::message("limit is not a number")),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(number) = trimmed.parse::<i64>() {
        return Ok(Some(number));
    }
    #[allow(clippy::cast_possible_truncation)]
    match trimmed.parse::<f64>() {
        Ok(number) if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 => Ok(Some(number as i64)),
        _ => {
            tracing::debug!("entity find {what} {trimmed:?} is not an integer; no {what} applied");
            Ok(None)
        }
    }
}

/// `limit(count)`: a number, or the text an expanded `limit="${expr}"` of an entity-find gives (see [`find_count`]).
fn limit(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let count = find_count("limit", &call.value_or_null(0))?;
    chain(call, |find| match count {
        Some(number) => find.limit(number),
        None => find,
    })
}

/// `offset(offset)`, or `offset(pageIndex, pageSize)` for the offset `pageIndex * pageSize`, as Java's. The page size
/// is not kept: `limit` sets it.
fn offset(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(mut number) = find_count("offset", &call.value_or_null(0))? else {
        return chain(call, |find| find);
    };
    if let Some(size) = call.value(1) {
        if let Some(size) = find_count("page size", size)? {
            number = number.saturating_mul(size);
        }
    }
    chain(call, |find| find.offset(number))
}

/// `conditionDate(fromField, thruField, moment)`: the rows in effect at the moment, or now when it is empty.
fn condition_date(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let moment = call.value_or_null(2);
    let condition = date_range_condition(&call.text_or_empty(0), &call.text_or_empty(1), moment, false);
    chain(call, |find| match condition {
        Some(condition) => find.condition(condition),
        None => find,
    })
}

/// `getPageIndex()`: `offset / pageSize`, 0 with no offset.
fn get_page_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let find = peek_find(call)?;
    let size = find.limit_value().filter(|limit| *limit != 0).unwrap_or(20);
    Ok(Ret::Value(Value::I64(
        find.offset_value().map_or(0, |offset| offset / size),
    )))
}

/// `getPageSize()`: the limit, 20 with none.
fn get_page_size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::I64(peek_find(call)?.limit_value().unwrap_or(20))))
}

/// `updateAll(fieldsToSet)`: set the fields on each row found and update the ones that change. Returns how many rows
/// were updated. The find is not cached, so every row is read and its entity ECA rules run.
fn update_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Value::Map(fields) = call.value_or_null(0) else {
        return Err(HostError::message("updateAll needs a map of fields to set"));
    };
    let updated = peek_find(call)?.update_all(fields).map_err(lib)?;
    Ok(Ret::Value(Value::I64(updated)))
}

/// A flag as `useCache` and `useClone` read it: a boolean, or the text `true`.
fn flag_on(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text == "true",
        _ => false,
    }
}

/// `useCache(flag)`: through the entity cache for any entity not marked `never`, as Java's `EntityFind.useCache`. A null
/// or undefined flag leaves the entity's own setting.
fn use_cache(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let flag = call.value_or_null(0);
    if flag.is_null() {
        return chain(call, |find| find);
    }
    let on = flag_on(&flag);
    chain(call, |find| find.use_cache(on))
}

/// A flag as `forUpdate` and `distinct` read it: a boolean, true for anything else.
fn flag_or_true(call: &Call<'_>) -> bool {
    match call.value(0) {
        Some(Value::Bool(flag)) => *flag,
        _ => true,
    }
}

fn for_update(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag_or_true(call);
    chain(call, |find| find.for_update(on))
}

/// `forNoKeyUpdate(flag)`, an MR Moqui addition: `forUpdate` with Postgres's `FOR NO KEY UPDATE`
/// ([`crate::entity::EntityFind::for_no_key_update`]).
fn for_no_key_update(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag_or_true(call);
    chain(call, |find| find.for_no_key_update(on))
}

/// Java's `resultSetType(type)`: 1003 (`TYPE_FORWARD_ONLY`, the default) gives an `iterator()` that reads forward; 1004
/// and 1005 one that moves back and jumps.
fn result_set_type(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let kind = call.int(0).unwrap_or(crate::entity::TYPE_FORWARD_ONLY);
    chain(call, |find| find.result_set_type(kind))
}

fn get_result_set_type(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let kind = run_find(call, EntityFind::get_result_set_type)?;
    Ok(Ret::Value(Value::I32(kind)))
}

/// Java's `resultSetConcurrency`: an iterator here is read only, so the value is not used.
fn result_set_concurrency(call: &mut Call<'_>) -> Result<Ret, HostError> {
    chain(call, |find| find)
}

fn distinct(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag_or_true(call);
    chain(call, |find| find.distinct(on))
}

fn disable_authz(call: &mut Call<'_>) -> Result<Ret, HostError> {
    chain(call, EntityFind::disable_authz)
}

/// `useClone(flag)`: read from a clone datasource of the entity's group when one is open.
fn use_clone(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag_on(&call.value_or_null(0));
    chain(call, |find| find.use_clone(on))
}

fn select_field(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = call.text_or_empty(0);
    chain(call, |find| find.select_field(&field))
}

/// The shown and hidden fields of a form-list, for its `entity-find` (generated code only).
fn select_form_fields(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let fields = call.text_or_empty(0);
    chain(call, |find| find.select_form_fields(&fields))
}

/// `list()`, a leaf: a hit of the entity cache that nothing on the way could turn into a nested script (see
/// [`EntityFind::list_from_cache`]) is answered without lending the engine; any other find runs with it lent, as a
/// read may run a nested script (an authorization filter, a definition made from rows).
fn list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (state, find) = take_find(call)?;
    let found = run_lent(call, &find, EntityFind::list_from_cache, EntityFind::list);
    put_back(&state, find);
    Ok(Ret::Value(list_of(found?)))
}

/// `one()`, a leaf as [`list`].
fn one(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (state, find) = take_find(call)?;
    let found = run_lent(call, &find, EntityFind::one_from_cache, EntityFind::one);
    put_back(&state, find);
    Ok(Ret::Value(found?.map_or(Value::Null, value_of)))
}

/// `cached(find)` when it answers, else `run(find)` with the engine lent.
fn run_lent<T>(
    call: &mut Call<'_>,
    find: &EntityFind,
    cached: fn(&EntityFind) -> Result<Option<T>, moqui_lib::error::Error>,
    run: fn(&EntityFind) -> Result<T, moqui_lib::error::Error>,
) -> Result<T, HostError> {
    if let Some(hit) = cached(find).map_err(lib)? {
        return Ok(hit);
    }
    let mut found = None;
    call.ops.lend(&mut || found = Some(run(find)))?;
    found
        .ok_or_else(|| HostError::message("entity find did not run"))?
        .map_err(lib)
}

fn one_master(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let facade = entity_facade()?;
    let master = call.text(0).unwrap_or("default").to_string();
    match run_find(call, EntityFind::one)?.map_err(lib)? {
        Some(value) => Ok(Ret::Value(
            crate::entity::query_rest::master_value(&facade, &value, &master).map_err(lib)?,
        )),
        None => Ok(Ret::Value(Value::Null)),
    }
}

fn count(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::I64(run_find(call, EntityFind::count)?.map_err(lib)?)))
}

/// `getQueryTextList()`: the SQL the find ran, without its bound values; there after the find was used up.
fn get_query_text_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let state = call.this::<FindState>()?;
    let texts = lock(&state.query_texts).clone();
    Ok(Ret::from(texts))
}

/// `getLimit()`: the limit of the find, or null. Does not use the find up.
fn get_limit(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(
        peek_find(call)?.limit_value().map_or(Value::Null, Value::I64),
    ))
}

/// `getOffset()`: the offset of the find, or null. Does not use the find up.
fn get_offset(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(
        peek_find(call)?.offset_value().map_or(Value::Null, Value::I64),
    ))
}

/// A copy of the find, so a page's `list()` and the `count()` of all rows can both run.
fn copy(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(find_object(peek_find(call)?))
}

/// Delete the rows the find matches. Returns how many, as Java's `deleteAll`.
fn delete_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::I64(peek_find(call)?.delete_all().map_err(lib)?)))
}

/// The number a value argument holds, `None` for anything else.
fn number_arg(call: &Call<'_>, index: usize) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    match call.value(index)? {
        Value::I32(number) => Some(f64::from(*number)),
        Value::I64(number) => Some(*number as f64),
        Value::F64(number) => Some(*number),
        _ => None,
    }
}

/// `purge(maxSeconds, chunkSize)`: delete the rows the find matches, each chunk of rows by primary key in a transaction
/// of its own, a failed chunk logged and passed over, and no chunk begun after `maxSeconds` (none: no limit). Returns
/// how many rows the committed chunks deleted. For the cleanup jobs whose backlog may not fit one transaction
/// (plans/High-Level-Review.md HL-033).
fn purge(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let deadline = number_arg(call, 0)
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds));
    let mut find = peek_find(call)?;
    if let Some(rows) = number_arg(call, 1).filter(|rows| rows.is_finite() && *rows >= 1.0) {
        // A finite number of at least 1: the cast saturates at most.
        #[allow(clippy::cast_possible_truncation)]
        let rows = rows as i64;
        find = find.bulk_chunk(rows);
    }
    let purged = find.purge(deadline).map_err(lib)?;
    Ok(Ret::Value(Value::I64(purged.deleted)))
}

/// A condition on an aggregate alias of a view-entity. A null condition is skipped.
fn having_condition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let condition = condition_in(&call.value_or_null(0));
    chain(call, |find| match condition {
        Some(condition) => find.having(condition),
        None => find,
    })
}

/// `searchFormMap(inputFields, defaultParameters, skipFields, defaultOrderBy, paginate)`. The defaults and `orderByField`
/// it applied are put in the context, so the form shows them (the engine's tracked values are written back and released
/// first, so a later read sees them).
fn search_form_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map_arg = |index: usize| -> Result<BTreeMap<String, Value>, HostError> {
        match call.value(index) {
            None | Some(Value::Null) => Ok(BTreeMap::new()),
            Some(Value::Map(entries)) => Ok(entries.clone().into_iter().collect()),
            Some(_) => Err(HostError::message("searchFormMap needs maps of fields")),
        }
    };
    let text_arg = |index: usize| moqui_lib::object::to_plain_string(&call.value_or_null(index));
    let input = map_arg(0)?;
    let defaults = map_arg(1)?;
    let paginate = matches!(call.value(4), Some(Value::Bool(true)));
    let found = peek_find(call)?
        .search_form_map(&input, &defaults, &text_arg(2), &text_arg(3), paginate)
        .map_err(lib)?;
    for (name, value) in found.context {
        crate::script::run::STACK
            .with(|stack| stack.borrow_mut().put(&name, value).map(|_| ()))
            .map_err(|err| HostError::message(err.message()))?;
    }
    let state = call.this::<FindState>()?;
    *lock(&state.find) = Some(found.find);
    Ok(Ret::This)
}

fn require_search_form_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let require = call.bool(0).unwrap_or(true);
    chain(call, |find| find.require_search_form_parameters(require))
}

// the entity list

/// A find result as scripts and the context stack hold it. Each row is its own shared cell, so a field set through
/// `list[0]`, `get`, `for...of` or an `<iterate>` entry is kept in the list, as on a Java `List` of values. The rows are
/// copied from the [`EntityList`] the find returned, so the entity cache's list is not changed.
pub(crate) struct ScriptList {
    cells: Vec<Arc<EntityCell>>,
}

impl ScriptList {
    pub(crate) fn of_list(list: EntityList) -> Self {
        Self::of_cells(
            list.into_values()
                .into_iter()
                .map(|value| Arc::new(Mutex::new(value)))
                .collect(),
        )
    }

    pub(crate) fn of_cells(cells: Vec<Arc<EntityCell>>) -> Self {
        Self { cells }
    }

    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn cells(&self) -> &[Arc<EntityCell>] {
        &self.cells
    }

    /// The rows as they are now.
    pub(crate) fn values(&self) -> Vec<EntityValue> {
        self.cells.iter().map(|cell| lock(cell).clone()).collect()
    }
}

/// The state of one script object of a list: its rows.
pub(crate) struct ListState {
    pub(crate) list: Arc<ScriptList>,
}

impl ListState {
    pub(crate) fn of(list: Arc<ScriptList>) -> Self {
        ListState { list }
    }

    /// The row at `index` itself, not a copy, so a field set on it is kept in the list; null outside the rows.
    pub(crate) fn item(&self, index: usize) -> Value {
        match self.list.cells().get(index) {
            Some(cell) => Value::Object(HostObject::new(ListRow(Arc::clone(cell)))),
            None => Value::Null,
        }
    }
}

/// A row of a list on its way to a script: an engine makes a new entity value object of it each time, as QuickJS does
/// (`list[0] !== list[0]`, the same row underneath), and keeps no identity for it, so a loop over many rows holds none of
/// them. Only a method's result and an index read are this; the value a script passes on is the row's cell.
pub(crate) struct ListRow(pub(crate) Arc<EntityCell>);

fn this_list(call: &Call<'_>) -> Result<Arc<ListState>, HostError> {
    call.this::<ListState>()
}

/// Groovy's `List.first()`: the first row, null for an empty list.
fn list_first(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(this_list(call)?.item(0)))
}

/// Groovy's `List.last()`: the last row, null for an empty list.
fn list_last(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = this_list(call)?;
    Ok(Ret::Value(match list.list.len() {
        0 => Value::Null,
        len => list.item(len - 1),
    }))
}

fn list_get(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = this_list(call)?;
    let index = call.int(0).unwrap_or(0);
    if index < 0 {
        return Ok(Ret::Value(Value::Null));
    }
    let index = usize::try_from(index).map_err(|_| HostError::message("list index is too large"))?;
    Ok(Ret::Value(list.item(index)))
}

fn list_partial(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = this_list(call)?;
    let start = usize::try_from(call.int(0).unwrap_or(0).max(0)).unwrap_or(0);
    let take = usize::try_from(call.int(1).unwrap_or(0).max(0)).unwrap_or(0);
    // The part shares the rows, as a Java sublist shares its elements.
    let cells = list.list.cells().iter().skip(start).take(take).cloned().collect();
    Ok(Ret::Value(list_of_cells(cells)))
}

fn list_filter_by_date(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = this_list(call)?;
    let from_name = call.text_or_empty(0);
    let thru_name = call.text_or_empty(1);
    let valid = call.value_or_null(2);
    let empty = moqui_lib::object::is_empty(&valid);
    let cells = if empty && call.bool(3).unwrap_or(false) {
        list.list.cells().to_vec()
    } else {
        let valid = if empty { now_timestamp() } else { timestamp_of(&valid) };
        list.list
            .cells()
            .iter()
            .filter(|cell| {
                let row = lock(cell);
                let from = timestamp_of(row.get(&from_name).unwrap_or(&Value::Null));
                let thru = timestamp_of(row.get(&thru_name).unwrap_or(&Value::Null));
                let started = from.is_null()
                    || moqui_lib::object::compare_obj(&from, &valid)
                        .is_ok_and(|ordering| ordering != Ordering::Greater);
                let open = thru.is_null()
                    || moqui_lib::object::compare_obj(&thru, &valid)
                        .is_ok_and(|ordering| ordering == Ordering::Greater);
                started && open
            })
            .cloned()
            .collect()
    };
    Ok(Ret::Value(list_of_cells(cells)))
}

// the entity value

/// What a name that is not a method stands for on an entity value.
pub(crate) enum Member {
    Field,
    One,
    Many,
    Unknown,
}

fn val(cell: &EntityCell) -> MutexGuard<'_, EntityValue> {
    lock(cell)
}

fn installed_facade() -> Option<EntityFacade> {
    crate::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
}

/// What `name` is on the entity of the value, or None when no entity definition can be reached.
pub(crate) fn member(cell: &EntityCell, name: &str) -> Option<Member> {
    let facade = installed_facade()?;
    let entity_name = val(cell).entity_name().to_string();
    facade
        .with_entity(&entity_name, |entity, _, _| {
            if entity.has_field(name) {
                return Ok(Member::Field);
            }
            Ok(match entity.relationship(name) {
                Some(relationship) if relationship.relationship_type.starts_with("one") => Member::One,
                Some(_) => Member::Many,
                None => Member::Unknown,
            })
        })
        .ok()
}

/// A field as a script reads it, in the user's locale when it is a localized field, and a view's `pq-expression` alias
/// evaluated, as Java's `get`. A lookup or an expression runs with the value unlocked: a lookup is a find, and an
/// expression may read this same value.
pub(crate) fn field_value(cell: &EntityCell, name: &str) -> Option<Value> {
    let value = val(cell);
    if !value.localizes(name) {
        return value.get(name).cloned();
    }
    let Some(facade) = installed_facade() else {
        return value.get(name).cloned();
    };
    let read = value.read_localized(&facade, name);
    drop(value);
    match read {
        Ok(found) => found,
        Err(read) => read.run(&facade),
    }
}

/// [`field_value`], except for a `pq-expression` alias with the name of a relationship: as Java's `get`, the
/// relationship comes first, so that is None.
pub(crate) fn held_value(cell: &EntityCell, name: &str) -> Option<Value> {
    let pq_alias = val(cell).is_pq_alias(name);
    if pq_alias && matches!(member(cell, name), Some(Member::One | Member::Many)) {
        return None;
    }
    field_value(cell, name)
}

/// The fields as a plain map, the localized ones in the user's locale, with a view's `pq-expression` aliases, as Java's
/// `getMap`.
fn field_map(cell: &EntityCell) -> BTreeMap<String, Value> {
    let value = val(cell);
    if !value.has_l10n() {
        return crate::entity::string_map(value.fields());
    }
    let copy = value.clone();
    drop(value);
    match installed_facade() {
        Some(facade) => crate::entity::string_map(&copy.localized_fields(&facade)),
        None => crate::entity::string_map(copy.fields()),
    }
}

/// Whether `name` is a field: one the value holds, or one its entity defines.
pub(crate) fn is_field(cell: &EntityCell, name: &str) -> bool {
    val(cell).fields().contains_key(name) || matches!(member(cell, name), Some(Member::Field))
}

/// What reading `name` of an entity value finds, without an engine.
pub(crate) enum EntityField {
    /// The field's value as a script reads it ([`held_value`]).
    Value(Value),
    /// A field the entity defines that the value does not hold.
    Null,
    /// A relationship: its value is a find ([`relationship_value`]).
    Relationship,
    /// A name that is neither, or no entity definition to tell: undefined.
    Unknown,
}

/// [`EntityField`] of `name` on the value in `cell`, as a property read reads it.
pub(crate) fn entity_field(cell: &Arc<EntityCell>, name: &str) -> EntityField {
    if let Some(value) = held_value(cell, name) {
        return EntityField::Value(value);
    }
    match member(cell, name) {
        Some(Member::Field) => EntityField::Null,
        Some(Member::One | Member::Many) => EntityField::Relationship,
        Some(Member::Unknown) | None => EntityField::Unknown,
    }
}

/// [`entity_field`] when the read needs no find and no expression (a localized field the memos hold): `None` when it
/// does, to be read with the engine lent. It runs no script.
pub(crate) fn entity_field_memoized(cell: &Arc<EntityCell>, name: &str) -> Option<EntityField> {
    let value = val(cell);
    if value.is_pq_alias(name) {
        return None;
    }
    let found = if !value.localizes(name) {
        value.get(name).cloned()
    } else {
        let facade = installed_facade()?;
        let read = value.read_localized(&facade, name);
        drop(value);
        match read {
            Ok(found) => found,
            Err(read) => read.run_memoized(&facade).ok()?,
        }
    };
    Some(match found {
        Some(value) => EntityField::Value(value),
        None => match member(cell, name) {
            Some(Member::Field) => EntityField::Null,
            Some(Member::One | Member::Many) => EntityField::Relationship,
            Some(Member::Unknown) | None => EntityField::Unknown,
        },
    })
}

/// Whether reading `name` may run a find or an expression (a localized field, a view's `pq-expression` alias), which
/// may run a nested script: such a read is made with the engine lent.
pub(crate) fn entity_field_reads(cell: &Arc<EntityCell>, name: &str) -> bool {
    let value = val(cell);
    value.localizes(name) || value.is_pq_alias(name)
}

/// Whether `name` is a field of the value in `cell`: one it holds, or one its entity defines.
pub(crate) fn entity_is_field(cell: &Arc<EntityCell>, name: &str) -> bool {
    is_field(cell, name)
}

/// The names of the fields the value in `cell` holds, for `Object.keys` and a spread.
pub(crate) fn entity_field_names(cell: &Arc<EntityCell>) -> Vec<String> {
    lock(cell).fields().keys().map(|name| name.to_string()).collect()
}

/// Set `field` of the value in `cell` to `value`, as Java's `put`: a name the entity defines as other than a field is an
/// error; with no entity definition to tell, any name is set.
pub(crate) fn entity_set_field(cell: &Arc<EntityCell>, field: String, value: Value) -> Result<(), String> {
    let known = val(cell).fields().contains_key(field.as_str());
    if !known && member(cell, &field).is_some_and(|member| !matches!(member, Member::Field)) {
        let entity_name = val(cell).entity_name().to_string();
        return Err(format!("The field name {field} is not valid for entity {entity_name}"));
    }
    val(cell).set(field, value);
    Ok(())
}

/// The find of the rows a relationship of the value names.
pub(crate) fn related_find(cell: &EntityCell, relationship: &str) -> Result<EntityFind, moqui_lib::error::Error> {
    let facade = installed_facade().ok_or_else(|| moqui_lib::error::Error::new("no entity facade is installed"))?;
    let record = val(cell).clone();
    let (entity, conditions) =
        crate::service::call::related_conditions(&facade, record.entity_name(), relationship, &record)?;
    let mut find = facade.find(&entity)?;
    for (field, value) in conditions {
        find = find.eq(&field, value);
    }
    Ok(find)
}

/// The related value (a `one` relationship, null when there is none) or list (a `many`) of a relationship name, as
/// Java's `EntityValueBase.get` resolves one; `None` for a name that is not a relationship.
pub(crate) fn relationship_value(cell: &EntityCell, name: &str) -> Result<Option<Value>, moqui_lib::error::Error> {
    Ok(match member(cell, name) {
        Some(Member::One) => Some(related_find(cell, name)?.one()?.map_or(Value::Null, value_of)),
        Some(Member::Many) => Some(list_of(related_find(cell, name)?.list()?)),
        _ => None,
    })
}

/// A field value, null for a field the value does not hold, or the related value or list for a relationship name;
/// `None` (undefined) for any other name.
pub(crate) fn read_member(cell: &EntityCell, name: &str) -> Result<Option<Value>, moqui_lib::error::Error> {
    if let Some(value) = held_value(cell, name) {
        return Ok(Some(value));
    }
    if let Some(Member::Field) = member(cell, name) {
        return Ok(Some(Value::Null));
    }
    relationship_value(cell, name)
}

fn this_cell(call: &Call<'_>) -> Result<Arc<EntityCell>, HostError> {
    call.this::<EntityCell>()
}

/// A copy of the value, for a write: the write runs entity ECA rules, and a rule that reads this same value must not
/// find the cell locked. The writes take the value by reference and set nothing on it.
fn snapshot(cell: &EntityCell) -> EntityValue {
    val(cell).clone()
}

/// Run `write` on a [`snapshot`]; after it, the value holds what the database does, so a later update writes only the
/// fields set after this write.
fn write(
    call: &Call<'_>,
    write: impl FnOnce(&EntityValue, &EntityFacade) -> Result<(), moqui_lib::error::Error>,
) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    let written = snapshot(&cell);
    write(&written, &facade).map_err(lib)?;
    val(&cell).note_written(&written);
    Ok(Ret::Undefined)
}

/// `get(name)`: a field, or as Java's `EntityValue.get` the related value or list for a relationship name; null for any
/// other name.
fn value_get(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let name = call.text_or_empty(0);
    Ok(Ret::Value(
        read_member(&cell, &name).map_err(lib)?.unwrap_or(Value::Null),
    ))
}

fn value_entity_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let name = val(&cell).entity_name().to_string();
    Ok(Ret::from(name))
}

/// `getEntityNamePretty()` (Java's `resolveEntityNamePretty`): the entity's short name in words, `OrderPart` as `Order
/// Part`.
fn value_entity_name_pretty(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    let name = val(&cell).entity_name().to_string();
    let pretty = facade
        .with_entity(&name, |entity, _, _| {
            Ok(moqui_lib::string::camel_case_to_pretty(&entity.entity_name))
        })
        .map_err(lib)?;
    Ok(Ret::from(pretty))
}

/// `refresh()`: read the row of this value's primary key again and replace the fields with it. False, and the value as
/// it was, when there is no such row.
fn value_refresh(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    let current = snapshot(&cell);
    let keys = facade.pk_map(current.entity_name(), current.fields()).map_err(lib)?;
    match facade.find_by_pk(current.entity_name(), &keys).map_err(lib)? {
        Some(fresh) => {
            *val(&cell) = fresh;
            Ok(Ret::from(true))
        }
        None => Ok(Ret::from(false)),
    }
}

/// The fields as `[key, value]` pairs, for the shim's `entrySet()`.
fn value_entry_pairs(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let pairs = field_map(&cell)
        .into_iter()
        .map(|(name, value)| Value::List(vec![Value::String(name), value]))
        .collect();
    Ok(Ret::Value(Value::List(pairs)))
}

/// The texts of an optional value argument; none when it was not passed, null or undefined.
fn texts_arg(call: &Call<'_>, index: usize) -> Vec<String> {
    match call.value(index) {
        Some(value) if !value.is_null() => host_texts(value.clone()),
        _ => Vec::new(),
    }
}

/// The rows of the entities that hold a foreign key to the value (a `many` relationship whose reverse is a `one` of the
/// related entity), but those whose entity is one of `skip`.
fn related_fk_rows(cell: &EntityCell, skip: &[String]) -> Result<Vec<EntityValue>, HostError> {
    let facade = entity_facade()?;
    let parent = val(cell).entity_name().to_string();
    let names: Vec<String> = facade
        .with_entity(&parent, |entity, _, model| {
            let mut names = Vec::new();
            for relationship in &entity.relationships {
                let Some(related) = model.entity_full(&relationship.related) else {
                    continue;
                };
                // The reverse: a one relationship of the related entity back to this one, with the same title.
                let reverse_is_one = related.relationships.iter().any(|back| {
                    back.relationship_type.starts_with("one")
                        && back.title == relationship.title
                        && (back.related == entity.full_name || back.related_entity_name == entity.entity_name)
                });
                let skipped = skip.iter().any(|name| {
                    *name == related.full_name
                        || *name == related.entity_name
                        || related.short_alias.as_deref() == Some(name.as_str())
                });
                if reverse_is_one && !skipped {
                    names.push(relationship.name.clone());
                }
            }
            Ok(names)
        })
        .map_err(lib)?;
    let mut rows = Vec::new();
    for name in names {
        let list = related_find(cell, &name).and_then(|find| find.list()).map_err(lib)?;
        rows.extend(list.into_values());
    }
    Ok(rows)
}

/// `findRelatedFk(skipEntities)`: every row of the entities that hold a foreign key to this value, as one list. The
/// names to skip are full names, short aliases, or entity names, in an array or a set.
fn value_find_related_fk(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let rows = related_fk_rows(&cell, &texts_arg(call, 0))?;
    Ok(Ret::Value(list_of(EntityList::new(rows))))
}

/// `cloneValue()`: a separate value with the same entity and fields, as Java's `EntityValue.cloneValue`. Changes to
/// either do not reach the other.
fn value_clone(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let copy = val(&*this_cell(call)?).clone();
    Ok(Ret::Value(value_of(copy)))
}

/// `equals(other)`: the same entity with the same field values, as Java's `EntityValueBase.equals`; a script's `==` on
/// two values is identity, so a service that asks whether an edit changed anything calls this.
fn value_equals(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let other = call.value_or_null(0);
    let theirs: IndexMap<String, Value> = match &other {
        Value::Map(entries) => entries.clone(),
        Value::Object(object) => match object.downcast::<EntityCell>() {
            Some(theirs) => val(&theirs)
                .fields()
                .iter()
                .map(|(key, item)| (key.to_string(), item.clone()))
                .collect(),
            None => return Ok(Ret::from(false)),
        },
        _ => return Ok(Ret::from(false)),
    };
    // a map has no entity name, so it is never equal (runtime/values.rs `value_entity_name`)
    let Some(name) = (match &other {
        Value::Object(object) => object
            .downcast::<EntityCell>()
            .map(|theirs| val(&theirs).entity_name().to_string()),
        _ => None,
    }) else {
        return Ok(Ret::from(false));
    };
    let mine = val(&cell);
    if mine.entity_name() != name {
        return Ok(Ret::from(false));
    }
    let fields = mine.fields();
    let comparable = |value: &Value| -> String {
        if value.is_null() {
            String::new()
        } else {
            moqui_lib::object::to_plain_string(value)
        }
    };
    let same = |name: &str| {
        comparable(fields.get(name).unwrap_or(&Value::Null)) == comparable(theirs.get(name).unwrap_or(&Value::Null))
    };
    Ok(Ret::from(
        fields.keys().all(|name| same(name)) && theirs.keys().all(|name| same(name)),
    ))
}

/// `getMap()`: the fields as a plain map, the localized ones in the user's locale.
fn value_get_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map: IndexMap<String, Value> = field_map(&*this_cell(call)?).into_iter().collect();
    Ok(Ret::Value(Value::Map(map)))
}

/// `getPlainValueMap(dependentLevels)`: the fields as a plain map, the nulls removed, with `_entity` the short-or-full
/// entity name, and when `dependentLevels` is greater than 0 the values of the dependent relationships, as Java's
/// `getPlainValueMap`.
fn value_plain_value_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    let levels = call.int(0).unwrap_or(0);
    let map = plain_value_map(&facade, &snapshot(&cell), levels, &[]).map_err(lib)?;
    Ok(Ret::Value(Value::Map(map)))
}

/// The value's fields, the nulls and the parent's PK fields removed, with `_entity`, and the values of the dependent
/// relationships down `levels`, as Java's `internalPlainValueMap`.
fn plain_value_map(
    facade: &EntityFacade,
    value: &EntityValue,
    levels: i32,
    parent_pks: &[String],
) -> Result<IndexMap<String, Value>, moqui_lib::error::Error> {
    let mut map = IndexMap::new();
    for (name, field) in crate::entity::string_map(value.fields()) {
        if field.is_null() || parent_pks.iter().any(|pk| pk == &name) {
            continue;
        }
        map.insert(name, field);
    }
    let entity_name = value.entity_name().to_string();
    let (entity_label, pk_names, relationships) = facade.with_entity(&entity_name, |entity, _, model| {
        let label = entity.short_alias.clone().unwrap_or_else(|| entity.full_name.clone());
        let pks = entity
            .pk_field_names()
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        let rels = entity
            .relationships
            .iter()
            .filter(|rel| crate::entity::info::is_dependent(model, entity, rel))
            .map(|rel| {
                let entry = rel
                    .short_alias
                    .clone()
                    .filter(|alias| !alias.is_empty())
                    .unwrap_or_else(|| rel.name.clone());
                (entry, rel.name.clone(), rel.relationship_type.clone())
            })
            .collect::<Vec<_>>();
        Ok((label, pks, rels))
    })?;
    map.insert("_entity".to_string(), Value::String(entity_label));
    if levels > 0 {
        let mut pks = pk_names;
        pks.extend(parent_pks.iter().cloned());
        for (entry, rel_name, rel_type) in relationships {
            let (target, conditions) =
                crate::service::call::related_conditions(facade, &entity_name, &rel_name, value)?;
            let mut find = facade.find(&target)?;
            for (field, condition) in conditions {
                find = find.eq(&field, condition);
            }
            if rel_type.starts_with("one") {
                if let Some(related) = find.one()? {
                    map.insert(entry, Value::Map(plain_value_map(facade, &related, levels - 1, &pks)?));
                }
            } else {
                let found = find.list()?;
                let values = found.values();
                if !values.is_empty() {
                    let items = values
                        .iter()
                        .map(|related| Ok(Value::Map(plain_value_map(facade, related, levels - 1, &pks)?)))
                        .collect::<Result<Vec<_>, moqui_lib::error::Error>>()?;
                    map.insert(entry, Value::List(items));
                }
            }
        }
    }
    Ok(map)
}

fn value_set(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let field = call.text_or_empty(0);
    entity_set_field(&cell, field, call.value_or_null(1)).map_err(HostError::Message)?;
    Ok(Ret::Undefined)
}

/// Which fields `setFields` sets: all, the primary key, or the others.
enum FieldInclude {
    All,
    Pk,
    NonPk,
}

fn value_set_fields(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let set_empty = call.bool(1).unwrap_or(false);
    let prefix = match call.value(2) {
        None | Some(Value::Null) => String::new(),
        Some(value) => string_arg(value)?,
    };
    let include = match call.value(3) {
        Some(Value::Bool(true)) => FieldInclude::Pk,
        Some(Value::Bool(false)) => FieldInclude::NonPk,
        _ => FieldInclude::All,
    };
    let Some(source) = setfields_source(call.value(0)) else {
        return Err(HostError::message("setFields needs an object"));
    };
    let facade = entity_facade()?;
    let entity_name = val(&cell).entity_name().to_string();
    let specs = facade
        .with_entity(&entity_name, |entity, _, _| {
            Ok(entity
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.is_pk))
                .collect::<Vec<_>>())
        })
        .map_err(lib)?;
    for (name, is_pk) in specs {
        let keep = match include {
            FieldInclude::All => true,
            FieldInclude::Pk => is_pk,
            FieldInclude::NonPk => !is_pk,
        };
        if !keep {
            continue;
        }
        let key = prefixed_field(&prefix, &name);
        let Some(host) = source.get(&key) else {
            continue;
        };
        let empty = host.is_null() || matches!(&host, Value::String(text) if text.is_empty());
        if !set_empty && empty {
            continue;
        }
        // With setIfEmpty an empty text is set as null (EntityJavaUtil.setFieldsEv), so a blank field clears.
        let host = if empty { Value::Null } else { host };
        val(&cell).set(name, host);
    }
    Ok(Ret::Undefined)
}

/// What `setFields` reads its fields from, as the QuickJS object's `in` and property read saw it: a map's entries, an
/// entity value's fields (a field its entity defines that it does not hold is there, as null), a list's indexes.
enum FieldSource {
    Entries(IndexMap<String, Value>),
    Entity(Arc<EntityCell>),
}

impl FieldSource {
    fn get(&self, key: &str) -> Option<Value> {
        match self {
            FieldSource::Entries(entries) => entries.get(key).cloned(),
            FieldSource::Entity(cell) => is_field(cell, key).then(|| field_value(cell, key).unwrap_or(Value::Null)),
        }
    }
}

/// The source of a `setFields` argument; `None` for a value that is not an object.
fn setfields_source(value: Option<&Value>) -> Option<FieldSource> {
    Some(match value? {
        Value::List(items) => FieldSource::Entries(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item.clone()))
                .collect(),
        ),
        Value::Map(entries) => FieldSource::Entries(entries.clone()),
        Value::Object(object) => match object.downcast::<EntityCell>() {
            Some(cell) => FieldSource::Entity(cell),
            None => FieldSource::Entries(IndexMap::new()),
        },
        _ => return None,
    })
}

/// `name` with `prefix` before it and its first letter upper case, as Java's `setFields` prefixes.
pub(crate) fn prefixed_field(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        return field.to_string();
    }
    let mut name = String::with_capacity(prefix.len() + field.len());
    name.push_str(prefix);
    let mut chars = field.chars();
    if let Some(first) = chars.next() {
        name.push(first.to_ascii_uppercase());
        name.push_str(chars.as_str());
    }
    name
}

fn value_create(call: &mut Call<'_>) -> Result<Ret, HostError> {
    write(call, |value, facade| value.create(facade))
}

/// `update()`: write the fields that changed since the value was read or last written, as Java's `update`, so a field
/// another transaction changed in between is left as it wrote it.
fn value_update(call: &mut Call<'_>) -> Result<Ret, HostError> {
    write(call, |value, facade| value.update(facade))
}

fn value_delete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    snapshot(&cell).delete(&facade).map_err(lib)?;
    Ok(Ret::Undefined)
}

fn value_create_or_update(call: &mut Call<'_>) -> Result<Ret, HostError> {
    write(call, |value, facade| value.create_or_update(facade))
}

/// `getPrimaryKeys()`: the primary key fields of the value, as a map.
fn value_primary_keys(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    let value = val(&cell);
    let names: Vec<String> = facade
        .with_entity(value.entity_name(), |entity, _, _| {
            Ok(entity.pk_field_names().iter().map(|name| (*name).to_string()).collect())
        })
        .map_err(lib)?;
    let map: IndexMap<String, Value> = names
        .into_iter()
        .map(|name| {
            let field = value.get(&name).cloned().unwrap_or(Value::Null);
            (name, field)
        })
        .collect();
    Ok(Ret::Value(Value::Map(map)))
}

fn value_sequenced_id_primary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    val(&cell).set_sequenced_id_primary(&facade).map_err(lib)?;
    Ok(Ret::Undefined)
}

fn value_sequenced_id_secondary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let facade = entity_facade()?;
    val(&cell).set_sequenced_id_secondary(&facade).map_err(lib)?;
    Ok(Ret::Undefined)
}

/// Whether a `forUpdate` argument is `true` itself.
fn locks(call: &Call<'_>, index: usize) -> bool {
    matches!(call.value(index), Some(Value::Bool(true)))
}

/// The lock of a related find: `forUpdate` at `index`, or `forNoKeyUpdate` (an MR Moqui addition) after it; both true
/// is refused, as XML actions refuse both attributes.
fn related_lock(call: &Call<'_>, index: usize, find: EntityFind) -> Result<EntityFind, HostError> {
    let (update, no_key) = (locks(call, index), locks(call, index + 1));
    if update && no_key {
        return Err(HostError::message(
            "a related find cannot be both forUpdate and forNoKeyUpdate",
        ));
    }
    Ok(find.for_update(update).for_no_key_update(no_key))
}

/// `findRelated(relationshipName, byAndFields, orderBy, useCache, forUpdate, forNoKeyUpdate)`.
fn value_find_related(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let name = call.text_or_empty(0);
    let mut find = related_lock(call, 4, related_find(&cell, &name).map_err(lib)?)?;
    if let Some(Value::Map(entries)) = call.value(1) {
        for (key, value) in entries {
            find = find.eq(key, value.clone());
        }
    }
    if let Some(Value::List(fields)) = call.value(2) {
        for field in fields {
            find = find.order_by_text(&moqui_lib::object::to_plain_string(field));
        }
    }
    Ok(Ret::Value(list_of(find.list().map_err(lib)?)))
}

/// `findRelatedOne(relationshipName, useCache, forUpdate, forNoKeyUpdate)`.
fn value_find_related_one(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let name = call.text_or_empty(0);
    let find = related_lock(call, 2, related_find(&cell, &name).map_err(lib)?)?;
    let found = find.one().map_err(lib)?;
    Ok(Ret::Value(found.map_or(Value::Null, value_of)))
}

/// `deleteWithRelated(relationships)`: delete the rows of the named `many` relationships (relationship names or short
/// aliases), then this value. When rows of any other entity still hold a foreign key to this value nothing is deleted
/// and the result is false, so a caller can report that the value is in use.
fn value_delete_with_related(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let wanted = texts_arg(call, 0);
    let facade = entity_facade()?;
    let parent = val(&cell).entity_name().to_string();
    // The relationships to delete through: the named ones that lead to many rows.
    let chosen: Vec<(String, String)> = facade
        .with_entity(&parent, |entity, _, _| {
            Ok(entity
                .relationships
                .iter()
                .filter(|relationship| {
                    relationship.relationship_type.starts_with("many")
                        && wanted.iter().any(|name| relationship.matches(name))
                })
                .map(|relationship| (relationship.name.clone(), relationship.related.clone()))
                .collect())
        })
        .map_err(lib)?;
    let skip: Vec<String> = chosen.iter().map(|(_, related)| related.clone()).collect();
    if !related_fk_rows(&cell, &skip)?.is_empty() {
        return Ok(Ret::from(false));
    }
    let finds = chosen
        .iter()
        .map(|(name, _)| related_find(&cell, name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(lib)?;
    // One transaction around the lot, so a failure part way leaves nothing deleted; it joins the caller's.
    let parent_value = snapshot(&cell);
    facade
        .transactions_for(&parent)
        .and_then(|tx| {
            tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
                for find in finds {
                    for value in find.list()?.values() {
                        value.delete(&facade)?;
                    }
                }
                parent_value.delete(&facade)
            })
        })
        .map_err(lib)?;
    Ok(Ret::from(true))
}

fn value_delete_related(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cell = this_cell(call)?;
    let name = call.text_or_empty(0);
    let facade = entity_facade()?;
    let list = related_find(&cell, &name).and_then(|find| find.list()).map_err(lib)?;
    for value in list.values() {
        value.delete(&facade).map_err(lib)?;
    }
    Ok(Ret::Undefined)
}
