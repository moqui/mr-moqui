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

//! `EntitySyncServices.xml`, but for `run#EntitySyncAll`, which stays XML actions.
//!
//! The Java pushed and pulled with `callJsonRpc`. Here the remote call is a REST POST of the same parameters as JSON
//! to the `moqui.rest.xml` path of the service (`/rest/s1/moqui/entity/syncs/data/get` and `.../put`) under
//! `targetServerUrl`, with basic auth of `targetUsername` and `targetPassword`.

use std::collections::{BTreeMap, BTreeSet};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::to_json;
use moqui_lib::object::{basic_convert, Value};

use super::{
    add_error, add_message, call_service, has_error, integer_of, map_text, millis_of, now_millis, pairs, param_text,
    stamp, take_errors, text_of, GET_DATA, INCLUDE_LIST, INTERNAL_RUN, PUT_DATA, RUN_SYNC,
};
use crate::entity::load_feed::{load_entity_xml_text, LoadOptions};
use crate::entity::{Condition, EntityFacade};

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, run_sync)?;
    rust_service!(map, internal_run)?;
    rust_service!(map, put_data)?;
    rust_service!(map, get_data)?;
    rust_service!(map, include_list)?;
    Ok(())
}

const ENTITY_SYNC: &str = "moqui.entity.sync.EntitySync";
const ARTIFACT_DETAIL: &str = "moqui.entity.sync.EntitySyncArtifactDetail";
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
const DEFAULT_DELAY_MILLIS: i64 = 300_000;
/// The most records one `get#EntitySyncData` reads, whatever `recordThreshold` asks: the answer is one string in memory
/// (plans/archive/Security-Review-Low.md SEC-DAT-08).
const MAX_RECORD_THRESHOLD: i64 = 10_000;

