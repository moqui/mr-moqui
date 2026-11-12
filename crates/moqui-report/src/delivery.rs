//! Owner-equivalent delivery of business reports (BOSS impl-insights P7, INS-06; design-insights §7.1, §7.3;
//! docs/tools/Monitoring.md "Report delivery").
//!
//! A schedule renders as its owner. That result may go to another user only when it is what that user would see: when
//! their **scope fingerprint** for the definition equals the owner's. The fingerprint ([`scope`]) is a hash of what the
//! user reads of the definition and everything it references, the documents of each dataset they may read, each
//! dataset's resolved row filter (`rowFilter`, `contextFilters` and the `rowFilterService`'s answer) and, for a dataset
//! whose records are restricted, whether they hold them. It is computed in a context of its own signed in as that user,
//! so nothing of the caller's request (a screen's inheritable grant, a disabled authorization) enters it. A service
//! source decides its scope in its own code, which a fingerprint cannot see: a definition over one is never comparable,
//! so it is delivered to its owner alone.
//!
//! The check runs when a schedule is saved ([`store_schedule`], `check#ReportAudience`), when it is resumed, and again at
//! each run for each recipient just before their message ([`send_each`]): a recipient whose scope changed is skipped
//! with `scope-differs` and the others are sent. Each recipient gets a message of their own, no two on one address
//! line, and an outcome row of their own ([`DELIVERY`]) under the run's [`OCCURRENCE`]. One render serves every recipient
//! of a locale, since they share the definition's version and the fingerprint.
//!
//! Which schedules are held to this: one saved by `store#Schedule` (its `deliveryMode` personal or group), and any other
//! report schedule whose definition reads a business dataset, whatever wrote its row, so a schedule made by
//! `create#Schedule` or by an entity write cannot send a business result unchecked. A technical schedule (telemetry and
//! other datasets that are not business ones) is delivered as before, in owner mode.
//!
//! No row keeps a report. An occurrence and its deliveries are evidence: who, when, which revision and version, what
//! state and which safe reason code. A recipient reads their report again through [`get_recipient_report`], rendered as
//! themselves, and only while their fingerprint is still the one the delivery was sent under.
use crate::schedule::{self, Rendered};
use crate::{definitions, fail};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::search::Query;
use moqui_core::transaction::TransactionAttribute;
use moqui_core::{
    context::ExecutionContext,
    native::{NativeRegistry, Parameters, ServiceInfo},
};
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const SCHEDULE: &str = "moqui.screen.ScreenScheduled";
const LOCK: &str = "moqui.screen.ScreenScheduledLock";
const RECIPIENT: &str = "moqui.report.ReportScheduleRecipient";
pub(crate) const OCCURRENCE: &str = "moqui.report.ReportOccurrence";
pub(crate) const DELIVERY: &str = "moqui.report.ReportRecipientDelivery";
const ACCOUNT: &str = "moqui.security.UserAccount";
const MEMBER: &str = "moqui.security.UserGroupMember";
pub(crate) const DEFINITION: &str = "moqui.report.Definition";
const REFERENCE: &str = "moqui.report.DefinitionReference";
/// The artifact a user needs to deliver to anyone but themselves.
pub(crate) const GROUP_ARTIFACT: &str = "org.moqui.report.delivery.group";
const STORE_SERVICE: &str = "org.moqui.report.ReportServices.store#Schedule";
const OWNER_PERMISSION: &str = "REPORT_OWNER_CHANGE";
pub(crate) const ADMIN_PERMISSION: &str = "ADMIN_TRANSFER_VIEW";
const CEILING_KEY: &str = "report.groupRecipientCeiling";
const DEFAULT_CAP: i64 = 50;
const DEFAULT_CEILING: i64 = 200;
const DEFAULT_TEMPLATE: &str = "DASHBOARD_REPORT";

pub(crate) const PENDING: &str = "RptDlvPending";
const SENDING: &str = "RptDlvSending";
pub(crate) const SENT: &str = "RptDlvSent";
pub(crate) const SKIPPED: &str = "RptDlvSkipped";
pub(crate) const FAILED: &str = "RptDlvFailed";
const UNCERTAIN: &str = "RptDlvUncertain";
const RUNNING: &str = "RptOccRunning";

pub(crate) fn text<'a>(p: &'a Parameters, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("")
}
pub(crate) fn field<'a>(row: &'a EntityValue, name: &str) -> &'a str {
    row.get(name).and_then(Value::as_str).unwrap_or("")
}
pub(crate) fn number(value: Option<&Value>) -> Option<i64> {
    value
        .filter(|value| !value.is_null())
        .and_then(|value| moqui_lib::object::to_plain_string(value).parse::<i64>().ok())
}
pub(crate) fn millis(row: &EntityValue, name: &str) -> Json {
    row.get(name)
        .and_then(moqui_core::user::value_millis)
        .map_or(Json::Null, Json::from)
}
/// A list parameter: a list of ids, or one comma-separated string of them as a form posts it.
fn ids(p: &Parameters, k: &str) -> Vec<String> {
    let mut out: Vec<String> = match p.get(k) {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(value) if !value.is_null() => moqui_lib::object::to_plain_string(value)
            .split(',')
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    for id in &mut out {
        *id = id.trim().to_owned();
    }
    out.retain(|id| !id.is_empty());
    out.sort();
    out.dedup();
    out
}
pub(crate) fn flag(p: &Parameters, k: &str) -> Option<bool> {
    p.get(k)
        .filter(|value| !value.is_null())
        .map(|value| matches!(value, Value::Bool(true)) || matches!(value.as_str(), Some("true" | "Y")))
}
pub(crate) fn viewer() -> Result<String, Error> {
    moqui_core::user::current_user_id()?
        .filter(|user| !user.is_empty())
        .ok_or_else(|| fail("report delivery requires an authenticated viewer"))
}
pub(crate) fn entity(ec: &ExecutionContext) -> Result<EntityFacade, Error> {
    ec.factory().entity().ok_or_else(|| fail("entity facade unavailable"))
}
pub(crate) fn out(value: Json) -> Parameters {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => Parameters::new(),
    }
}
/// A write of delivery evidence in a transaction of its own, with authorization off: the rows are the engine's, and a
/// run's own transaction is suspended while it sends.
pub(crate) fn write<T>(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let tx = facade.transactions_for(DELIVERY)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let _authz = ec.disable_authz();
        body()
    })
}

// ===== accounts and recipients =====

#[derive(Clone, Debug)]
pub(crate) struct Account {
    pub user_id: String,
    pub username: String,
    pub name: String,
    pub address: Option<String>,
    pub locale: Option<String>,
    pub active: bool,
}

pub(crate) fn account(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    user_id: &str,
    now: i64,
) -> Result<Option<Account>, Error> {
    if user_id.is_empty() {
        return Ok(None);
    }
    let _authz = ec.disable_authz();
    let Some(row) = facade
        .find(ACCOUNT)?
        .use_cache(false)
        .eq("userId", Value::string(user_id))
        .one()?
    else {
        return Ok(None);
    };
    let active = field(&row, "disabled") != "Y"
        && !row
            .get("terminateDate")
            .and_then(moqui_core::user::value_millis)
            .is_some_and(|date| date <= now);
    let some = |name: &str| Some(field(&row, name).to_owned()).filter(|value| !value.is_empty());
    Ok(Some(Account {
        user_id: user_id.to_owned(),
        username: field(&row, "username").to_owned(),
        name: some("userFullName").unwrap_or_else(|| field(&row, "username").to_owned()),
        address: some("emailAddress"),
        locale: some("locale"),
        active,
    }))
}

#[derive(Clone, Debug)]
pub(crate) struct Recipient {
    pub account: Account,
    /// The groups that brought this user in, empty for one named alone.
    pub via: Vec<String>,
    /// Named as a user, not only a member of a named group.
    pub explicit: bool,
}

/// The named users and the members in date of the named groups, one entry a user, in user id order. An id that names
/// no account is left out and logged.
pub(crate) fn expand(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    users: &[String],
    groups: &[String],
    now: i64,
) -> Result<Vec<Recipient>, Error> {
    let mut found: BTreeMap<String, Recipient> = BTreeMap::new();
    for user in users {
        match account(ec, facade, user, now)? {
            Some(account) => {
                found.insert(
                    user.clone(),
                    Recipient {
                        account,
                        via: Vec::new(),
                        explicit: true,
                    },
                );
            }
            None => tracing::error!(user = %user, "report recipient is not a user account; left out"),
        }
    }
    for group in groups {
        let members = {
            let _authz = ec.disable_authz();
            facade
                .find(MEMBER)?
                .use_cache(false)
                .eq("userGroupId", Value::string(group))
                .list()?
        };
        for member in members.iter() {
            let open = |name: &str, before: bool| {
                member
                    .get(name)
                    .and_then(moqui_core::user::value_millis)
                    .is_some_and(|date| if before { date > now } else { date <= now })
            };
            if open("fromDate", true) || open("thruDate", false) {
                continue;
            }
            let user = field(member, "userId").to_owned();
            if let Some(entry) = found.get_mut(&user) {
                entry.via.push(group.clone());
                continue;
            }
            if let Some(account) = account(ec, facade, &user, now)? {
                found.insert(
                    user,
                    Recipient {
                        account,
                        via: vec![group.clone()],
                        explicit: false,
                    },
                );
            }
        }
    }
    Ok(found.into_values().collect())
}

/// The recipients a schedule row names: its ReportScheduleRecipient rows, and the userId and userGroupId an owner-mode
/// row carries; the owner alone when it names nobody.
fn schedule_recipients(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    schedule: &EntityValue,
) -> Result<(Vec<String>, Vec<String>), Error> {
    let id = field(schedule, "screenScheduledId");
    let rows = {
        let _authz = ec.disable_authz();
        facade
            .find(RECIPIENT)?
            .use_cache(false)
            .eq("screenScheduledId", Value::string(id))
            .list()?
    };
    let mut users = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for row in rows.iter() {
        let target = if field(row, "recipientKind") == "group" {
            &mut groups
        } else {
            &mut users
        };
        target.insert(field(row, "recipientId").to_owned());
    }
    if field(schedule, "deliveryMode") != "group" {
        for (name, target) in [("userId", &mut users), ("userGroupId", &mut groups)] {
            if !field(schedule, name).is_empty() {
                target.insert(field(schedule, name).to_owned());
            }
        }
    }
    if users.is_empty() && groups.is_empty() {
        users.insert(field(schedule, "reportOwnerId").to_owned());
    }
    Ok((users.into_iter().collect(), groups.into_iter().collect()))
}