/// `run#EntitySync`.
pub(super) fn run_sync(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let start = now_millis();
    let sync_id = param_text(parameters, "entitySyncId");
    // Lock the EntitySync record, only one sync at a time.
    let Some(mut sync) = facade
        .find(ENTITY_SYNC)?
        .eq("entitySyncId", Value::string(&sync_id))
        .for_update(true)
        .one()?
    else {
        add_error(format!("EntitySync {sync_id} not found"));
        return Ok(BTreeMap::new());
    };
    // If already running quit, unless lastStartDate was more than 24 hours ago.
    if let Some(started) = millis_of(sync.get("lastStartDate"))
        .filter(|started| text_of(sync.get("statusId")) == "EsRunning" && *started > now_millis() - DAY_MILLIS)
    {
        let text = format!(
            "EntitySync {sync_id} not run: a run started at {} is still running",
            shown(started)
        );
        tracing::info!("{text}");
        add_message(text);
        return Ok(BTreeMap::new());
    }
    // Make sure a whole split is older than delayBufferMillis since lastSuccessfulSyncTime: a run before then would
    // read nothing and only write a History row, so it is skipped with a message saying when the next split is ready.
    let delay = delay_millis(sync.get("delayBufferMillis"));
    let split = split_millis(sync.get("syncSplitMillis"));
    let previous = millis_of(sync.get("lastSuccessfulSyncTime"));
    if let Some(last) = previous.filter(|last| !split_due(*last, split, delay, now_millis())) {
        let text = format!(
            "EntitySync {sync_id} not run: the next split, after {}, passes the delay buffer at {}",
            shown(last),
            shown(last.saturating_add(split).saturating_add(delay))
        );
        tracing::info!("{text}");
        add_message(text);
        return Ok(BTreeMap::new());
    }

    // Validations done, save that it is running and get started.
    sync.set("lastStartDate", stamp(start));
    sync.set("statusId", Value::string("EsRunning"));
    sync.update(facade)?;

    // The main work is in a separate transaction so it is independent of updating the status.
    let fields: moqui_lib::object::ValueMap = sync.fields().iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let results = call_service(
        INTERNAL_RUN,
        pairs([
            ("entitySync", Value::Map(fields)),
            ("entitySyncId", Value::string(&sync_id)),
        ]),
    );
    if has_error() {
        return Ok(BTreeMap::new());
    }

    let error_message = text_of(results.get("errorMessage"));
    if error_message.is_empty() {
        sync.set("statusId", Value::string("EsComplete"));
        sync.set(
            "lastSuccessfulSyncTime",
            results.get("inclusiveThruTime").cloned().unwrap_or(Value::Null),
        );
    } else {
        sync.set("statusId", Value::string("EsOtherError"));
    }
    sync.update(facade)?;

    let finish = now_millis();
    let status = text_of(sync.get("statusId"));
    let record = |name: &str| results.get(name).cloned().unwrap_or(Value::Null);
    call_service(
        "create#moqui.entity.sync.EntitySyncHistory",
        pairs([
            ("entitySyncId", Value::string(&sync_id)),
            ("statusId", Value::string(&status)),
            ("startDate", stamp(start)),
            ("finishDate", stamp(finish)),
            ("exclusiveFromTime", record("exclusiveFromTime")),
            ("inclusiveThruTime", record("inclusiveThruTime")),
            ("recordsStored", record("recordsStored")),
            ("runningTimeMillis", Value::I64(finish - start)),
            ("errorMessage", record("errorMessage")),
        ]),
    );
    if has_error() {
        return Ok(BTreeMap::new());
    }

    let last_success = millis_of(sync.get("lastSuccessfulSyncTime"));
    tracing::info!(
        "EntitySync [{sync_id}] finished: startDate={start}, lastSuccessfulSyncTime={last_success:?}, recordsStored={}, errorMessage={error_message}",
        text_of(results.get("recordsStored"))
    );

    // If another whole split is older than delayBufferMillis trigger another async run. Only after a success that
    // moved lastSuccessfulSyncTime: a failed run leaves it where it was, so following it again would repeat the failure
    // at once, without end, with a History row each time (plans/ui-review-system-ai-integrations.md AI-UI-002); the next
    // scheduled run tries again. Nor while the next split is still inside the buffer: that run would read nothing
    // (AI-UI-004).
    if error_message.is_empty() && follow_up(previous, last_success, split, delay, now_millis()) {
        let again = pairs([("entitySyncId", Value::string(&sync_id))]);
        if let Err(err) = crate::service::async_call(RUN_SYNC, &again) {
            add_error(err.message());
        }
    }
    Ok(BTreeMap::new())
}