// ===== the scope fingerprint =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub hash: String,
    /// The part of the fingerprint that is the definitions read: where it differs the recipient cannot open the report
    /// itself, which its owner mends by sharing it.
    pub definitions: String,
    /// False when the definition reads a service source, whose scope is its own code's.
    pub comparable: bool,
    /// True when the user may not read the definition, one it references or one of its datasets.
    pub denied: bool,
}

/// A query in a form that does not depend on the order a policy listed its values in.
fn canonical(query: &Query) -> String {
    match query {
        Query::Bool {
            all,
            any,
            none,
            min_any,
        } => {
            let part = |queries: &[Query]| {
                let mut parts: Vec<String> = queries.iter().map(canonical).collect();
                parts.sort();
                parts.dedup();
                parts.join(",")
            };
            format!(
                "bool(all[{}]any[{}]none[{}]min{min_any:?})",
                part(all),
                part(any),
                part(none)
            )
        }
        Query::Field(field) => {
            let mut values: Vec<String> = field.values.iter().map(moqui_lib::object::to_plain_string).collect();
            values.sort();
            values.dedup();
            let mut rest = (**field).clone();
            rest.values = Vec::new();
            format!("field({rest:?}in[{}])", values.join("\u{1f}"))
        }
        Query::Child { path, query } => format!("child({path},{})", canonical(query)),
        other => format!("{other:?}"),
    }
}

/// The definitions one walk follows, and the name it marks unread when there are more.
const WALK_LIMIT: usize = 500;
const BEYOND: &str = "(more definitions than are followed)";
/// The dataset a query without a `datasetId` reads (lib.rs `execute_query_inner`): the telemetry spans for a document,
/// service map or flame query, the telemetry metrics otherwise. Named here so a fingerprint counts it.
fn default_dataset(query: &Json) -> &'static str {
    match query["mode"].as_str() {
        Some("documents" | "serviceMap" | "flame") => "otel_spans",
        _ => "otel_metrics",
    }
}

/// The definitions a definition reaches by reference and the datasets its queries name, a query that names none
/// counted by the dataset it reads by default. `read` answers a definition, or None for one that is missing or not the
/// reader's.
fn walk(
    id: &str,
    read: &dyn Fn(&str) -> Option<Json>,
    seen: &mut BTreeSet<String>,
    datasets: &mut BTreeSet<String>,
    unread: &mut BTreeSet<String>,
) {
    if seen.contains(id) {
        return;
    }
    if seen.len() >= WALK_LIMIT {
        // more definitions than are followed: what is past the bound is not known to be read, so nobody is equivalent
        unread.insert(BEYOND.to_owned());
        return;
    }
    seen.insert(id.to_owned());
    let Some(definition) = read(id) else {
        unread.insert(id.to_owned());
        return;
    };
    if definition["type"] == "dataset" {
        datasets.insert(id.to_owned());
        return;
    }
    if definition["type"] == "alert" {
        rule_defaults(&definition["body"]["query"], datasets);
    }
    collect(&definition["body"], read, seen, datasets, unread);
}
/// The dataset an alert rule's query reads when it names none (alert.rs `approved_labels`: the telemetry metrics), for each
/// leaf of a compound query.
fn rule_defaults(query: &Json, datasets: &mut BTreeSet<String>) {
    if let Some(queries) = query["queries"].as_array() {
        for query in queries {
            rule_defaults(query, datasets);
        }
    } else if query.is_object() && !query["datasetId"].is_string() {
        datasets.insert("otel_metrics".to_owned());
    }
}
fn collect(
    value: &Json,
    read: &dyn Fn(&str) -> Option<Json>,
    seen: &mut BTreeSet<String>,
    datasets: &mut BTreeSet<String>,
    unread: &mut BTreeSet<String>,
) {
    match value {
        Json::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value.as_str()) {
                    ("datasetId", Some(id)) => {
                        datasets.insert(id.to_owned());
                    }
                    ("panelRef" | "dashboardRef" | "definitionId", Some(id)) => walk(id, read, seen, datasets, unread),
                    _ => {
                        // a leaf query, or a query variable, that names no dataset reads the engine's default one
                        let defaulted = |item: &&Json| item.is_object() && !item["datasetId"].is_string();
                        for item in value.as_array().into_iter().flatten().filter(defaulted) {
                            if key == "queries" && !item["queries"].is_array() {
                                datasets.insert(default_dataset(item).to_owned());
                            } else if key == "variables" && item["kind"] == "query" {
                                datasets.insert("otel_metrics".to_owned());
                            }
                        }
                        collect(value, read, seen, datasets, unread);
                    }
                }
            }
        }
        Json::Array(items) => {
            for item in items {
                collect(item, read, seen, datasets, unread);
            }
        }
        _ => {}
    }
}

/// The scope fingerprint of the entered viewer for a definition. Call it in a context signed in as that user
/// ([`scope_as`]), so only their own grants decide.
pub(crate) fn scope(ec: &ExecutionContext, definition_id: &str) -> Scope {
    let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    walk(
        definition_id,
        &|id| definitions::read(ec, id).ok(),
        &mut seen,
        &mut datasets,
        &mut unread,
    );
    let mut denied = !unread.is_empty();
    let mut comparable = true;
    let mut parts: Vec<String> = seen
        .iter()
        .map(|id| {
            format!(
                "definition {id} {}",
                if unread.contains(id) { "unread" } else { "read" }
            )
        })
        .collect();
    for id in &datasets {
        match crate::resolve_dataset(ec, id, true) {
            Ok(dataset) => {
                if dataset.service.is_some() {
                    comparable = false;
                }
                let mut documents = match &dataset.target {
                    moqui_core::search::Target::Documents(documents) => documents.clone(),
                    moqui_core::search::Target::Index(index) => vec![format!("index:{index}")],
                };
                documents.sort();
                let records = crate::semantics::check_rows(ec, &dataset).is_ok();
                parts.push(format!(
                    "dataset {id} documents[{}] service[{}] records[{records}] filter[{}]",
                    documents.join(","),
                    dataset.service.as_deref().unwrap_or(""),
                    canonical(&dataset.filter)
                ));
            }
            Err(_) => {
                denied = true;
                parts.push(format!("dataset {id} unread"));
            }
        }
    }
    // What a dashboard reads though its definition names it nowhere (lib.rs `dashboard_frames_filtered`): the change
    // markers, and over a business dataset each registered source of business events. Each is read as the viewer and
    // adds nothing for one who may not read it, so whether and how this user reads it is part of what they see; it is
    // not a source the report needs, so one they may not read does not make the report denied.
    let mut implicit = vec!["monitor_annotations".to_owned()];
    if datasets.iter().any(|id| {
        definitions::read_unchecked(ec, id)
            .ok()
            .flatten()
            .is_some_and(|definition| crate::business::is_business(&definition["body"]))
    }) {
        implicit.extend(crate::annotations::source_datasets(ec));
    }
    implicit.sort();
    implicit.dedup();
    for id in implicit.iter().filter(|id| !datasets.contains(*id)) {
        parts.push(match crate::resolve_dataset(ec, id, true) {
            Ok(dataset) => {
                let mut documents = match &dataset.target {
                    moqui_core::search::Target::Documents(documents) => documents.clone(),
                    moqui_core::search::Target::Index(index) => vec![format!("index:{index}")],
                };
                documents.sort();
                format!(
                    "markers {id} documents[{}] filter[{}]",
                    documents.join(","),
                    canonical(&dataset.filter)
                )
            }
            Err(_) => format!("markers {id} unread"),
        });
    }
    let mut hash = Sha256::new();
    for part in &parts {
        hash.update(part.as_bytes());
        hash.update([0x1e]);
    }
    Scope {
        definitions: seen
            .iter()
            .map(|id| format!("{id}{}", if unread.contains(id) { "-" } else { "+" }))
            .collect::<Vec<_>>()
            .join(" "),
        hash: hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect(),
        comparable,
        denied,
    }
}

/// [`scope`] in a context of its own, signed in as `username`. An account that cannot sign in (disabled, terminated, a
/// password change pending) is an error.
pub(crate) fn scope_as(ec: &ExecutionContext, username: &str, definition_id: &str) -> Result<Scope, Error> {
    let context = ec.factory().new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username)?;
    Ok(scope(&context, definition_id))
}

/// Whether `username` holds an artifact by their own grants, asked in a context of their own.
pub(crate) fn holds_as(ec: &ExecutionContext, username: &str, artifact: &str, action: &str) -> bool {
    let context = ec.factory().new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).is_ok()
        && moqui_core::user::authorize_own("AT_SERVICE", artifact, action).is_ok()
}

/// A dataset whose rows depend on who reads it: a business dataset, and any dataset scoped by a `rowFilterService` or by
/// `contextFilters` (the viewer's id, groups or account), whatever its signal, such as a business events dataset over
/// the annotations. Its owner's result is not another user's to receive unchecked.
fn viewer_scoped(body: &Json) -> bool {
    crate::business::is_business(body)
        || body["rowFilterService"].is_string()
        || body["contextFilters"]
            .as_array()
            .is_some_and(|filters| !filters.is_empty())
}

/// Whether a report schedule's definition reads a dataset whose rows depend on the viewer ([`viewer_scoped`]), read
/// whoever asks: the engine's own question, whose answer no caller sees.
pub(crate) fn reads_business(ec: &ExecutionContext, definition_id: &str) -> bool {
    let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let read = |id: &str| definitions::read_unchecked(ec, id).ok().flatten();
    walk(definition_id, &read, &mut seen, &mut datasets, &mut unread);
    datasets
        .iter()
        .any(|id| read(id).is_some_and(|definition| viewer_scoped(&definition["body"])))
}

/// [`reads_business`] for an alert rule's body as it is about to be saved, or as its stored row holds it (alert_delivery.rs).
pub(crate) fn body_reads_business(ec: &ExecutionContext, body: &Json) -> bool {
    let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let read = |id: &str| definitions::read_unchecked(ec, id).ok().flatten();
    collect(body, &read, &mut seen, &mut datasets, &mut unread);
    datasets
        .iter()
        .any(|id| read(id).is_some_and(|definition| viewer_scoped(&definition["body"])))
}

/// A schedule held to owner equivalence: one `store#Schedule` saved, or any whose definition reads a business dataset or
/// another whose rows depend on the viewer.
pub(crate) fn business(ec: &ExecutionContext, schedule: &EntityValue) -> bool {
    !field(schedule, "deliveryMode").is_empty() || reads_business(ec, field(schedule, "reportDefinitionId"))
}

/// The reason a recipient may not be sent the owner's result, or "" when they may. A code, never the other user's
/// grants.
pub(crate) fn judge(
    ec: &ExecutionContext,
    definition_id: &str,
    owner: &Account,
    owner_scope: &Scope,
    recipient: &Account,
) -> (&'static str, String) {
    if owner_scope.denied {
        return ("denied-source", String::new());
    }
    if !recipient.active {
        return ("inactive", String::new());
    }
    if recipient.user_id == owner.user_id {
        let reason = if recipient.address.is_none() { "no-address" } else { "" };
        return (reason, owner_scope.hash.clone());
    }
    if moqui_core::user::is_external_user(&recipient.user_id) {
        return ("external", String::new());
    }
    if !owner_scope.comparable {
        return ("not-comparable", String::new());
    }
    match scope_as(ec, &recipient.username, definition_id) {
        Err(_) => ("inactive", String::new()),
        // the report itself not shared with them, which the owner mends, before the data they read, which they cannot
        Ok(scope) if scope.definitions != owner_scope.definitions => ("not-shared", String::new()),
        Ok(scope) if scope != *owner_scope => ("scope-differs", String::new()),
        Ok(scope) => (if recipient.address.is_none() { "no-address" } else { "" }, scope.hash),
    }
}

pub(crate) fn reason_text(code: &str) -> &'static str {
    match code {
        "" => "",
        "scope-differs" => "Their access to this report's data differs from the owner's",
        "not-shared" => "The report, or a dashboard or panel it uses, is not shared with them",
        "not-comparable" => "This report reads a source whose scope cannot be compared, so it goes to its owner alone",
        "denied-source" => "The owner may not read this report or one of its sources",
        "inactive" => "The account is not active",
        "external" => "The account is not an internal user",
        "no-address" => "The account has no email address",
        "over-cap" => "The audience is larger than the recipient cap",
        "owner-unauthorized" => "The owner may not deliver to other users",
        "removed-audience" => "No longer a recipient of the schedule",
        "schedule-changed" => "The schedule changed before this delivery",
        "delivery-blocked" => "The run was stopped before this delivery",
        "source-failure" => "The report could not be rendered",
        "transport-failure" => "The message was not accepted for delivery",
        "uncertain" => "Whether the message was sent is not known",
        _ => "Not delivered",
    }
}

/// The recipient cap asked for (50 without one) held under the instance ceiling, and the ceiling.
pub(crate) fn caps(requested: Option<i64>) -> (i64, i64) {
    let ceiling = moqui_core::service::call::sync_checked(
        "org.moqui.admin.ConfigurationServices.get#InstancePreference",
        &Parameters::from([("settingKey".into(), Value::string(CEILING_KEY))]),
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ignore_previous_error: true,
            no_remember_parameters: true,
            ..Default::default()
        },
    )
    .ok()
    .and_then(|result| number(result.get("numberValue")))
    .filter(|ceiling| *ceiling > 0)
    .unwrap_or_else(|| {
        tracing::error!(
            key = CEILING_KEY,
            "the group recipient ceiling could not be read; using {DEFAULT_CEILING}"
        );
        DEFAULT_CEILING
    });
    (requested.unwrap_or(DEFAULT_CAP).clamp(1, ceiling), ceiling)
}

pub(crate) struct Verdict {
    pub recipient: Recipient,
    pub reason: &'static str,
}
impl Verdict {
    /// What refuses a save: a scope that differs or cannot be compared, and a named user who cannot be sent to. A group's
    /// member without an address or an active account is skipped at each run instead, as members come and go.
    pub(crate) fn blocking(&self) -> bool {
        match self.reason {
            "" => false,
            "inactive" | "no-address" => self.recipient.explicit,
            _ => true,
        }
    }
    pub(crate) fn json(&self) -> Json {
        json!({"userId": self.recipient.account.user_id, "username": self.recipient.account.username,
            "userFullName": self.recipient.account.name, "viaGroupIds": self.recipient.via,
            "eligible": self.reason.is_empty(), "blocking": self.blocking(), "reasonCode": self.reason,
            "reason": reason_text(self.reason)})
    }
}
pub(crate) struct Audience {
    pub verdicts: Vec<Verdict>,
    pub users: Vec<String>,
    pub groups: Vec<String>,
    pub mode: &'static str,
    pub cap: i64,
    pub ceiling: i64,
}

/// A definition a schedule may name: a dashboard or a report the viewer reads.
fn schedulable(ec: &ExecutionContext, definition_id: &str) -> Result<Json, Error> {
    let definition = definitions::read(ec, definition_id)?;
    if !matches!(definition["type"].as_str(), Some("report" | "dashboard")) {
        return Err(fail("a schedule requires a dashboard or report definition"));
    }
    Ok(definition)
}

/// The audience of a definition for an owner: each recipient's verdict, the mode the recipients make, and the cap.
/// Recipients other than the owner need the caller to hold the group-delivery artifact.
pub(crate) fn audience(
    ec: &ExecutionContext,
    owner: &Account,
    definition_id: &str,
    users: &[String],
    groups: &[String],
    cap: Option<i64>,
) -> Result<Audience, Error> {
    let facade = &entity(ec)?;
    let now = moqui_core::user::now_millis();
    let mut users = users.to_vec();
    if users.is_empty() && groups.is_empty() {
        users.push(owner.user_id.clone());
    }
    let personal = groups.is_empty() && users == [owner.user_id.clone()];
    if !personal {
        moqui_core::user::authorize_own("AT_SERVICE", GROUP_ARTIFACT, "create").map_err(|_| {
            fail("GROUP_DELIVERY_DENIED: delivering a report to other users needs the group delivery grant")
        })?;
    }
    let owner_scope = scope_as(ec, &owner.username, definition_id)?;
    let verdicts = expand(ec, facade, &users, groups, now)?
        .into_iter()
        .map(|recipient| {
            let (reason, _) = judge(ec, definition_id, owner, &owner_scope, &recipient.account);
            Verdict { recipient, reason }
        })
        .collect();
    let (cap, ceiling) = caps(cap);
    Ok(Audience {
        verdicts,
        users,
        groups: groups.to_vec(),
        mode: if personal { "personal" } else { "group" },
        cap,
        ceiling,
    })
}

/// `check#ReportAudience`.
fn check_audience(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let facade = entity(ec)?;
    let now = moqui_core::user::now_millis();
    let owner = account(ec, &facade, &viewer()?, now)?.ok_or_else(|| fail("the viewer has no account"))?;
    let definition_id = text(p, "definitionId");
    // a dashboard or report a schedule would send, or an alert rule whose state its audience is told (alert_delivery.rs)
    if definitions::read(ec, definition_id)?["type"] != "alert" {
        schedulable(ec, definition_id)?;
    }
    let audience = audience(
        ec,
        &owner,
        definition_id,
        &ids(p, "recipientUserIds"),
        &ids(p, "recipientGroupIds"),
        number(p.get("recipientCap")),
    )?;
    let count = audience.verdicts.len();
    let eligible = audience
        .verdicts
        .iter()
        .filter(|verdict| verdict.reason.is_empty())
        .count();
    Ok(out(json!({
        "recipients": audience.verdicts.iter().map(Verdict::json).collect::<Vec<_>>(),
        "recipientCount": count, "eligibleCount": eligible, "refusedCount": count - eligible,
        "recipientCap": audience.cap, "recipientCeiling": audience.ceiling,
        "overCap": i64::try_from(count).unwrap_or(i64::MAX) > audience.cap, "deliveryMode": audience.mode,
    })))
}

/// The refusal of a save or a resume when a recipient may not be sent the owner's result, or the audience is over the cap.
/// `needs_address` is false where no channel is email (an alert told in the app alone), so a missing address refuses nothing.
pub(crate) fn refuse(audience: &Audience, what: &str, needs_address: bool) -> Result<(), Error> {
    let refused: Vec<String> = audience
        .verdicts
        .iter()
        .filter(|verdict| verdict.blocking() && (needs_address || verdict.reason != "no-address"))
        .map(|verdict| format!("{} ({})", verdict.recipient.account.name, verdict.reason))
        .collect();
    if !refused.is_empty() {
        return Err(fail(format!(
            "RECIPIENT_REFUSED: the {what}'s result may not go to {}",
            refused.join(", ")
        )));
    }
    if i64::try_from(audience.verdicts.len()).unwrap_or(i64::MAX) > audience.cap {
        return Err(fail(format!(
            "RECIPIENT_CAP: {} recipients are more than the cap of {}",
            audience.verdicts.len(),
            audience.cap
        )));
    }
    Ok(())
}

// ===== schedules =====

fn revision(row: &EntityValue) -> i64 {
    number(row.get("versionNumber")).unwrap_or(0)
}

/// The audience check of a business schedule as it stands, for a resume: refused while a recipient differs.
pub(crate) fn check_resume(ec: &ExecutionContext, schedule: &EntityValue) -> Result<(), Error> {
    if field(schedule, "deliveryMode").is_empty() {
        return Ok(());
    }
    let facade = entity(ec)?;
    let now = moqui_core::user::now_millis();
    let owner = account(ec, &facade, field(schedule, "reportOwnerId"), now)?
        .ok_or_else(|| fail("report owner no longer exists"))?;
    let (users, groups) = schedule_recipients(ec, &facade, schedule)?;
    refuse(
        &audience(
            ec,
            &owner,
            field(schedule, "reportDefinitionId"),
            &users,
            &groups,
            number(schedule.get("recipientCap")),
        )?,
        "schedule",
        true,
    )
}