/// `internalRun#EntitySync`, in its own transaction.
pub(super) fn internal_run(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let sync = parameters
        .get("entitySync")
        .and_then(crate::script::value_map)
        .ok_or_else(|| LibError::new("entitySync is not an EntitySync value"))?;
    let sync_id = map_text(&sync, "entitySyncId");

    let included = call_service(INCLUDE_LIST, pairs([("entitySyncId", Value::string(&sync_id))]));
    if has_error() {
        return Ok(BTreeMap::new());
    }
    let get_in = pairs([
        (
            "entityIncludeList",
            included.get("entityIncludeList").cloned().unwrap_or(Value::Null),
        ),
        ("lastSuccessfulSyncTime", field(&sync, "lastSuccessfulSyncTime")),
        ("syncSplitMillis", field(&sync, "syncSplitMillis")),
        ("recordThreshold", field(&sync, "recordThreshold")),
        // the delay run#EntitySync waited for, so the read and the wait agree (a stored 0 is the default, as there)
        (
            "delayBufferMillis",
            Value::I64(delay_millis(sync.get("delayBufferMillis"))),
        ),
    ]);

    let mut results = BTreeMap::new();
    if map_text(&sync, "forPull") == "Y" {
        // Get remotely, store locally.
        results.extend(remote_or_error(&sync, "get", get_in, facade.sim_active()));
        if truthy(results.get("recordCount")) {
            let put_in = pairs([("entityData", results.get("entityData").cloned().unwrap_or(Value::Null))]);
            results.extend(call_service(PUT_DATA, put_in));
        } else {
            // No records to store, so no local call.
            results.insert("recordsStored".to_string(), Value::I64(0));
        }
    } else {
        // Get locally, store remotely.
        let found = call_service(GET_DATA, get_in);
        if has_error() {
            return Ok(BTreeMap::new());
        }
        results.extend(found);
        if truthy(results.get("recordCount")) {
            let put_in = pairs([("entityData", results.get("entityData").cloned().unwrap_or(Value::Null))]);
            results.extend(remote_or_error(&sync, "put", put_in, facade.sim_active()));
        } else {
            // No records to store, so no remote call.
            results.insert("recordsStored".to_string(), Value::I64(0));
        }
    }

    let mut out = BTreeMap::new();
    if has_error() {
        out.insert("errorMessage".to_string(), Value::string(take_errors()));
    }
    for name in ["exclusiveFromTime", "inclusiveThruTime"] {
        if let Some(millis) = millis_of(results.get(name)) {
            out.insert(name.to_string(), stamp(millis));
        }
    }
    if let Some(stored) = integer_of(results.get("recordsStored")) {
        out.insert("recordsStored".to_string(), Value::I64(stored));
    }
    Ok(out)
}

/// `put#EntitySyncData`. The load restricts sensitive entities, as the Java `restrictSensitiveEntities(true)` load did,
/// and checks the resolved entity of every record, root or nested by relationship or short alias, where the Java
/// checked the root elements only; service elements are refused.
pub(super) fn put_data(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = param_text(parameters, "entityData");
    let timeout = integer_of(parameters.get("timeout"))
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(std::time::Duration::from_secs);
    let options = LoadOptions {
        dummy_fks: flag(parameters.get("dummyFks")),
        // `useTryInsert` is not read: a store is one upsert wherever that fits.
        timeout,
        // A sync partner is not an admin: the loader refuses identity-admin and secret-config records, root or
        // nested, and service elements.
        restrict_sensitive_entities: true,
        // ... and a UserAccount record that sets a password hash
        refuse_password_fields: true,
        // ... and a service job, which runs with authorization off
        refused_packages: vec!["moqui.service.job".to_string()],
        ..LoadOptions::default()
    };
    let loaded = load_entity_xml_text(facade, PUT_DATA, &text, &options)?;
    let stored = i64::try_from(loaded.records).unwrap_or(i64::MAX);
    Ok(BTreeMap::from([("recordsStored".to_string(), Value::I64(stored))]))
}

/// `get#EntitySyncData`. Each entity's records of one time split stream through an entity list iterator.
///
/// The Java looped over an unset `includeFilterList`, so the filters never applied; here they are each entry's
/// `includeFilterList`, ORed together, as its parameter description says.
pub(super) fn get_data(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let entries = match parameters.get("entityIncludeList") {
        Some(Value::List(items)) => items.iter().filter_map(crate::script::value_map).collect(),
        _ => Vec::new(),
    };
    let last_success = millis_of(parameters.get("lastSuccessfulSyncTime"));
    let split = split_millis(parameters.get("syncSplitMillis"));
    // A threshold under one would read no split and not move forward.
    let threshold = integer_of(parameters.get("recordThreshold"))
        .unwrap_or(1000)
        .clamp(1, MAX_RECORD_THRESHOLD);
    let delay = integer_of(parameters.get("delayBufferMillis")).unwrap_or(DEFAULT_DELAY_MILLIS);

    // Without a lastSuccessfulSyncTime this gets everything up to now less delayBufferMillis in one split, as the Java.
    let max_sync = now_millis() - delay;
    let starting = last_success.unwrap_or(0);
    let mut split_from = starting;
    let mut split_thru = if last_success.is_some() {
        split_from + split
    } else {
        max_sync
    };

    let mut xml = String::from("<entity-facade-xml>");
    let mut record_count = 0_i64;
    while record_count < threshold && split_thru <= max_sync {
        for entry in &entries {
            let entity_name = map_text(entry, "entityName");
            // a sync partner is not an admin: never the entities generic access does not read (login keys, sign-in
            // factors, secret configuration, the audit entities), and a replica holds no password field
            if crate::entity::sensitive::is_restricted_entity_or_view(facade, &entity_name) {
                tracing::error!("get#EntitySyncData: entity {entity_name} is not available to a sync partner, skipped");
                continue;
            }
            let mut find = facade
                .find(&entity_name)?
                .condition(Condition::greater("lastUpdatedStamp", stamp(split_from)))
                .condition(Condition::less_equal("lastUpdatedStamp", stamp(split_thru)));
            if let Some(filter) = include_filter(entry.get("includeFilterList")) {
                find = find.condition(filter);
            }
            let current = find.clone().count()?;
            record_count += current;
            if current > 0 {
                let levels = if flag(entry.get("dependents")) { 2 } else { 0 };
                write_records(facade, &entity_name, find, levels, &mut xml)?;
            }
        }
        split_from = split_thru;
        split_thru = split_from + split;
    }
    xml.push_str("</entity-facade-xml>");

    // The end of the last split read, or the start when none was: the Java returned the split after it (`splitThru`),
    // which the run stored as lastSuccessfulSyncTime, so the records of that next split were never read
    // (plans/ui-review-system-ai-integrations.md AI-UI-004).
    Ok(BTreeMap::from([
        ("entityData".to_string(), Value::string(xml)),
        ("recordCount".to_string(), Value::I64(record_count)),
        ("exclusiveFromTime".to_string(), stamp(starting)),
        ("inclusiveThruTime".to_string(), stamp(split_from)),
    ]))
}

/// `get#EntitySyncIncludeList`.
pub(super) fn include_list(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let sync_id = param_text(parameters, "entitySyncId");
    let details = facade
        .find(ARTIFACT_DETAIL)?
        .eq("artifactTypeEnumId", Value::string("AT_ENTITY"))
        .eq("entitySyncId", Value::string(&sync_id))
        .list()?;

    // Entity name to its filter maps (an empty list for no filter).
    let mut include: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut exclude: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut always: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut with_dependents = BTreeSet::new();
    // Read once, matched as needed for nameIsPattern.
    let mut all_entities: Option<Vec<String>> = None;
    for detail in details.iter() {
        let artifact_name = text_of(detail.get("artifactName"));
        let names: Vec<String> = if text_of(detail.get("nameIsPattern")) == "Y" {
            let all = all_entities.get_or_insert_with(|| non_view_entity_names(facade));
            let mut matched = Vec::new();
            for name in all.iter() {
                if moqui_lib::object::regex_full_match(name, &artifact_name)? {
                    matched.push(name.clone());
                }
            }
            matched
        } else {
            vec![artifact_name]
        };
        let target = match text_of(detail.get("applEnumId")).as_str() {
            "EsaaExclude" => &mut exclude,
            "EsaaAlways" => &mut always,
            _ => &mut include,
        };
        let filter_text = text_of(detail.get("filterMap"));
        for name in names {
            let filters = target.entry(name.clone()).or_default();
            if !filter_text.is_empty() {
                let filter = crate::script::eval_map(&filter_text)?;
                filters.push(Value::Map(filter.into_iter().collect()));
            }
            if text_of(detail.get("dependents")) == "Y" {
                with_dependents.insert(name);
            }
        }
    }

    // Remove excluded entities, then add the always entries.
    for name in exclude.keys() {
        include.remove(name);
    }
    for (name, filters) in always {
        include.entry(name).or_default().extend(filters);
    }

    let list = include
        .into_iter()
        .map(|(name, filters)| {
            let dependents = if with_dependents.contains(&name) { "Y" } else { "N" };
            let mut entry = IndexMap::default();
            entry.insert("entityName".to_string(), Value::string(&name));
            entry.insert("includeFilterList".to_string(), Value::List(filters));
            entry.insert("dependents".to_string(), Value::string(dependents));
            Value::Map(entry)
        })
        .collect();
    Ok(BTreeMap::from([("entityIncludeList".to_string(), Value::List(list))]))
}