/// `store#Schedule`.
fn store_schedule(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let facade = entity(ec)?;
    let now = moqui_core::user::now_millis();
    let owner = account(ec, &facade, &viewer()?, now)?.ok_or_else(|| fail("the viewer has no account"))?;
    let id = text(p, "screenScheduledId");
    let existing = if id.is_empty() {
        None
    } else {
        let row = schedule::owned(ec, id)?;
        if field(&row, "reportDefinitionId").is_empty() {
            return Err(fail("report schedule not found for this owner"));
        }
        if number(p.get("expectedRevision")) != Some(revision(&row)) {
            return Err(fail(
                "STALE_REVISION: the schedule changed since it was read; read it again",
            ));
        }
        Some(row)
    };
    let or_existing = |name: &str, existing_name: &str| -> String {
        let given = text(p, name);
        if given.is_empty() {
            existing
                .as_ref()
                .map_or(String::new(), |row| field(row, existing_name).to_owned())
        } else {
            given.to_owned()
        }
    };
    let definition_id = or_existing("definitionId", "reportDefinitionId");
    schedulable(ec, &definition_id)?;
    let cron = or_existing("cronExpression", "cronExpression");
    moqui_lib::cron::Cron::parse(&cron).map_err(fail)?;
    let mode = Some(or_existing("renderMode", "renderMode"))
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| "pdf".to_owned());
    schedule::validate_mode(&mode)?;
    // the recipients: those given, else the ones the schedule has
    // an empty list names nobody (the owner alone); a list not sent keeps the recipients as they are
    let sent = |name: &str| p.get(name).is_some_and(|value| !value.is_null());
    let given = sent("recipientUserIds") || sent("recipientGroupIds");
    let (users, groups) = match &existing {
        Some(row) if !given => schedule_recipients(ec, &facade, row)?,
        _ => (ids(p, "recipientUserIds"), ids(p, "recipientGroupIds")),
    };
    let cap =
        number(p.get("recipientCap")).or_else(|| existing.as_ref().and_then(|row| number(row.get("recipientCap"))));
    let audience = audience(ec, &owner, &definition_id, &users, &groups, cap)?;
    let paused = flag(p, "paused").unwrap_or_else(|| existing.as_ref().is_some_and(|row| field(row, "paused") == "Y"));
    // a paused schedule may be saved with a recipient to be fixed; an active one only once every recipient passes
    if !paused {
        refuse(&audience, "schedule", true)?;
    }
    let template = or_existing("emailTemplateId", "emailTemplateId");
    let _authz = ec.disable_authz();
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(SCHEDULE)?;
            row.set("screenPath", Value::string("/apps/dashboards/Reports"));
            row.set("reportOwnerId", Value::string(&owner.user_id));
            row.set("noResultsAbort", Value::string("Y"));
            row.set_sequenced_id_primary(&facade)?;
            row
        }
    };
    let next = revision(&row) + 1;
    row.set("reportDefinitionId", Value::string(&definition_id));
    row.set("cronExpression", Value::string(&cron));
    row.set("renderMode", Value::string(&mode));
    row.set("deliveryMode", Value::string(audience.mode));
    row.set("recipientCap", Value::I64(audience.cap));
    row.set("versionNumber", Value::I64(next));
    row.set("paused", Value::string(if paused { "Y" } else { "N" }));
    if !paused {
        row.set("pausedReason", Value::Null);
    }
    row.set("saveToLocation", Value::Null);
    row.set("userGroupId", Value::Null);
    row.set(
        "userId",
        if audience.mode == "personal" {
            Value::string(&owner.user_id)
        } else {
            Value::Null
        },
    );
    row.set(
        "emailTemplateId",
        Value::string(if template.is_empty() {
            DEFAULT_TEMPLATE
        } else {
            &template
        }),
    );
    if matches!(text(p, "noResultsAbort"), "Y" | "N") {
        row.set("noResultsAbort", Value::string(text(p, "noResultsAbort")));
    }
    // a value not sent is kept; clearFields names the ones to empty (a form posts an emptied field as no value)
    let clear = ids(p, "clearFields");
    for name in ["emailSubject", "fromDate", "thruDate"] {
        if clear.iter().any(|cleared| cleared == name) {
            row.set(name, Value::Null);
        } else if let Some(value) = p.get(name).filter(|value| !value.is_null()) {
            row.set(name, value.clone());
        }
    }
    let id = field(&row, "screenScheduledId").to_owned();
    if creating {
        row.create(&facade)?;
        let mut lock = facade.make_value(LOCK)?;
        lock.set("screenScheduledId", Value::string(&id));
        lock.create(&facade)?;
    } else {
        row.update(&facade)?;
        // what the earlier revision had not yet sent is not sent under it
        for occurrence in facade
            .find(OCCURRENCE)?
            .eq("screenScheduledId", Value::string(&id))
            .eq("statusId", Value::string(RUNNING))
            .list()?
            .iter()
        {
            facade
                .find(DELIVERY)?
                .eq("occurrenceId", Value::string(field(occurrence, "occurrenceId")))
                .eq("statusId", Value::string(PENDING))
                .update_all([
                    ("statusId".into(), Value::string(SKIPPED)),
                    ("reasonCode".into(), Value::string("schedule-changed")),
                ])?;
        }
    }
    facade
        .find(RECIPIENT)?
        .eq("screenScheduledId", Value::string(&id))
        .delete_all()?;
    if audience.mode == "group" {
        for (kind, list) in [("user", &audience.users), ("group", &audience.groups)] {
            for recipient in list {
                let mut entry = facade.make_value(RECIPIENT)?;
                entry.set("screenScheduledId", Value::string(&id));
                entry.set("recipientKind", Value::string(kind));
                entry.set("recipientId", Value::string(recipient));
                entry.create(&facade)?;
            }
        }
    }
    tracing::info!(target: "moqui.report.audit", user = %owner.user_id, schedule = %id, definition = %definition_id,
        mode = audience.mode, recipients = audience.verdicts.len(), revision = next, "report schedule stored");
    Ok(out(
        json!({"screenScheduledId": id, "deliveryMode": audience.mode, "revision": next,
        "recipientCount": audience.verdicts.len()}),
    ))
}

/// One schedule as `list#Schedules` answers it.
pub(crate) fn describe(ec: &ExecutionContext, facade: &EntityFacade, row: &EntityValue) -> Result<Json, Error> {
    let id = field(row, "screenScheduledId");
    let definition_id = field(row, "reportDefinitionId");
    let (users, groups) = schedule_recipients(ec, facade, row)?;
    let _authz = ec.disable_authz();
    let lock = facade
        .find(LOCK)?
        .use_cache(false)
        .eq("screenScheduledId", Value::string(id))
        .one()?;
    let title = definitions::read(ec, definition_id)
        .ok()
        .and_then(|definition| definition["title"].as_str().map(str::to_owned));
    Ok(json!({
        "screenScheduledId": id, "definitionId": definition_id, "definitionTitle": title,
        "cronExpression": field(row, "cronExpression"), "renderMode": field(row, "renderMode"),
        "paused": field(row, "paused") == "Y", "pausedReason": field(row, "pausedReason"),
        "deliveryMode": field(row, "deliveryMode"), "revision": revision(row),
        "recipientUserIds": users, "recipientGroupIds": groups, "recipientCap": number(row.get("recipientCap")),
        "emailTemplateId": field(row, "emailTemplateId"), "emailSubject": field(row, "emailSubject"),
        "noResultsAbort": field(row, "noResultsAbort") == "Y",
        "fromDate": millis(row, "fromDate"), "thruDate": millis(row, "thruDate"),
        "lastRunTime": lock.as_ref().map_or(Json::Null, |lock| millis(lock, "lastRunTime")),
    }))
}

// ===== a run =====

/// What a retry sends: the failed deliveries' recipients, under the occurrence that holds them.
pub(crate) struct Retry {
    pub occurrence_id: String,
    pub users: BTreeSet<String>,
}

/// Why a business schedule's owner cannot run it now, if so: the account is gone, disabled or terminated.
pub(crate) fn owner_inactive(ec: &ExecutionContext, facade: &EntityFacade, schedule: &EntityValue, now: i64) -> bool {
    !matches!(account(ec, facade, field(schedule, "reportOwnerId"), now), Ok(Some(owner)) if owner.active)
}

/// Pause a schedule whose owner is not active, until a transfer hands it on.
pub(crate) fn pause_for_owner(ec: &ExecutionContext, facade: &EntityFacade, id: &str) {
    let paused = write(ec, facade, || {
        facade
            .find(SCHEDULE)?
            .eq("screenScheduledId", Value::string(id))
            .update_all([
                ("paused".into(), Value::string("Y")),
                ("pausedReason".into(), Value::string("owner-disabled")),
            ])
    });
    match paused {
        Ok(_) => {
            tracing::info!(target: "moqui.report.audit", schedule = %id, "report schedule paused: its owner is not active")
        }
        Err(error) => {
            tracing::error!(schedule = %id, %error, "could not pause a report schedule whose owner is not active")
        }
    }
}

/// Open the occurrence of a run and settle what an earlier run of the schedule left unsettled: this run holds the
/// schedule's lease, so a delivery still pending or sending belongs to a run that died, and its outcome is not known.
pub(crate) fn open_occurrence(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    schedule: &EntityValue,
    instant: Option<i64>,
    now: i64,
) -> Result<String, Error> {
    let id = field(schedule, "screenScheduledId");
    let definition_id = field(schedule, "reportDefinitionId");
    let version = definitions::read_unchecked(ec, definition_id)
        .ok()
        .flatten()
        .and_then(|definition| definition["version"].as_i64());
    write(ec, facade, || {
        for stale in facade
            .find(OCCURRENCE)?
            .eq("screenScheduledId", Value::string(id))
            .eq("statusId", Value::string(RUNNING))
            .list()?
            .iter()
        {
            settle(facade, field(stale, "occurrenceId"), None)?;
        }
        let key = match instant {
            Some(instant) => format!("{id}:{instant}"),
            None => format!("{id}:manual:{}", moqui_core::entity::uuid_v4()?),
        };
        // a cron occurrence a failed run already opened is run again under its own key
        if let Some(mut row) = facade
            .find(OCCURRENCE)?
            .eq("occurrenceKey", Value::string(&key))
            .one()?
        {
            row.set("statusId", Value::string(RUNNING));
            row.update(facade)?;
            return Ok(field(&row, "occurrenceId").to_owned());
        }
        let mut row = facade.make_value(OCCURRENCE)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("screenScheduledId", Value::string(id));
        row.set("occurrenceKey", Value::string(key));
        row.set(
            "occurrenceKind",
            Value::string(if instant.is_some() { "scheduled" } else { "manual" }),
        );
        row.set("occurrenceDate", Value::I64(now));
        row.set("ownerUserId", Value::string(field(schedule, "reportOwnerId")));
        row.set("scheduleRevision", Value::I64(revision(schedule)));
        row.set("definitionId", Value::string(definition_id));
        if let Some(version) = version {
            row.set("definitionVersion", Value::I64(version));
        }
        row.set("renderMode", Value::string(field(schedule, "renderMode")));
        row.set("deliveryMode", Value::string(field(schedule, "deliveryMode")));
        row.set("statusId", Value::string(RUNNING));
        row.set("versionNumber", Value::I64(1));
        row.create(facade)?;
        Ok(field(&row, "occurrenceId").to_owned())
    })
}