/// Append each record of `find` as its XML element, written from the open cursor as Java's sync iterates; a record's
/// dependents are read beside it. On a database that runs no other query while a cursor is open (MySQL,
/// [`EntityFacade::streams_beside_queries`]) records with dependents are read first.
fn write_records(
    facade: &EntityFacade,
    entity_name: &str,
    find: crate::entity::EntityFind,
    levels: i64,
    xml: &mut String,
) -> Result<(), LibError> {
    let beside = levels == 0 || facade.streams_beside_queries(entity_name);
    let mut cursor = find.iterator()?;
    let mut held = Vec::new();
    while let Some(value) = cursor.next_value()? {
        if beside {
            xml.push_str(&crate::entity::query_rest::guarded_xml_element(
                facade,
                entity_name,
                value.fields(),
                levels,
                crate::entity::query_rest::RecordGuard::Replica,
            )?);
        } else {
            held.push(value);
        }
    }
    cursor.close()?;
    for value in held {
        xml.push_str(&crate::entity::query_rest::guarded_xml_element(
            facade,
            entity_name,
            value.fields(),
            levels,
            crate::entity::query_rest::RecordGuard::Replica,
        )?);
    }
    Ok(())
}

/// The filter maps of one include entry ORed together, each map being equals conditions ANDed, as
/// `makeCondition(Map)` makes them. `None` when there are no filters.
fn include_filter(filters: Option<&Value>) -> Option<Condition> {
    let Some(Value::List(filters)) = filters else {
        return None;
    };
    let conditions: Vec<Condition> = filters
        .iter()
        .filter_map(crate::script::value_map)
        .map(|filter| {
            Condition::and(
                filter
                    .into_iter()
                    .map(|(field, value)| Condition::equals(field.as_str(), value))
                    .collect(),
            )
        })
        .collect();
    (!conditions.is_empty()).then(|| Condition::or(conditions))
}

fn non_view_entity_names(facade: &EntityFacade) -> Vec<String> {
    facade.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|entity| !entity.is_view)
            .map(|entity| entity.full_name.clone())
            .collect()
    })
}

/// A remote call whose failure is a message error, as `callJsonRpc` reported one. The result is empty then.
fn remote_or_error(
    sync: &moqui_lib::object::ValueMap,
    operation: &str,
    parameters: BTreeMap<String, Value>,
    sim: bool,
) -> BTreeMap<String, Value> {
    match call_remote(sync, operation, &parameters, sim) {
        Ok(results) => results,
        Err(err) => {
            add_error(err.message());
            BTreeMap::new()
        }
    }
}