/// Mark a retried occurrence as running again.
pub(crate) fn reopen_occurrence(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    occurrence_id: &str,
) -> Result<(), Error> {
    write(ec, facade, || {
        facade
            .find(OCCURRENCE)?
            .eq("occurrenceId", Value::string(occurrence_id))
            .update_all([("statusId".into(), Value::string(RUNNING))])
            .map(|_| ())
    })
}

/// Count an occurrence's deliveries and set its state. `outcome` is the run's own result (None for a run that died): a
/// skipped run's reason, or an error. A delivery left pending or sending is settled first: not delivered, or not known.
fn settle(
    facade: &EntityFacade,
    occurrence_id: &str,
    outcome: Option<&Result<Parameters, Error>>,
) -> Result<(), Error> {
    let deliveries = || {
        facade
            .find(DELIVERY)
            .map(|find| find.eq("occurrenceId", Value::string(occurrence_id)))
    };
    deliveries()?.eq("statusId", Value::string(SENDING)).update_all([
        ("statusId".into(), Value::string(UNCERTAIN)),
        ("reasonCode".into(), Value::string("uncertain")),
    ])?;
    deliveries()?.eq("statusId", Value::string(PENDING)).update_all([
        ("statusId".into(), Value::string(SKIPPED)),
        ("reasonCode".into(), Value::string("delivery-blocked")),
    ])?;
    let count = |status: &str| deliveries()?.eq("statusId", Value::string(status)).count();
    let (sent, skipped, failed, uncertain) = (count(SENT)?, count(SKIPPED)?, count(FAILED)?, count(UNCERTAIN)?);
    let skipped_reason = outcome
        .and_then(|outcome| outcome.as_ref().ok())
        .filter(|result| result.get("skipped") == Some(&Value::Bool(true)))
        .map(|result| match result.get("reason").and_then(Value::as_str) {
            Some("empty report") => "empty",
            Some("owner-inactive") => "owner-inactive",
            _ => "skipped",
        });
    let (status, reason) = if failed + uncertain > 0 {
        ("RptOccIncomplete", "")
    } else if let Some(reason) = skipped_reason {
        ("RptOccSkipped", reason)
    } else if outcome.is_none_or(|outcome| outcome.is_err()) && sent == 0 {
        // the error's text may name a source or a value: only that the run failed is kept
        (
            "RptOccFailed",
            if outcome.is_none() {
                "run-ended"
            } else {
                "source-failure"
            },
        )
    } else {
        ("RptOccComplete", "")
    };
    let Some(mut row) = facade
        .find(OCCURRENCE)?
        .eq("occurrenceId", Value::string(occurrence_id))
        .one()?
    else {
        return Ok(());
    };
    row.set("statusId", Value::string(status));
    row.set("reasonCode", Value::string(reason));
    row.set("completedDate", Value::I64(moqui_core::user::now_millis()));
    row.set("recipientCount", Value::I64(sent + skipped + failed + uncertain));
    row.set("sentCount", Value::I64(sent));
    row.set("skippedCount", Value::I64(skipped));
    row.set("failedCount", Value::I64(failed));
    row.set("uncertainCount", Value::I64(uncertain));
    row.set("versionNumber", Value::I64(revision(&row) + 1));
    row.update(facade)
}

/// Close a run's occurrence with its result. A failure to record it is logged: the run's own result stands.
pub(crate) fn close_occurrence(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    occurrence_id: &str,
    outcome: &Result<Parameters, Error>,
    renders: i64,
) {
    let closed = write(ec, facade, || {
        if renders > 0 {
            if let Some(mut row) = facade
                .find(OCCURRENCE)?
                .eq("occurrenceId", Value::string(occurrence_id))
                .one()?
            {
                row.set(
                    "renderCount",
                    Value::I64(number(row.get("renderCount")).unwrap_or(0) + renders),
                );
                row.update(facade)?;
            }
        }
        settle(facade, occurrence_id, Some(outcome))
    });
    if let Err(error) = closed {
        tracing::error!(occurrence = %occurrence_id, %error, "could not close a report occurrence");
    }
}

/// What [`send_each`] needs of a run.
pub(crate) struct Sending<'a> {
    pub ec: &'a ExecutionContext,
    pub facade: &'a EntityFacade,
    pub schedule: &'a EntityValue,
    pub occurrence_id: &'a str,
    pub now: i64,
    /// A retry's recipients; every recipient of the schedule otherwise.
    pub only: Option<&'a BTreeSet<String>>,
    /// The owner's own render, which serves every recipient of the owner's locale.
    pub first: &'a Rendered,
    /// The owner's render in another locale.
    pub render: &'a dyn Fn(&str) -> Result<Rendered, Error>,
    /// The lease, the owner and the schedule checked again before an effect.
    pub check: &'a dyn Fn() -> Result<(), Error>,
    /// One message to one address.
    pub mail: &'a dyn Fn(&str, &Rendered) -> Result<Parameters, Error>,
}

/// The state of a recipient's delivery in the occurrence as it stands, "" when they have none yet.
fn standing(s: &Sending<'_>, user_id: &str) -> Result<String, Error> {
    let _authz = s.ec.disable_authz();
    Ok(s.facade
        .find(DELIVERY)?
        .use_cache(false)
        .eq("occurrenceId", Value::string(s.occurrence_id))
        .eq("userId", Value::string(user_id))
        .eq("channel", Value::string("email"))
        .one()?
        .map(|row| field(&row, "statusId").to_owned())
        .unwrap_or_default())
}

/// Record one recipient's state in an occurrence, creating the row or revising it.
fn record(
    s: &Sending<'_>,
    recipient: &Account,
    status: &str,
    reason: &str,
    scope_hash: &str,
    attempt: Option<&str>,
    message_id: Option<&str>,
) -> Result<(), Error> {
    write(s.ec, s.facade, || {
        let existing = s
            .facade
            .find(DELIVERY)?
            .eq("occurrenceId", Value::string(s.occurrence_id))
            .eq("userId", Value::string(&recipient.user_id))
            .eq("channel", Value::string("email"))
            .one()?;
        let creating = existing.is_none();
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = s.facade.make_value(DELIVERY)?;
                row.set_sequenced_id_primary(s.facade)?;
                row.set("occurrenceId", Value::string(s.occurrence_id));
                row.set("userId", Value::string(&recipient.user_id));
                row.set("channel", Value::string("email"));
                row
            }
        };
        // a delivery known to be sent is never revised by a later pass
        if field(&row, "statusId") == SENT {
            return Ok(());
        }
        row.set("statusId", Value::string(status));
        row.set("reasonCode", Value::string(reason));
        if !scope_hash.is_empty() {
            row.set("scopeHash", Value::string(scope_hash));
        }
        if let Some(locale) = &recipient.locale {
            row.set("locale", Value::string(locale));
        }
        if let Some(attempt) = attempt {
            row.set("attemptId", Value::string(attempt));
            row.set(
                "attemptCount",
                Value::I64(number(row.get("attemptCount")).unwrap_or(0) + 1),
            );
            row.set("attemptDate", Value::I64(moqui_core::user::now_millis()));
        }
        if let Some(message_id) = message_id {
            row.set("messageId", Value::string(message_id));
        }
        if creating {
            row.create(s.facade)
        } else {
            row.update(s.facade)
        }
    })
}