fn call_remote(
    sync: &moqui_lib::object::ValueMap,
    operation: &str,
    parameters: &BTreeMap<String, Value>,
    sim: bool,
) -> Result<BTreeMap<String, Value>, LibError> {
    let base = map_text(sync, "targetServerUrl");
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return Err(LibError::new(format!(
            "EntitySync {} has no targetServerUrl",
            map_text(sync, "entitySyncId")
        )));
    }
    let url = format!("{base}/rest/s1/moqui/entity/syncs/data/{operation}");
    let body = to_json(&Value::Map(
        parameters
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    ));
    let shown = crate::outbound::redacted(&url);
    if sim {
        return Err(LibError::new(format!(
            "HTTP POST {shown} denied because sim mode is active"
        )));
    }
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    let username = map_text(sync, "targetUsername");
    if !username.is_empty() {
        let credentials = format!("{username}:{}", map_text(sync, "targetPassword"));
        headers.push((
            "Authorization".to_string(),
            format!("Basic {}", STANDARD.encode(credentials)),
        ));
    }
    let response = crate::outbound::send(crate::outbound::Request {
        method: crate::outbound::Method::Post,
        url,
        headers,
        body: Some(body.to_string()),
        timeout: None,
        // the row's address: a push to another server on the private network needs its allowPrivateNetwork Y on a
        // production instance
        deny_internal: crate::outbound::deny_private_network(&map_text(sync, "allowPrivateNetwork")),
    })?;
    let status = response.status;
    if (300..400).contains(&status) {
        // The shared client follows a redirect only to the same origin; another one (most often http to https) is the
        // partner's address to correct, so say where it points.
        let location = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("location"))
            .map(|(_, value)| {
                if value.contains("://") {
                    crate::outbound::redacted(value)
                } else {
                    value.clone()
                }
            })
            .unwrap_or_default();
        return Err(LibError::new(format!(
            "Remote EntitySync {operation} at {shown} was redirected ({status}) to {location}: set the target server URL to the address the partner serves"
        )));
    }
    let reply = crate::service::parse_json(&response.body).ok();
    if !(200..300).contains(&status) {
        let errors = match &reply {
            Some(Value::Map(map)) => text_of(map.get("errors")),
            _ => String::new(),
        };
        return Err(LibError::new(format!(
            "Remote EntitySync {operation} at {shown} failed ({status}): {errors}"
        )));
    }
    match reply {
        Some(Value::Map(map)) => Ok(map.into_iter().collect()),
        _ => Ok(BTreeMap::new()),
    }
}

/// A field of the EntitySync map, null when it is missing, so the remote service applies its default.
fn field(sync: &moqui_lib::object::ValueMap, name: &str) -> Value {
    sync.get(name).cloned().unwrap_or(Value::Null)
}

/// `syncSplitMillis ?: 1000`; a split under one millisecond would not move forward.
fn split_millis(value: Option<&Value>) -> i64 {
    integer_of(value).unwrap_or(1000).max(1)
}

/// Whether the split after `last` is whole and older than `delay` at `now`, so a run would read it: what
/// `get#EntitySyncData` reads, and so what a run and its follow-up wait for.
pub(super) fn split_due(last: i64, split: i64, delay: i64, now: i64) -> bool {
    last.saturating_add(split) <= now.saturating_sub(delay)
}

/// Whether a run queues another: only after it moved lastSuccessfulSyncTime from `previous` to `last`, and while the
/// split after `last` is due. A run that did not move it (a failure, or a pull from a remote that answered the same end)
/// would only repeat itself.
pub(super) fn follow_up(previous: Option<i64>, last: Option<i64>, split: i64, delay: i64, now: i64) -> bool {
    last.is_some_and(|last| previous.is_none_or(|before| last > before) && split_due(last, split, delay, now))
}

/// A time in a message: RFC 3339 in UTC.
fn shown(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis).map_or_else(|_| millis.to_string(), |at| at.to_string())
}

/// `delayBufferMillis ?: 300000`: null or zero is the default.
fn delay_millis(value: Option<&Value>) -> i64 {
    match integer_of(value) {
        Some(millis) if millis != 0 => millis,
        _ => DEFAULT_DELAY_MILLIS,
    }
}

/// Groovy truth for a count: null and zero are false.
fn truthy(value: Option<&Value>) -> bool {
    integer_of(value).is_some_and(|number| number != 0)
}

/// A Boolean parameter, or an indicator: true, Y, and their text forms.
fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(other) => {
            matches!(basic_convert(other, "Boolean").value, Value::Bool(true)) || text_of(Some(other)) == "Y"
        }
        None => false,
    }
}