/// Send a business schedule's result to each of its recipients, one message each. Every recipient is checked again
/// just before their message; one who may not be sent the owner's result is skipped with the reason, one whose
/// message fails is recorded and the others go on. Returns the addresses delivered and how many renders it added; an
/// error when a recipient failed or the run was stopped, after the others were tried.
pub(crate) fn send_each(s: &Sending<'_>, attempted: &mut bool, renders: &mut i64) -> Result<Vec<String>, Error> {
    let definition_id = field(s.schedule, "reportDefinitionId");
    let owner = account(s.ec, s.facade, field(s.schedule, "reportOwnerId"), s.now)?
        .ok_or_else(|| fail("report owner no longer exists"))?;
    let (users, groups) = schedule_recipients(s.ec, s.facade, s.schedule)?;
    let mut recipients = expand(s.ec, s.facade, &users, &groups, s.now)?;
    let owner_scope = scope_as(s.ec, &owner.username, definition_id)?;
    let others = recipients
        .iter()
        .any(|recipient| recipient.account.user_id != owner.user_id);
    let may_distribute = !others || holds_as(s.ec, &owner.username, GROUP_ARTIFACT, "create");
    let (cap, _) = caps(number(s.schedule.get("recipientCap")));
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    // the owner first, so a cap never leaves out the one whose result it is
    recipients.sort_by_key(|recipient| {
        (
            recipient.account.user_id != owner.user_id,
            recipient.account.user_id.clone(),
        )
    });
    let mut by_locale: BTreeMap<String, Rendered> = BTreeMap::new();
    let mut delivered = Vec::new();
    let (mut failed, mut considered) = (0_usize, 0_usize);
    let mut blocked: Option<Error> = None;
    if let Some(only) = s.only {
        // a retried recipient who is no longer in the audience is not sent to
        for user in only {
            if !recipients.iter().any(|recipient| &recipient.account.user_id == user) {
                if let Some(gone) = account(s.ec, s.facade, user, s.now)? {
                    record(s, &gone, SKIPPED, "removed-audience", "", None, None)?;
                }
            }
        }
    }
    for (position, recipient) in recipients.iter().enumerate() {
        let recipient = &recipient.account;
        if s.only.is_some_and(|only| !only.contains(&recipient.user_id)) {
            continue;
        }
        // An occurrence run again (a run that died or failed before its schedule's last run time was kept, a second
        // retry of the same delivery): one who was sent it is never sent it twice, and one whose outcome is not known
        // waits for a person's word (reconcile#ReportRecipient). Their rows stand as they are.
        if matches!(standing(s, &recipient.user_id)?.as_str(), SENT | UNCERTAIN) {
            continue;
        }
        considered += 1;
        if blocked.is_some() {
            record(s, recipient, SKIPPED, "delivery-blocked", "", None, None)?;
            continue;
        }
        if position >= cap {
            record(s, recipient, SKIPPED, "over-cap", "", None, None)?;
            continue;
        }
        if recipient.user_id != owner.user_id && !may_distribute {
            record(s, recipient, SKIPPED, "owner-unauthorized", "", None, None)?;
            continue;
        }
        let (reason, scope_hash) = judge(s.ec, definition_id, &owner, &owner_scope, recipient);
        if !reason.is_empty() {
            record(s, recipient, SKIPPED, reason, "", None, None)?;
            continue;
        }
        let Some(address) = recipient.address.as_deref() else {
            continue;
        };
        if let Err(error) = (s.check)() {
            tracing::error!(schedule = %field(s.schedule, "screenScheduledId"), %error,
                "report delivery stopped before a recipient; the recipients left are not sent");
            record(s, recipient, SKIPPED, "delivery-blocked", "", None, None)?;
            blocked = Some(error);
            continue;
        }
        // one render a locale: every recipient here shares the definition's version and the owner's fingerprint
        let locale = recipient
            .locale
            .clone()
            .filter(|locale| Some(locale) != owner.locale.as_ref());
        let rendered = match &locale {
            None => s.first,
            Some(locale) => {
                if !by_locale.contains_key(locale) {
                    match (s.render)(locale) {
                        Ok(rendered) => {
                            *renders += 1;
                            by_locale.insert(locale.clone(), rendered);
                        }
                        Err(error) => {
                            tracing::error!(schedule = %field(s.schedule, "screenScheduledId"), locale = %locale, %error,
                                "report render for a locale failed; its recipients are not sent, the others go on");
                            record(s, recipient, FAILED, "source-failure", &scope_hash, None, None)?;
                            failed += 1;
                            continue;
                        }
                    }
                }
                &by_locale[locale]
            }
        };
        let attempt = moqui_core::entity::uuid_v4()?;
        record(s, recipient, SENDING, "", &scope_hash, Some(&attempt), None)?;
        *attempted = true;
        match (s.mail)(address, rendered) {
            Ok(mail) => {
                let answer = |name: &str| mail.get(name).and_then(Value::as_str).unwrap_or("");
                let message_id = answer("messageId");
                // the transport says what it knows: a message id is a send; a rejection, or a server that was not
                // reached, is a failure that may be retried; anything else (a connection lost after it was made, no
                // answer at all) is not known, and is never sent again without a person's word
                if !message_id.is_empty() {
                    record(s, recipient, SENT, "", &scope_hash, None, Some(message_id))?;
                    delivered.push(address.to_owned());
                } else if matches!(answer("deliveryOutcome"), "retryable" | "permanent")
                    || !answer("sendError").is_empty()
                {
                    record(s, recipient, FAILED, "transport-failure", &scope_hash, None, None)?;
                    failed += 1;
                } else {
                    record(s, recipient, UNCERTAIN, "uncertain", &scope_hash, None, None)?;
                    failed += 1;
                }
            }
            Err(error) => {
                tracing::error!(schedule = %field(s.schedule, "screenScheduledId"), recipient = %recipient.user_id, %error,
                    "report email to one recipient failed; the others go on");
                record(s, recipient, FAILED, "transport-failure", &scope_hash, None, None)?;
                failed += 1;
            }
        }
    }
    if !delivered.is_empty() {
        tracing::info!(target: "moqui.report.audit", occurrence = %s.occurrence_id, definition = %definition_id,
            sent = delivered.len(), considered, "report delivered to each recipient separately");
    }
    if let Some(error) = blocked {
        return Err(error);
    }
    if failed > 0 {
        return Err(fail(format!(
            "RECIPIENTS_FAILED: {failed} of {considered} report recipients were not delivered; each delivery says why"
        )));
    }
    Ok(delivered)
}

// ===== occurrences =====

fn occurrence_json(row: &EntityValue, titled: bool, ec: &ExecutionContext) -> Json {
    let mut json = json!({
        "occurrenceId": field(row, "occurrenceId"), "screenScheduledId": field(row, "screenScheduledId"),
        "occurrenceKind": field(row, "occurrenceKind"), "occurrenceDate": millis(row, "occurrenceDate"),
        "completedDate": millis(row, "completedDate"), "ownerUserId": field(row, "ownerUserId"),
        "scheduleRevision": number(row.get("scheduleRevision")), "definitionId": field(row, "definitionId"),
        "definitionVersion": number(row.get("definitionVersion")), "renderMode": field(row, "renderMode"),
        "deliveryMode": field(row, "deliveryMode"), "statusId": field(row, "statusId"),
        "reasonCode": field(row, "reasonCode"), "recipientCount": number(row.get("recipientCount")).unwrap_or(0),
        "sentCount": number(row.get("sentCount")).unwrap_or(0), "skippedCount": number(row.get("skippedCount")).unwrap_or(0),
        "failedCount": number(row.get("failedCount")).unwrap_or(0),
        "uncertainCount": number(row.get("uncertainCount")).unwrap_or(0),
        "renderCount": number(row.get("renderCount")).unwrap_or(0), "revision": revision(row),
    });
    if titled {
        // the title only where the viewer reads the definition
        json["title"] = definitions::read(ec, field(row, "definitionId"))
            .ok()
            .map_or(Json::Null, |definition| definition["title"].clone());
    }
    json
}
fn delivery_json(row: &EntityValue) -> Json {
    json!({
        "deliveryId": field(row, "deliveryId"), "occurrenceId": field(row, "occurrenceId"), "userId": field(row, "userId"),
        "statusId": field(row, "statusId"), "reasonCode": field(row, "reasonCode"),
        "reason": reason_text(field(row, "reasonCode")), "attemptDate": millis(row, "attemptDate"),
        "attemptCount": number(row.get("attemptCount")).unwrap_or(0), "retryable": field(row, "statusId") == FAILED,
        "uncertain": field(row, "statusId") == UNCERTAIN,
    })
}

/// `find#ReportOccurrences`.
fn find_occurrences(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = viewer()?;
    let facade = entity(ec)?;
    let size = number(p.get("pageSize")).unwrap_or(20).clamp(1, 100);
    let index = number(p.get("pageIndex")).unwrap_or(0).clamp(0, 10_000 / size);
    let received = flag(p, "received") == Some(true);
    let administrative = flag(p, "administrative") == Some(true);
    if administrative && !moqui_core::user::has_permission(ADMIN_PERMISSION)? {
        return Err(fail(format!(
            "{ADMIN_PERMISSION} is required to list every owner's report occurrences"
        )));
    }
    let _authz = ec.disable_authz();
    if received {
        let mine = || {
            facade
                .find(DELIVERY)
                .map(|find| find.use_cache(false).eq("userId", Value::string(&user)))
        };
        let total = mine()?.count()?;
        let mut occurrences = Vec::new();
        for delivery in mine()?
            .order_by("attemptDate", true)
            .order_by("deliveryId", true)
            .offset(index * size)
            .limit(size)
            .list()?
            .iter()
        {
            let Some(occurrence) = facade
                .find(OCCURRENCE)?
                .eq("occurrenceId", Value::string(field(delivery, "occurrenceId")))
                .one()?
            else {
                continue;
            };
            // what a recipient learns of the run: when, what of, and their own delivery; no counters of the others
            occurrences.push(json!({
                "occurrenceId": field(&occurrence, "occurrenceId"), "occurrenceDate": millis(&occurrence, "occurrenceDate"),
                "definitionId": field(&occurrence, "definitionId"), "ownerUserId": field(&occurrence, "ownerUserId"),
                "title": definitions::read(ec, field(&occurrence, "definitionId")).ok().map_or(Json::Null, |d| d["title"].clone()),
                "ownDelivery": delivery_json(delivery),
            }));
        }
        return Ok(out(
            json!({"occurrences": occurrences, "total": total, "pageIndex": index, "pageSize": size}),
        ));
    }
    let scoped = || -> Result<_, Error> {
        let mut find = facade.find(OCCURRENCE)?.use_cache(false);
        if !administrative {
            find = find.eq("ownerUserId", Value::string(&user));
        }
        for name in ["screenScheduledId", "statusId"] {
            if !text(p, name).is_empty() {
                find = find.eq(name, Value::string(text(p, name)));
            }
        }
        Ok(find)
    };
    let total = scoped()?.count()?;
    let occurrences: Vec<Json> = scoped()?
        .order_by("occurrenceDate", true)
        .order_by("occurrenceId", true)
        .offset(index * size)
        .limit(size)
        .list()?
        .iter()
        .map(|row| occurrence_json(row, !administrative, ec))
        .collect();
    Ok(out(
        json!({"occurrences": occurrences, "total": total, "pageIndex": index, "pageSize": size}),
    ))
}

/// An occurrence, with whether the viewer administers it (its schedule's owner now, or the administrative permission).
fn occurrence_for(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    occurrence_id: &str,
    user: &str,
) -> Result<Option<(EntityValue, bool, bool)>, Error> {
    let _authz = ec.disable_authz();
    let Some(row) = facade
        .find(OCCURRENCE)?
        .use_cache(false)
        .eq("occurrenceId", Value::string(occurrence_id))
        .one()?
    else {
        return Ok(None);
    };
    let owner = facade
        .find(SCHEDULE)?
        .use_cache(false)
        .eq("screenScheduledId", Value::string(field(&row, "screenScheduledId")))
        .one()?
        .is_some_and(|schedule| field(&schedule, "reportOwnerId") == user);
    let administrator = !owner && moqui_core::user::has_permission(ADMIN_PERMISSION)?;
    Ok(Some((row, owner, administrator)))
}

/// `get#ReportOccurrence`.
fn get_occurrence(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = viewer()?;
    let facade = entity(ec)?;
    let missing = || fail("report occurrence not available");
    let (row, owner, administrator) =
        occurrence_for(ec, &facade, text(p, "occurrenceId"), &user)?.ok_or_else(missing)?;
    let _authz = ec.disable_authz();
    let deliveries = facade
        .find(DELIVERY)?
        .use_cache(false)
        .eq("occurrenceId", Value::string(field(&row, "occurrenceId")))
        .order_by("deliveryId", false)
        .list()?;
    let own = deliveries
        .iter()
        .find(|delivery| field(delivery, "userId") == user)
        .map(delivery_json);
    if !owner && !administrator {
        // a recipient who administers nothing sees their own delivery, and of the run only what their message told them
        let own = own.ok_or_else(missing)?;
        return Ok(out(json!({
            "occurrence": {"occurrenceId": field(&row, "occurrenceId"), "occurrenceDate": millis(&row, "occurrenceDate"),
                "definitionId": field(&row, "definitionId"), "renderMode": field(&row, "renderMode"),
                "title": definitions::read(ec, field(&row, "definitionId")).ok().map_or(Json::Null, |d| d["title"].clone())},
            "deliveries": [], "ownDelivery": own, "administers": false,
        })));
    }
    let now = moqui_core::user::now_millis();
    let mut listed = Vec::new();
    for delivery in deliveries.iter() {
        let mut json = delivery_json(delivery);
        if let Some(recipient) = account(ec, &facade, field(delivery, "userId"), now)? {
            json["username"] = json!(recipient.username);
            json["userFullName"] = json!(recipient.name);
        }
        // only the owner retries; an administrator reads the outcomes
        json["retryable"] = json!(owner && json["retryable"] == true);
        listed.push(json);
    }
    // whose run it was, by name: the owner's account is the engine's to read, so a screen that shows the occurrence to
    // its owner or to the administrator reads no account itself (plans/Admin-Screens-Down.md, Track I)
    let mut occurrence = occurrence_json(&row, owner, ec);
    occurrence["ownerName"] =
        account(ec, &facade, field(&row, "ownerUserId"), now)?.map_or(Json::Null, |owner| json!(owner.name));
    Ok(out(json!({
        "occurrence": occurrence, "deliveries": listed, "ownDelivery": own, "administers": owner,
    })))
}

/// `get#RecipientReport`.
fn get_recipient_report(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    use base64::Engine;
    let user = viewer()?;
    let facade = entity(ec)?;
    let missing = || fail("report delivery not available");
    // another user's delivery is refused as one that does not exist
    let (delivery, occurrence) = {
        let _authz = ec.disable_authz();
        let delivery = facade
            .find(DELIVERY)?
            .use_cache(false)
            .eq("deliveryId", Value::string(text(p, "deliveryId")))
            .one()?
            .filter(|delivery| field(delivery, "userId") == user && field(delivery, "statusId") == SENT)
            .ok_or_else(missing)?;
        let occurrence = facade
            .find(OCCURRENCE)?
            .use_cache(false)
            .eq("occurrenceId", Value::string(field(&delivery, "occurrenceId")))
            .one()?
            .ok_or_else(missing)?;
        (delivery, occurrence)
    };
    let definition_id = field(&occurrence, "definitionId");
    let username = moqui_core::user::current_username()?.unwrap_or_default();
    // the recipient's scope now against the one the delivery was sent under: what was sent is not released to a scope
    // that has since changed
    let current = scope_as(ec, &username, definition_id)?;
    if current.denied || current.hash != field(&delivery, "scopeHash") {
        return Ok(out(
            json!({"available": false, "reasonCode": "scope-changed", "definitionId": definition_id}),
        ));
    }
    let mode = if text(p, "renderMode").is_empty() {
        field(&occurrence, "renderMode")
    } else {
        text(p, "renderMode")
    };
    // nothing sent is stored: the recipient's own render, as themselves, now
    let rendered = schedule::render_as(ec, &username, None, definition_id, mode, false)?;
    crate::audit_export("recipient", rendered.extension, definition_id, rendered.rows, &[]);
    let (bytes, mime, filename) = schedule::download(&rendered)?;
    Ok(out(json!({
        "available": true, "contentBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "contentType": mime, "filename": filename, "rerendered": true, "generatedDate": moqui_core::user::now_millis(),
        "definitionId": definition_id, "title": rendered.title,
    })))
}

/// `retry#ReportRecipients`.
fn retry_recipients(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = viewer()?;
    let facade = entity(ec)?;
    let occurrence_id = text(p, "occurrenceId");
    let (occurrence, _, _) = occurrence_for(ec, &facade, occurrence_id, &user)?
        .filter(|(_, owner, _)| *owner)
        .ok_or_else(|| fail("report occurrence not available"))?;
    if number(p.get("expectedRevision")) != Some(revision(&occurrence)) {
        return Err(fail(
            "STALE_REVISION: the occurrence changed since it was read; read it again",
        ));
    }
    let schedule = schedule::owned(ec, field(&occurrence, "screenScheduledId"))?;
    let version = definitions::read_unchecked(ec, field(&occurrence, "definitionId"))?
        .and_then(|definition| definition["version"].as_i64());
    if number(occurrence.get("scheduleRevision")) != Some(revision(&schedule))
        || number(occurrence.get("definitionVersion")) != version
    {
        return Err(fail(
            "DEFINITION_CHANGED: the schedule or its report changed since this occurrence; send a new one",
        ));
    }
    let wanted = ids(p, "deliveryIds");
    let mut users = BTreeSet::new();
    let mut results = Vec::new();
    {
        let _authz = ec.disable_authz();
        for delivery_id in &wanted {
            let delivery = facade
                .find(DELIVERY)?
                .use_cache(false)
                .eq("deliveryId", Value::string(delivery_id))
                .eq("occurrenceId", Value::string(occurrence_id))
                .one()?;
            // a batch names each refusal and goes on with the rest
            let refusal = |code: &str, reason: &str| json!({"deliveryId": delivery_id, "retried": false, "reasonCode": code, "reason": reason});
            match delivery {
                Some(delivery) if field(&delivery, "statusId") == FAILED => {
                    users.insert(field(&delivery, "userId").to_owned());
                }
                Some(delivery) if field(&delivery, "statusId") == UNCERTAIN => results.push(refusal(
                    "uncertain",
                    "Reconcile an uncertain delivery before sending it again",
                )),
                Some(_) => results.push(refusal("not-failed", "Only a failed delivery is retried")),
                None => results.push(refusal("not-found", "Not a delivery of this occurrence")),
            }
        }
    }
    if !users.is_empty() {
        tracing::info!(target: "moqui.report.audit", user = %user, occurrence = %occurrence_id, recipients = users.len(),
            reason = %text(p, "reason"), "report recipients retried");
        let retry = Retry {
            occurrence_id: occurrence_id.to_owned(),
            users,
        };
        // a recipient's failure is in their row; an error here is the run's own (a lease held, the owner refused)
        if let Err(error) = schedule::deliver(ec, &schedule, false, Some(&retry)) {
            tracing::error!(occurrence = %occurrence_id, %error, "report retry ended with an error; each delivery says its outcome");
        }
        let _authz = ec.disable_authz();
        for delivery in facade
            .find(DELIVERY)?
            .use_cache(false)
            .eq("occurrenceId", Value::string(occurrence_id))
            .list()?
            .iter()
            .filter(|delivery| retry.users.contains(field(delivery, "userId")))
        {
            let mut json = delivery_json(delivery);
            json["retried"] = json!(true);
            results.push(json);
        }
    }
    let sent = results.iter().filter(|result| result["statusId"] == SENT).count();
    Ok(out(json!({"results": results, "sentCount": sent})))
}

/// `reconcile#ReportRecipient`.
fn reconcile_recipient(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = viewer()?;
    let facade = entity(ec)?;
    let missing = || fail("report delivery not available");
    let status = match text(p, "outcome") {
        "sent" => SENT,
        "failed" => FAILED,
        _ => return Err(fail("outcome is sent or failed")),
    };
    if text(p, "reason").trim().is_empty() {
        return Err(fail("a reason is required to settle an uncertain delivery"));
    }
    let mut delivery = {
        let _authz = ec.disable_authz();
        facade
            .find(DELIVERY)?
            .use_cache(false)
            .eq("deliveryId", Value::string(text(p, "deliveryId")))
            .one()?
            .ok_or_else(missing)?
    };
    let occurrence_id = field(&delivery, "occurrenceId").to_owned();
    occurrence_for(ec, &facade, &occurrence_id, &user)?
        .filter(|(_, owner, _)| *owner)
        .ok_or_else(missing)?;
    if field(&delivery, "statusId") != UNCERTAIN {
        return Err(fail("only an uncertain delivery is reconciled"));
    }
    let _authz = ec.disable_authz();
    delivery.set("statusId", Value::string(status));
    delivery.set(
        "reasonCode",
        Value::string(if status == SENT { "" } else { "transport-failure" }),
    );
    delivery.update(&facade)?;
    // the occurrence's counters and state follow; the run itself ended long ago
    settle(&facade, &occurrence_id, Some(&Ok(Parameters::new())))?;
    tracing::info!(target: "moqui.report.audit", user = %user, delivery = %text(p, "deliveryId"), outcome = %text(p, "outcome"),
        reason = %text(p, "reason"), "uncertain report delivery reconciled");
    Ok(out(json!({"statusId": status})))
}

// ===== ownership: the lifecycle participant =====

/// The viewer acting on `user_id`'s schedules: their own, or another's with the owner-change permission.
pub(crate) fn owner_authority(user_id: &str) -> Result<String, Error> {
    let viewer = viewer()?;
    if viewer != user_id && !moqui_core::user::has_permission(OWNER_PERMISSION)? {
        return Err(fail(format!(
            "{OWNER_PERMISSION} is required for another user's report schedules"
        )));
    }
    Ok(viewer)
}
fn owned_schedules(ec: &ExecutionContext, facade: &EntityFacade, user_id: &str) -> Result<Vec<EntityValue>, Error> {
    let _authz = ec.disable_authz();
    Ok(facade
        .find(SCHEDULE)?
        .use_cache(false)
        .eq("reportOwnerId", Value::string(user_id))
        .order_by("screenScheduledId", false)
        .list()?
        .iter()
        .filter(|row| !field(row, "reportDefinitionId").is_empty())
        .cloned()
        .collect())
}

/// `find#OwnedSchedules`: names only.
fn find_owned_schedules(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user_id = text(p, "userId");
    owner_authority(user_id)?;
    let facade = entity(ec)?;
    let mut resources = Vec::new();
    for row in owned_schedules(ec, &facade, user_id)? {
        let definition_id = field(&row, "reportDefinitionId");
        let title = definitions::read_unchecked(ec, definition_id)
            .ok()
            .flatten()
            .and_then(|definition| definition["title"].as_str().map(str::to_owned))
            .unwrap_or_else(|| definition_id.to_owned());
        let subject = field(&row, "emailSubject");
        let note = match (field(&row, "paused") == "Y", field(&row, "pausedReason")) {
            (true, "owner-disabled") => "Paused until a new owner takes it",
            (true, "transfer-pending") => "Paused until its new owner resumes it",
            (true, _) => "Paused",
            _ => match field(&row, "deliveryMode") {
                "group" => "Delivers to a group",
                _ => "Delivers to its owner",
            },
        };
        resources.push(json!({
            "resourceId": field(&row, "screenScheduledId"), "resourceKind": "schedule", "transferable": true, "note": note,
            "resourceName": if subject.is_empty() { format!("Schedule of {title}") } else { subject.to_owned() },
        }));
    }
    Ok(out(json!({"resourceList": resources})))
}

/// `pause#OwnedSchedules`.
fn pause_owned_schedules(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user_id = text(p, "userId");
    let actor = owner_authority(user_id)?;
    let facade = entity(ec)?;
    let mut paused = 0;
    for mut row in owned_schedules(ec, &facade, user_id)? {
        if field(&row, "paused") == "Y" && !field(&row, "pausedReason").is_empty() {
            continue;
        }
        // one schedule that cannot be paused is named and the others are still paused
        let _authz = ec.disable_authz();
        row.set("paused", Value::string("Y"));
        row.set("pausedReason", Value::string("owner-disabled"));
        row.set("versionNumber", Value::I64(revision(&row) + 1));
        match row.update(&facade) {
            Ok(()) => paused += 1,
            Err(error) => {
                tracing::error!(schedule = %field(&row, "screenScheduledId"), %error, "could not pause an owned report schedule")
            }
        }
    }
    tracing::info!(target: "moqui.report.audit", user = %actor, owner = %user_id, paused, reason = %text(p, "reason"),
        "owned report schedules paused");
    Ok(out(json!({"pausedCount": paused})))
}

/// `transfer#DefinitionOwner`.
fn transfer_definition_owner(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let (from, to, id) = (text(p, "fromUserId"), text(p, "toUserId"), text(p, "screenScheduledId"));
    let actor = owner_authority(from)?;
    let facade = entity(ec)?;
    let now = moqui_core::user::now_millis();
    let mut schedule = owned_schedules(ec, &facade, from)?
        .into_iter()
        .find(|row| field(row, "screenScheduledId") == id)
        .ok_or_else(|| fail("report schedule not found for this owner"))?;
    let new_owner = account(ec, &facade, to, now)?
        .filter(|account| account.active && !moqui_core::user::is_external_user(&account.user_id))
        .ok_or_else(|| fail("NEW_OWNER_REFUSED: the new owner is not an active internal user"))?;
    if from == to {
        return Err(fail("NEW_OWNER_REFUSED: the new owner already owns the schedule"));
    }
    if !holds_as(ec, &new_owner.username, STORE_SERVICE, "all") {
        return Err(fail("NEW_OWNER_REFUSED: the new owner may not schedule reports"));
    }
    if field(&schedule, "deliveryMode") == "group" && !holds_as(ec, &new_owner.username, GROUP_ARTIFACT, "create") {
        return Err(fail(
            "NEW_OWNER_REFUSED: the new owner may not deliver reports to other users",
        ));
    }
    let definition_id = field(&schedule, "reportDefinitionId").to_owned();
    // the definition and what it references that the leaving owner owns go with the schedule, so the new owner reads them
    let mut moved = Vec::new();
    {
        let _authz = ec.disable_authz();
        let mut queue = vec![definition_id.clone()];
        let mut seen = BTreeSet::new();
        while let Some(next) = queue.pop() {
            if seen.len() >= 500 || !seen.insert(next.clone()) {
                continue;
            }
            if let Some(mut row) = facade
                .find(DEFINITION)?
                .eq("definitionId", Value::string(&next))
                .one()?
            {
                // a telemetry alias the leaving owner does not hold the index of stays theirs (alias.rs)
                let owned = field(&row, "ownerId") == from && field(&row, "readOnly") != "Y";
                let goes = owned
                    && definitions::row_json(&row).is_ok_and(|definition| crate::alias::hands_over(ec, &definition));
                if goes {
                    row.set("ownerId", Value::string(to));
                    row.update(&facade)?;
                    moved.push(next.clone());
                }
            }
            for reference in facade
                .find(REFERENCE)?
                .eq("definitionId", Value::string(&next))
                .list()?
                .iter()
            {
                queue.push(field(reference, "referencedId").to_owned());
            }
        }
    }
    // qualified: the new owner reads the definition and every source it names (an error rolls the moves back)
    let scope = scope_as(ec, &new_owner.username, &definition_id)?;
    if scope.denied {
        return Err(fail(
            "NEW_OWNER_REFUSED: the new owner may not read this report or one of its sources",
        ));
    }
    let _authz = ec.disable_authz();
    schedule.set("reportOwnerId", Value::string(to));
    if field(&schedule, "userId") == from {
        schedule.set("userId", Value::string(to));
    }
    // paused until the new owner resumes it, which checks the recipients against the new owner's scope
    schedule.set("paused", Value::string("Y"));
    schedule.set("pausedReason", Value::string("transfer-pending"));
    schedule.set("versionNumber", Value::I64(revision(&schedule) + 1));
    schedule.update(&facade)?;
    tracing::info!(target: "moqui.report.audit", user = %actor, schedule = %id, from = %from, to = %to,
        definitions = moved.len(), reason = %text(p, "reason"), "report schedule transferred to a new owner");
    Ok(out(json!({"transferred": true, "definitionIds": moved})))
}

pub fn register(registry: &mut NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("check_audience", check_audience as moqui_core::native::NativeService),
        ("store_schedule", store_schedule),
        ("find_occurrences", find_occurrences),
        ("get_occurrence", get_occurrence),
        ("get_recipient_report", get_recipient_report),
        ("retry_recipients", retry_recipients),
        ("reconcile_recipient", reconcile_recipient),
        ("find_owned_schedules", find_owned_schedules),
        ("pause_owned_schedules", pause_owned_schedules),
        ("transfer_definition_owner", transfer_definition_owner),
    ] {
        registry.service_function(&format!("moqui_core::report_delivery::{name}"), function)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_filter_reads_the_same_whatever_order_its_values_came_in() {
        let filter = |values: &[&str]| {
            Query::all(vec![
                Query::is_in(
                    "vendorPartyId",
                    values.iter().map(|value| Value::string(*value)).collect(),
                ),
                Query::eq("statusId", Value::string("Open")),
            ])
        };
        assert_eq!(canonical(&filter(&["A", "B"])), canonical(&filter(&["B", "A", "B"])));
        assert_ne!(canonical(&filter(&["A", "B"])), canonical(&filter(&["A"])));
        assert_ne!(
            canonical(&Query::eq("vendorPartyId", Value::string("A"))),
            canonical(&Query::eq("organizationPartyId", Value::string("A")))
        );
    }

    #[test]
    fn a_definition_walk_names_each_dataset_and_follows_references_once() {
        let definitions = BTreeMap::from([
            (
                "report",
                json!({"type":"report","body":{"dashboards":[{"dashboardRef":"board"},{"definitionId":"board"}]}}),
            ),
            (
                "board",
                json!({"type":"dashboard","body":{"panels":{"a":{"panelRef":"panel"},
                "b":{"queries":[{"datasetId":"sales"},{"queries":[{"datasetId":"stock"}]}]},"c":{"panelRef":"private"}}}}),
            ),
            (
                "panel",
                json!({"type":"panel","body":{"queries":[{"datasetId":"sales"}],"links":[{"definitionId":"report"}]}}),
            ),
        ]);
        let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        walk(
            "report",
            &|id| definitions.get(id).cloned(),
            &mut seen,
            &mut datasets,
            &mut unread,
        );
        assert_eq!(datasets.into_iter().collect::<Vec<_>>(), ["sales", "stock"]);
        assert_eq!(
            seen.into_iter().collect::<Vec<_>>(),
            ["board", "panel", "private", "report"]
        );
        // a library panel the reader may not read is named, so their fingerprint differs from one who reads it
        assert_eq!(unread.into_iter().collect::<Vec<_>>(), ["private"]);
    }

    #[test]
    fn a_query_that_names_no_dataset_counts_the_one_it_reads_and_a_walk_past_its_bound_is_unread() {
        let board = json!({"type":"dashboard","body":{
            "variables":[{"kind":"query","field":"service.name"},{"kind":"custom","options":["a"]}],
            "panels":{"a":{"queries":[{"aggregations":[]},{"mode":"documents"},{"datasetId":"sales"},
                {"queries":[{"datasetId":"stock"},{"mode":"flame"}]}]}}}});
        let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        walk(
            "board",
            &|id| (id == "board").then(|| board.clone()),
            &mut seen,
            &mut datasets,
            &mut unread,
        );
        assert_eq!(
            datasets.into_iter().collect::<Vec<_>>(),
            ["otel_metrics", "otel_spans", "sales", "stock"]
        );
        assert!(unread.is_empty());
        // a chain of references longer than the walk follows: the rest is not known to be read
        let chain = |id: &str| {
            let next = id.parse::<usize>().ok()? + 1;
            Some(json!({"type":"dashboard","body":{"panels":{"a":{"panelRef":next.to_string()}}}}))
        };
        let (mut seen, mut datasets, mut unread) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        walk("0", &chain, &mut seen, &mut datasets, &mut unread);
        assert_eq!(seen.len(), WALK_LIMIT);
        assert_eq!(unread.into_iter().collect::<Vec<_>>(), [BEYOND]);
    }

    #[test]
    fn a_dataset_whose_rows_depend_on_the_viewer_holds_its_schedule_to_the_check() {
        assert!(viewer_scoped(&json!({"signal":"business","documentType":"Sales"})));
        // a business events dataset over the annotations: no signal, scoped by the viewer's organizations
        assert!(viewer_scoped(
            &json!({"indexName":"monitor_annotations","rowFilterService":"a.b.get#Filter"})
        ));
        assert!(viewer_scoped(
            &json!({"documentType":"Mine","contextFilters":[{"field":"userId","contextKey":"userId"}]})
        ));
        // a technical dataset: the same rows for every reader, as before
        assert!(!viewer_scoped(
            &json!({"indexName":"otel_logs","rowFilter":{"field":{"name":"level","op":"eq","value":"error"}},
            "contextFilters":[]})
        ));
    }

    #[test]
    fn a_save_is_refused_for_a_scope_that_differs_and_a_named_user_who_cannot_be_sent_to() {
        let verdict = |reason: &'static str, explicit: bool| Verdict {
            recipient: Recipient {
                account: Account {
                    user_id: "U".into(),
                    username: "u".into(),
                    name: "U".into(),
                    address: None,
                    locale: None,
                    active: true,
                },
                via: Vec::new(),
                explicit,
            },
            reason,
        };
        assert!(!verdict("", true).blocking());
        assert!(verdict("scope-differs", false).blocking());
        assert!(verdict("not-comparable", false).blocking());
        assert!(verdict("no-address", true).blocking());
        // a group's member without an address is skipped at each run, not a reason to refuse the group
        assert!(!verdict("no-address", false).blocking());
        assert!(!verdict("inactive", false).blocking());
    }
}
