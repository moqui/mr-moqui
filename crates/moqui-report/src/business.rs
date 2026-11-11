//! Business datasets: their admission, scope policy, fields, conditions and one-feed index (BOSS impl-insights P3, INS-03;
//! design-insights §5 Datasets, DatasetDetail, DatasetFields; docs/tools/Monitoring.md "Business datasets").
//!
//! A business dataset is a DataDocument dataset whose definition says `signal: "business"`. It is read only once admitted
//! (a `moqui.report.BusinessDataset` row with an admitted date and no retired one), except by its steward, who holds the
//! dataset's update artifact (`org.moqui.report.dataset.<id>`). Admission needs three things: the document in a registered
//! DataFeed, the document's index built, and a `rowFilterService` registered as a `ReportScopePolicy` enumeration (its
//! `enumCode` the service). The row filter is called with the dataset's id and its `rowFilterParameters` beside the viewer
//! (lib.rs), so it narrows rows, suggestions, value counts, aggregates and exports alike.
use super::{
    definitions, fail, input, json, metadata, output, search, text, Error, ExecutionContext, Json, Parameters,
};
use super::{ServiceInfo, Value};
use std::collections::{BTreeMap, BTreeSet};

const ADMISSION: &str = "moqui.report.BusinessDataset";
const POLICY_TYPE: &str = "ReportScopePolicy";
const DOCUMENT: &str = "moqui.entity.document.DataDocument";
const FIELD: &str = "moqui.entity.document.DataDocumentField";
const CONDITION: &str = "moqui.entity.document.DataDocumentCondition";
const FEED_DOCUMENT: &str = "moqui.entity.feed.DataFeedDocument";
const INDEX_SERVICE: &str = "org.moqui.search.SearchServices.index#DataFeedDocuments";
/// The condition operators a steward may use; the others (a post-query expression) stay Tools'.
const OPERATORS: &[&str] = &[
    "equals",
    "not-equals",
    "less",
    "greater",
    "less-equals",
    "greater-equals",
    "in",
    "not-in",
    "between",
    "not-between",
    "like",
    "not-like",
    "is-null",
    "is-not-null",
];
const FUNCTIONS: &[&str] = &["min", "max", "sum", "avg", "count", "count-distinct", "upper", "lower"];

/// A business dataset is one whose definition says `signal: "business"`, whatever else its body names: a body that says so
/// without a document is still held to admission (and is never admissible), so the signal alone cannot be dropped to escape it.
pub(super) fn is_business(body: &Json) -> bool {
    body["signal"] == "business"
}

fn entity(ec: &ExecutionContext) -> Result<moqui_core::entity::EntityFacade, Error> {
    ec.factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))
}
fn viewer() -> Result<String, Error> {
    moqui_core::user::current_user_id()
        .map_err(fail)?
        .filter(|user| !user.is_empty())
        .ok_or_else(|| fail("business datasets require an authenticated viewer"))
}
/// The dataset's update artifact, which its steward holds.
fn steward_of(id: &str) -> Result<(), Error> {
    moqui_core::user::authorize_own("AT_SERVICE", &format!("org.moqui.report.dataset.{id}"), "update").map_err(fail)
}
fn admission_row(ec: &ExecutionContext, id: &str) -> Result<Option<moqui_core::entity::EntityValue>, Error> {
    metadata(ec, || {
        entity(ec)?.find(ADMISSION)?.eq("datasetId", Value::string(id)).one()
    })
}
fn admitted(row: Option<&moqui_core::entity::EntityValue>) -> bool {
    row.is_some_and(|row| {
        row.get("admittedDate").is_some_and(|value| !value.is_null())
            && row.get("retiredDate").is_none_or(Value::is_null)
    })
}
fn millis(row: Option<&moqui_core::entity::EntityValue>, field: &str) -> Json {
    row.and_then(|row| row.get(field))
        .and_then(moqui_core::user::value_millis)
        .map_or(Json::Null, Json::from)
}
fn row_text(row: Option<&moqui_core::entity::EntityValue>, field: &str) -> Json {
    row.and_then(|row| row.get(field))
        .and_then(Value::as_str)
        .map_or(Json::Null, Json::from)
}

/// Whether a business dataset's fields changed after its index was last built, so what it answers is not what its definition
/// says (the admission check `index`): an alert over it has no valid reading until the index is rebuilt (alert_delivery.rs).
pub(super) fn index_stale(ec: &ExecutionContext, id: &str) -> bool {
    let Ok(row) = admission_row(ec, id) else {
        return false;
    };
    millis(row.as_ref(), "fieldsChangedDate")
        .as_i64()
        .is_some_and(|changed| {
            millis(row.as_ref(), "lastIndexedDate")
                .as_i64()
                .is_none_or(|built| built < changed)
        })
}

/// The engine's check before a business dataset is read: admitted with a registered scope policy still bound (a shipped
/// definition can change under its admission with a component update), or read by its steward. A business dataset reads its
/// one document: an index or a service in its body would read past what was admitted, so either is refused to everyone.
pub(super) fn check_admitted(ec: &ExecutionContext, id: &str, body: &Json) -> Result<(), Error> {
    if !is_business(body) {
        return Ok(());
    }
    if body.get("indexName").is_some_and(|v| !v.is_null()) || body.get("serviceName").is_some_and(|v| !v.is_null()) {
        return Err(fail(format!(
            "business dataset {id} reads its document only, not an index or a service"
        )));
    }
    let bound = body["rowFilterService"]
        .as_str()
        .map_or(Ok(false), |service| registered_policy(ec, service))?;
    if bound && admitted(admission_row(ec, id)?.as_ref()) {
        return Ok(());
    }
    steward_of(id).map_err(|_| fail(format!("dataset {id} is not admitted")))
}

/// The documents business datasets read: a dataset that is not a business one (a DataDocument by its id, an `index:` alias, a
/// definition without the signal) over one of these reads it without the admission and the row filter, so the engine opens it
/// only to a holder of that dataset's update artifact, its steward or the system administrator (BOSS impl-insights F-P3-6).
pub(super) fn check_governed(ec: &ExecutionContext, id: &str, body: &Json, documents: &[String]) -> Result<(), Error> {
    if is_business(body) || documents.is_empty() {
        return Ok(());
    }
    let governed = definitions::business_documents(ec)?;
    let Some(document) = documents.iter().find(|document| governed.contains(*document)) else {
        return Ok(());
    };
    steward_of(id).map_err(|_| {
        fail(format!(
            "dataset {id} reads {document}, a business dataset's document, without its admission and row filter"
        ))
    })
}

/// A dataset definition saved through `save#Definition` (or copied, restored, imported or pulled): a business dataset whose body
/// changed, or one saved new over an id whose admission is left from a deleted definition, returns to review, so the source,
/// time field and scope that were admitted are the ones read. The steward's own operations revise through `store#BusinessDataset`,
/// which decides admission itself.
pub(super) fn definition_saved(
    ec: &ExecutionContext,
    id: &str,
    previous: Option<&Json>,
    body: &Json,
) -> Result<(), Error> {
    if previous.is_some_and(|previous| previous == body) {
        return Ok(());
    }
    let Some(mut row) = admission_row(ec, id)? else {
        return Ok(());
    };
    if row.get("admittedDate").is_none_or(Value::is_null) {
        return Ok(());
    }
    let version = row
        .get("versionNumber")
        .and_then(moqui_core::user::value_millis)
        .unwrap_or(0);
    row.set("admittedDate", Value::Null);
    row.set("admittedByUserId", Value::Null);
    row.set("versionNumber", Value::I64(version + 1));
    let facade = entity(ec)?;
    metadata(ec, || row.update(&facade))?;
    tracing::info!(target: "moqui.report.audit", dataset = id, "business dataset definition changed; returned to review");
    Ok(())
}
/// A deleted definition's admission goes with it, so a later definition of the same id starts in review.
pub(super) fn definition_deleted(ec: &ExecutionContext, id: &str) -> Result<(), Error> {
    let facade = entity(ec)?;
    metadata(ec, || {
        facade
            .find(ADMISSION)?
            .eq("datasetId", Value::string(id))
            .delete_all()
            .map(|_| ())
    })
}

/// The registered scope policies, by their service: `ReportScopePolicy` enumerations, seeded by the domain that owns each.
fn policies(ec: &ExecutionContext) -> Result<BTreeMap<String, (String, String)>, Error> {
    let rows = metadata(ec, || {
        entity(ec)?
            .find("moqui.basic.Enumeration")?
            .eq("enumTypeId", Value::string(POLICY_TYPE))
            .list()
    })?;
    let mut out = BTreeMap::new();
    for row in rows.iter() {
        let (Some(id), Some(service)) = (
            row.get("enumId").and_then(Value::as_str),
            row.get("enumCode")
                .and_then(Value::as_str)
                .filter(|code| !code.is_empty()),
        ) else {
            continue;
        };
        let description = row.get("description").and_then(Value::as_str).unwrap_or(id);
        out.insert(service.to_owned(), (id.to_owned(), description.to_owned()));
    }
    Ok(out)
}
/// Whether a row-filter service is a registered scope policy.
pub(super) fn registered_policy(ec: &ExecutionContext, service: &str) -> Result<bool, Error> {
    Ok(policies(ec)?.contains_key(service))
}
fn policy_service(ec: &ExecutionContext, policy_id: &str) -> Result<String, Error> {
    policies(ec)?
        .into_iter()
        .find(|(_, (id, _))| id == policy_id)
        .map(|(service, _)| service)
        .ok_or_else(|| fail(format!("scope policy {policy_id} is not registered")))
}

fn feeds_of(ec: &ExecutionContext, document: &str) -> Result<Vec<String>, Error> {
    let rows = metadata(ec, || {
        entity(ec)?
            .find(FEED_DOCUMENT)?
            .eq("dataDocumentId", Value::string(document))
            .list()
    })?;
    let mut feeds: Vec<String> = rows
        .iter()
        .filter_map(|row| row.get("dataFeedId").and_then(Value::as_str).map(str::to_owned))
        .collect();
    feeds.sort();
    Ok(feeds)
}
fn index_built(ec: &ExecutionContext, document: &str) -> Result<bool, Error> {
    let facade = search::current()?;
    metadata(ec, || facade.backend().index_exists(&*facade.mapping(document)?))
}
fn check(name: &str, passed: bool, message: String) -> Json {
    json!({"check": name, "passed": passed, "message": message})
}

/// A business dataset as the steward sees it: its source, scope policy, admission checks and state. Reads with
/// authorization off; the callers decide what the viewer may see.
fn describe(ec: &ExecutionContext, definition: &Json) -> Result<Json, Error> {
    let id = text(definition, "id", "");
    let body = &definition["body"];
    let document = text(body, "documentType", "");
    let row = admission_row(ec, id)?;
    let document_row = metadata(ec, || {
        entity(ec)?
            .find(DOCUMENT)?
            .eq("dataDocumentId", Value::string(document))
            .one()
    })?;
    let mut checks = Vec::new();
    let mut feed = Json::Null;
    let mut index_name = Json::Null;
    let mut available = true;
    if let Some(document_row) = &document_row {
        index_name = row_text(Some(document_row), "indexName");
        let feeds = feeds_of(ec, document)?;
        feed = feeds.first().map_or(Json::Null, |id| json!(id));
        checks.push(check(
            "feed",
            !feeds.is_empty(),
            if feeds.is_empty() {
                format!("document {document} is in no registered data feed")
            } else {
                format!("indexed by the {} feed", feeds.join(", "))
            },
        ));
        let built = match index_built(ec, document) {
            Ok(built) => built,
            Err(error) => {
                tracing::error!(dataset = id, %error, "business dataset index could not be checked");
                false
            }
        };
        let stale = built
            && millis(row.as_ref(), "fieldsChangedDate")
                .as_i64()
                .is_some_and(|changed| {
                    millis(row.as_ref(), "lastIndexedDate")
                        .as_i64()
                        .is_none_or(|built| built < changed)
                });
        checks.push(check(
            "index",
            built && !stale,
            if !built {
                format!("the index of {document} is not built")
            } else if stale {
                "its fields changed after the index was last built; rebuild it".to_owned()
            } else {
                format!("the index of {document} is built")
            },
        ));
    } else {
        available = false;
        checks.push(check("document", false, format!("document {document} does not exist")));
    }
    let service = text(body, "rowFilterService", "");
    let registered = policies(ec)?;
    let policy = registered.get(service);
    checks.push(check(
        "scope",
        policy.is_some(),
        match policy {
            Some((_, description)) => format!("rows are scoped by {description}"),
            None if service.is_empty() => "no row filter is bound".to_owned(),
            None => format!("row filter {service} is not a registered scope policy"),
        },
    ));
    let admissible = checks.iter().all(|check| check["passed"] == true);
    let status = if !available {
        "unavailable"
    } else if row
        .as_ref()
        .is_some_and(|row| row.get("retiredDate").is_some_and(|v| !v.is_null()))
    {
        "retired"
    } else if admissible && admitted(row.as_ref()) {
        "admitted"
    } else {
        "review"
    };
    Ok(json!({
        "datasetId": id,
        "title": definition["title"],
        "dataDocumentId": document,
        "documentName": document_row.as_ref().map_or(Json::Null, |row| row_text(Some(row), "documentName")),
        "indexName": index_name,
        "dataFeedId": feed,
        "timeField": text(body, "timeField", "_timestamp"),
        "rowFilterService": if service.is_empty() { Json::Null } else { json!(service) },
        "scopePolicyId": policy.map_or(Json::Null, |(id, _)| json!(id)),
        "scopePolicy": policy.map_or(Json::Null, |(_, description)| json!(description)),
        "rowFilterParameters": body.get("rowFilterParameters").cloned().unwrap_or(Json::Null),
        "component": definition["component"],
        "readOnly": definition["readOnly"] == true,
        "ownerId": definition["ownerId"],
        "status": status,
        "admissible": admissible,
        "checks": checks,
        "admittedDate": millis(row.as_ref(), "admittedDate"),
        "admittedByUserId": row_text(row.as_ref(), "admittedByUserId"),
        "retiredDate": millis(row.as_ref(), "retiredDate"),
        "lastIndexedDate": millis(row.as_ref(), "lastIndexedDate"),
        "lastIndexedCount": row.as_ref().and_then(|row| row.get("lastIndexedCount")).and_then(moqui_core::user::value_millis).map_or(Json::Null, Json::from),
        "fieldsChangedDate": millis(row.as_ref(), "fieldsChangedDate"),
        "stewardUserId": row_text(row.as_ref(), "stewardUserId"),
        "version": row.as_ref().and_then(|row| row.get("versionNumber")).and_then(moqui_core::user::value_millis).unwrap_or(0),
    }))
}

/// A business dataset's definition, refused when it is not one.
fn business_definition(ec: &ExecutionContext, id: &str) -> Result<Json, Error> {
    let definition = definitions::read(ec, id)?;
    if definition["type"] != "dataset" || !is_business(&definition["body"]) {
        return Err(fail(format!("{id} is not a business dataset")));
    }
    Ok(definition)
}

/// `find#BusinessDatasets`: the business datasets the viewer may read, each with its admission state, and for a steward
/// (the dataset-save artifact `org.moqui.report.dataset` update) also those waiting for review that they steward.
pub fn find_business_datasets(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    viewer()?;
    let only = params["datasetId"].as_str().filter(|id| !id.is_empty());
    let status = params["status"].as_str().filter(|status| !status.is_empty());
    let steward = moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset", "update").is_ok();
    let mut datasets = Vec::new();
    for definition in definitions::list(ec, Some("dataset"))? {
        let id = text(&definition, "id", "").to_owned();
        if only.is_some_and(|only| only != id) || !is_business(&definition["body"]) {
            continue;
        }
        let readable = super::resolve_dataset(ec, &id, false).is_ok();
        let stewards = steward && steward_of(&id).is_ok();
        if !readable && !stewards {
            continue;
        }
        // one dataset that cannot be described is named and the others go on
        let mut described = match describe(ec, &definition) {
            Ok(described) => described,
            Err(error) => {
                tracing::error!(dataset = %id, %error, "business dataset could not be described");
                json!({"datasetId": id, "title": definition["title"], "status": "unavailable", "checks": [], "admissible": false})
            }
        };
        if status.is_some_and(|status| described["status"] != status) {
            continue;
        }
        described["readable"] = json!(readable);
        described["stewarded"] = json!(stewards);
        datasets.push(described);
    }
    Ok(output(json!({"datasets": datasets})))
}

/// `validate#BusinessDataset`: the admission checks of one dataset, for its steward.
pub fn validate_business_dataset(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let id = text(&params, "datasetId", "");
    steward_of(id)?;
    let described = describe(ec, &business_definition(ec, id)?)?;
    Ok(output(json!({
        "checks": described["checks"],
        "admissible": described["admissible"],
        "status": described["status"],
        "dataset": described,
    })))
}

/// The admission record of a dataset, made when it has none, after the version read is checked.
fn admission_for_update(
    ec: &ExecutionContext,
    id: &str,
    expected: Option<i64>,
) -> Result<(moqui_core::entity::EntityValue, bool), Error> {
    let facade = entity(ec)?;
    let existing = metadata(ec, || {
        facade
            .find(ADMISSION)?
            .eq("datasetId", Value::string(id))
            .for_update(true)
            .one()
    })?;
    let version = existing
        .as_ref()
        .and_then(|row| row.get("versionNumber"))
        .and_then(moqui_core::user::value_millis)
        .unwrap_or(0);
    if expected.is_some_and(|expected| expected != version) {
        return Err(fail(format!(
            "dataset {id} changed since it was read; reload before saving"
        )));
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = metadata(ec, || facade.make_value(ADMISSION))?;
            row.set("datasetId", Value::string(id));
            row
        }
    };
    row.set("versionNumber", Value::I64(version + 1));
    Ok((row, creating))
}
fn save_admission(ec: &ExecutionContext, row: &moqui_core::entity::EntityValue, creating: bool) -> Result<(), Error> {
    let facade = entity(ec)?;
    metadata(ec, || {
        if creating {
            row.create(&facade)
        } else {
            row.update(&facade)
        }
    })?;
    Ok(())
}
fn expected_version(params: &Json) -> Option<i64> {
    params["expectedVersion"]
        .as_i64()
        .or_else(|| params["expectedVersion"].as_str().and_then(|text| text.parse().ok()))
}

/// The row-filter parameters a steward may set: names and values of plain text, so a policy reads a field name, never an
/// expression.
fn filter_parameters(value: &Json) -> Result<Json, Error> {
    let Some(map) = value.as_object() else {
        return Err(fail("rowFilterParameters must be a map"));
    };
    if map.len() > 16 {
        return Err(fail("rowFilterParameters holds at most 16 entries"));
    }
    for (key, value) in map {
        let plain = |text: &str| {
            !text.is_empty()
                && text.len() <= 128
                && text.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        };
        if !plain(key) || !value.as_str().is_some_and(plain) {
            return Err(fail(format!("rowFilterParameters {key} must be a name")));
        }
    }
    Ok(value.clone())
}

/// `create#BusinessDataset`: a steward's dataset over a DataDocument of a registered feed, with a registered scope policy,
/// saved as a dataset definition they own and waiting for review.
pub fn create_business_dataset(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let user = viewer()?;
    moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset", "update").map_err(fail)?;
    let id = text(&params, "datasetId", "");
    let document = text(&params, "dataDocumentId", "");
    if id.is_empty() || document.is_empty() {
        return Err(fail("a business dataset needs an id and a document"));
    }
    if search::telemetry::is_telemetry_index(document) {
        return Err(fail("a telemetry index is not a business source"));
    }
    if metadata(ec, || {
        entity(ec)?
            .find(DOCUMENT)?
            .eq("dataDocumentId", Value::string(document))
            .one()
    })?
    .is_none()
    {
        return Err(fail(format!("document {document} does not exist")));
    }
    // registered sources only (design §5 CreateDataset): a document no data feed indexes is not offered
    if feeds_of(ec, document)?.is_empty() {
        return Err(fail(format!("document {document} is in no registered data feed")));
    }
    let service = policy_service(ec, text(&params, "scopePolicyId", ""))?;
    let mut body = json!({
        "documentType": document,
        "timeField": text(&params, "timeField", "_timestamp"),
        "signal": "business",
        "rowFilterService": service,
    });
    if let Some(parameters) = params.get("rowFilterParameters").filter(|value| !value.is_null()) {
        body["rowFilterParameters"] = filter_parameters(parameters)?;
    }
    // a definition is shared only with a group of the steward's own (F42)
    let group = params["userGroupId"].as_str().filter(|group| !group.is_empty());
    if let Some(group) = group {
        if !moqui_core::user::is_in_group(group).map_err(fail)? {
            return Err(fail("share a dataset only with a group you belong to"));
        }
    }
    let definition =
        json!({"id": id, "type": "dataset", "title": text(&params, "title", id), "userGroupId": group, "body": body});
    definitions::save_owned(ec, &definition, "Business dataset created")?;
    let (mut row, creating) = admission_for_update(ec, id, None)?;
    row.set("stewardUserId", Value::string(&user));
    // a new dataset starts in review, whatever a row of the same id held
    for field in ["admittedDate", "admittedByUserId", "retiredDate"] {
        row.set(field, Value::Null);
    }
    save_admission(ec, &row, creating)?;
    let saved = definitions::read(ec, id)?;
    Ok(output(json!({"dataset": describe(ec, &saved)?})))
}

/// `store#BusinessDataset`: a steward's changes to a dataset: its title, time field, scope policy and parameters (a saved
/// dataset only; a shipped one's definition is its owner's), its steward, and admitting or retiring it. A change of the time
/// field or the scope returns it to review. Admission is refused while a check fails.
pub fn store_business_dataset(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let user = viewer()?;
    let id = text(&params, "datasetId", "");
    steward_of(id)?;
    let mut definition = business_definition(ec, id)?;
    let (mut row, creating) = admission_for_update(ec, id, expected_version(&params))?;
    let mut changed_scope = false;
    let mut body = definition["body"].clone();
    if let Some(value) = params["timeField"].as_str().filter(|value| !value.is_empty()) {
        if body["timeField"] != value {
            body["timeField"] = json!(value);
            changed_scope = true;
        }
    }
    if let Some(policy) = params["scopePolicyId"].as_str().filter(|value| !value.is_empty()) {
        let service = policy_service(ec, policy)?;
        if body["rowFilterService"] != service.as_str() {
            body["rowFilterService"] = json!(service);
            changed_scope = true;
        }
    }
    if let Some(parameters) = params.get("rowFilterParameters").filter(|value| !value.is_null()) {
        let parameters = filter_parameters(parameters)?;
        if body["rowFilterParameters"] != parameters {
            body["rowFilterParameters"] = parameters;
            changed_scope = true;
        }
    }
    let title = params["title"].as_str().filter(|title| !title.is_empty());
    if changed_scope || title.is_some_and(|title| definition["title"] != title) {
        if definition["readOnly"] == true {
            return Err(fail(format!(
                "dataset {id} is shipped by {}; its definition is changed there",
                text(&definition, "component", "a component")
            )));
        }
        definition["body"] = body;
        if let Some(title) = title {
            definition["title"] = json!(title);
        }
        definitions::save_owned(ec, &definition, "Business dataset revised")?;
    }
    if changed_scope {
        row.set("admittedDate", Value::Null);
        row.set("admittedByUserId", Value::Null);
    }
    if let Some(steward) = params["stewardUserId"].as_str().filter(|value| !value.is_empty()) {
        row.set("stewardUserId", Value::string(steward));
    }
    if params["retire"] == true || params["retire"] == "true" {
        row.set("retiredDate", moqui_core::user::now_value());
    } else if params["admit"] == true || params["admit"] == "true" {
        // the checks read the source, the feed, the index and the policy, none of which this call changes
        let described = describe(ec, &definitions::read(ec, id)?)?;
        if described["admissible"] != true {
            let reasons: Vec<&str> = described["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|check| check["passed"] != true)
                .filter_map(|check| check["message"].as_str())
                .collect();
            return Err(fail(format!("dataset {id} cannot be admitted: {}", reasons.join("; "))));
        }
        row.set("admittedDate", moqui_core::user::now_value());
        row.set("admittedByUserId", Value::string(&user));
        row.set("retiredDate", Value::Null);
    }
    save_admission(ec, &row, creating)?;
    Ok(output(json!({"dataset": describe(ec, &definitions::read(ec, id)?)?})))
}

/// `index#BusinessDataset`: rebuild one dataset's index, its document of its feed only, for its steward (the dataset's
/// update artifact), recording when and how many.
pub fn index_business_dataset(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let user = viewer()?;
    let id = text(&params, "datasetId", "");
    steward_of(id)?;
    let definition = business_definition(ec, id)?;
    let document = text(&definition["body"], "documentType", "").to_owned();
    let feed = feeds_of(ec, &document)?
        .into_iter()
        .next()
        .ok_or_else(|| fail(format!("document {document} is in no registered data feed")))?;
    let result = {
        // the steward's grant is the dataset's; the feed rebuild is the framework's own, for this document only
        let _system = ec.disable_authz();
        moqui_core::service::call::sync_checked(
            INDEX_SERVICE,
            &Parameters::from([
                ("dataFeedId".into(), Value::string(&feed)),
                ("dataDocumentId".into(), Value::string(&document)),
            ]),
            &moqui_core::service::call::CallOptions::default(),
        )
        .map_err(fail)?
    };
    let count = result
        .get("documentsIndexed")
        .and_then(moqui_core::user::value_millis)
        .unwrap_or(0);
    let (mut row, creating) = admission_for_update(ec, id, None)?;
    row.set("lastIndexedDate", moqui_core::user::now_value());
    row.set("lastIndexedCount", Value::I64(count));
    row.set("lastIndexedByUserId", Value::string(&user));
    save_admission(ec, &row, creating)?;
    Ok(output(
        json!({"documentsIndexed": count, "dataFeedId": feed, "dataDocumentId": document}),
    ))
}

/// A field's name in the document: its alias, or the last part of its path.
fn field_alias(row: &moqui_core::entity::EntityValue) -> String {
    let path = row.get("fieldPath").and_then(Value::as_str).unwrap_or("");
    row.get("fieldNameAlias")
        .and_then(Value::as_str)
        .filter(|alias| !alias.is_empty())
        .map_or_else(|| path.rsplit(':').next().unwrap_or(path).to_owned(), str::to_owned)
}
fn plain_name(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
/// A field path a steward may add: relationship and field names only, never an expression, and no secret field.
fn plain_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split(':').collect();
    parts.len() <= 6
        && parts.iter().all(|part| plain_name(part))
        && parts
            .last()
            .is_some_and(|field| !moqui_core::context::secret_parameter(field))
}

/// The definitions that read a dataset and name a field: any string in a body equal to the field's name. Every saved
/// definition is read, the viewer's or not, so a change cannot break a dashboard its steward does not see.
fn consumers_naming(ec: &ExecutionContext, id: &str, names: &BTreeSet<String>) -> Result<Vec<String>, Error> {
    fn names_in(value: &Json, names: &BTreeSet<String>, found: &mut BTreeSet<String>) {
        match value {
            Json::String(text) if names.contains(text) => {
                found.insert(text.clone());
            }
            Json::Array(items) => items.iter().for_each(|item| names_in(item, names, found)),
            Json::Object(map) => map.values().for_each(|item| names_in(item, names, found)),
            _ => {}
        }
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    // the business datasets over the same document name their time field and the field their row filter reads: removing or
    // renaming either would read the wrong rows, or none, once the index is rebuilt
    let document = definitions::read(ec, id)?["body"]["documentType"].clone();
    let sharing = definitions::business_datasets(ec)?
        .into_iter()
        .filter(|(_, body)| body["documentType"] == document);
    for (definition_id, body) in sharing.chain(definitions::all_referencing(ec, id)?) {
        let mut found = BTreeSet::new();
        names_in(&body, names, &mut found);
        if !found.is_empty() {
            out.push(format!(
                "{definition_id} ({})",
                found.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    Ok(out)
}

/// Record a change of a dataset's fields or conditions: a new version, and the index stale until it is rebuilt.
fn fields_changed(ec: &ExecutionContext, id: &str, expected: Option<i64>) -> Result<(), Error> {
    let (mut row, creating) = admission_for_update(ec, id, expected)?;
    row.set("fieldsChangedDate", moqui_core::user::now_value());
    save_admission(ec, &row, creating)
}
/// The document's definition is read afresh and checked after a change, so a path that does not resolve is refused, and so is
/// an encrypted entity field anywhere in it: the index would hold it decrypted for every reader of the dataset.
fn recompile(ec: &ExecutionContext, document: &str) -> Result<(), Error> {
    let _metadata = ec.disable_authz();
    let facade = search::current()?;
    facade.clear_definitions();
    facade
        .mapping(document)
        .map_err(|error| fail(format!("document {document} does not resolve: {error}")))?;
    let entities = entity(ec)?;
    let info = moqui_core::entity::document::document_info(&entities, document)
        .map_err(|error| fail(format!("document {document} does not resolve: {error}")))?;
    let mut encrypted = Vec::new();
    let mut failure = None;
    info.walk(|_, node| {
        for field in node.fields.iter().filter(|field| !field.is_expression) {
            match entities.with_entity(&node.entity, |definition, _, _| {
                Ok(definition.field(&field.path_element).is_some_and(|field| field.encrypt))
            }) {
                Ok(true) => encrypted.push(format!("{} ({})", field.alias, node.entity)),
                Ok(false) => {}
                Err(error) => failure = Some(error),
            }
        }
    });
    if failure.is_some() || !encrypted.is_empty() {
        // the refused change rolls back with the service's transaction; the definition read from it must not stay cached
        facade.clear_definitions();
    }
    if let Some(error) = failure {
        return Err(fail(format!("document {document} does not resolve: {error}")));
    }
    if !encrypted.is_empty() {
        return Err(fail(format!(
            "an encrypted field is not a dataset field: {}",
            encrypted.join(", ")
        )));
    }
    Ok(())
}
fn next_seq(rows: &[moqui_core::entity::EntityValue], field: &str) -> String {
    let highest = rows
        .iter()
        .filter_map(|row| row.get(field).and_then(Value::as_str))
        .filter_map(|id| id.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{:02}", highest + 1)
}

/// `store#BusinessDatasetFields`: a steward adds, revises and removes the dataset's document fields. A new field is a
/// plain path (no expression, no secret field); a function is one of the aggregate or case functions. Removing or renaming a
/// field a definition over the dataset names is refused until that definition is revised, and so is a stale version.
pub fn store_business_dataset_fields(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    viewer()?;
    let id = text(&params, "datasetId", "");
    steward_of(id)?;
    let definition = business_definition(ec, id)?;
    let document = text(&definition["body"], "documentType", "").to_owned();
    let facade = entity(ec)?;
    let existing: Vec<_> = metadata(ec, || {
        facade
            .find(FIELD)?
            .eq("dataDocumentId", Value::string(&document))
            .list()
    })?
    .iter()
    .cloned()
    .collect();
    // each change: the row to write, or to delete (None); a new row has no fieldSeqId yet
    let mut pending: Vec<(moqui_core::entity::EntityValue, Option<bool>)> = Vec::new();
    for change in params["fields"].as_array().into_iter().flatten() {
        let seq = change["fieldSeqId"].as_str().filter(|seq| !seq.is_empty());
        let current = match seq {
            Some(seq) => Some(
                existing
                    .iter()
                    .find(|row| row.get("fieldSeqId").and_then(Value::as_str) == Some(seq))
                    .ok_or_else(|| fail(format!("field {seq} is not in {document}")))?,
            ),
            None => None,
        };
        if change["remove"] == true || change["remove"] == "true" {
            if let Some(current) = current {
                pending.push((current.clone(), None));
            }
            continue;
        }
        let old_path = current.and_then(|row| row.get("fieldPath")).and_then(Value::as_str);
        let path = match (change["fieldPath"].as_str().filter(|path| !path.is_empty()), old_path) {
            (Some(path), Some(old)) if path == old => old.to_owned(),
            (Some(path), _) if plain_path(path) => path.to_owned(),
            (Some(path), _) => return Err(fail(format!("field path {path} is not a plain field path"))),
            (None, Some(old)) => old.to_owned(),
            (None, None) => return Err(fail("a new field needs a field path")),
        };
        let alias = change["fieldNameAlias"].as_str().unwrap_or("");
        if !alias.is_empty() && !plain_name(alias) {
            return Err(fail(format!("field name {alias} must be letters, digits or _")));
        }
        let function = change["functionName"].as_str().unwrap_or("");
        if !function.is_empty() && !FUNCTIONS.contains(&function) {
            return Err(fail(format!("function {function} is not one a dataset field may use")));
        }
        let mut row = match current {
            Some(row) => row.clone(),
            None => {
                let mut row = metadata(ec, || facade.make_value(FIELD))?;
                row.set("dataDocumentId", Value::string(&document));
                row
            }
        };
        row.set("fieldPath", Value::string(&path));
        row.set(
            "fieldNameAlias",
            if alias.is_empty() {
                Value::Null
            } else {
                Value::string(alias)
            },
        );
        row.set(
            "functionName",
            if function.is_empty() {
                Value::Null
            } else {
                Value::string(function)
            },
        );
        let sequence = change["sequenceNum"]
            .as_i64()
            .or_else(|| change["sequenceNum"].as_str().and_then(|text| text.parse().ok()));
        if let Some(sequence) = sequence {
            row.set("sequenceNum", Value::I64(sequence));
        }
        pending.push((row, Some(current.is_none())));
    }
    // the field names after the change, which must be unique, and the names a change takes away
    let changed: BTreeSet<&str> = pending
        .iter()
        .filter_map(|(row, _)| row.get("fieldSeqId").and_then(Value::as_str))
        .collect();
    let mut after = BTreeSet::new();
    for row in existing.iter().filter(|row| {
        row.get("fieldSeqId")
            .and_then(Value::as_str)
            .is_none_or(|seq| !changed.contains(seq))
    }) {
        after.insert(field_alias(row));
    }
    for (row, keep) in &pending {
        if keep.is_some() && !after.insert(field_alias(row)) {
            return Err(fail(format!("field name {} is used twice", field_alias(row))));
        }
    }
    let removed: BTreeSet<String> = existing
        .iter()
        .map(field_alias)
        .filter(|name| !after.contains(name))
        .collect();
    let blockers = consumers_naming(ec, id, &removed)?;
    if !blockers.is_empty() {
        return Err(fail(format!(
            "these definitions use the fields removed or renamed; revise them first: {}",
            blockers.join("; ")
        )));
    }
    fields_changed(ec, id, expected_version(&params))?;
    let mut all = existing.clone();
    metadata(ec, || {
        for (row, keep) in &mut pending {
            match keep {
                None => row.delete(&facade)?,
                Some(true) => {
                    row.set("fieldSeqId", Value::string(next_seq(&all, "fieldSeqId")));
                    row.create(&facade)?;
                    all.push(row.clone());
                }
                Some(false) => row.update(&facade)?,
            }
        }
        Ok(())
    })?;
    recompile(ec, &document)?;
    let saved = definitions::read(ec, id)?;
    Ok(output(json!({"dataset": describe(ec, &saved)?})))
}

/// `store#BusinessDatasetConditions`: a steward adds, revises and removes the conditions that narrow the dataset's
/// document. A condition names a field of the document and an operator of the list; it narrows a source and never replaces
/// its row filter.
pub fn store_business_dataset_conditions(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    viewer()?;
    let id = text(&params, "datasetId", "");
    steward_of(id)?;
    let definition = business_definition(ec, id)?;
    let document = text(&definition["body"], "documentType", "").to_owned();
    let facade = entity(ec)?;
    let names: BTreeSet<String> = metadata(ec, || {
        facade
            .find(FIELD)?
            .eq("dataDocumentId", Value::string(&document))
            .list()
    })?
    .iter()
    .map(field_alias)
    .collect();
    let existing: Vec<_> = metadata(ec, || {
        facade
            .find(CONDITION)?
            .eq("dataDocumentId", Value::string(&document))
            .list()
    })?
    .iter()
    .cloned()
    .collect();
    let mut pending = Vec::new();
    for change in params["conditions"].as_array().into_iter().flatten() {
        let seq = change["conditionSeqId"].as_str().filter(|seq| !seq.is_empty());
        let current = seq.and_then(|seq| {
            existing
                .iter()
                .find(|row| row.get("conditionSeqId").and_then(Value::as_str) == Some(seq))
        });
        if seq.is_some() && current.is_none() {
            return Err(fail(format!(
                "condition {} is not in {document}",
                seq.unwrap_or_default()
            )));
        }
        if change["remove"] == true || change["remove"] == "true" {
            if let Some(current) = current {
                pending.push((current.clone(), None));
            }
            continue;
        }
        let alias = change["fieldNameAlias"].as_str().unwrap_or("");
        if !names.contains(alias) {
            return Err(fail(format!("condition field {alias} is not a field of {document}")));
        }
        let operator = change["operator"]
            .as_str()
            .filter(|op| !op.is_empty())
            .unwrap_or("equals");
        if !OPERATORS.contains(&operator) {
            return Err(fail(format!("operator {operator} is not one a condition may use")));
        }
        let to_alias = change["toFieldNameAlias"].as_str().unwrap_or("");
        if !to_alias.is_empty() && !names.contains(to_alias) {
            return Err(fail(format!("condition field {to_alias} is not a field of {document}")));
        }
        let value = change["fieldValue"].as_str().unwrap_or("");
        if value.len() > 255 {
            return Err(fail("a condition value has at most 255 characters"));
        }
        let mut row = match current {
            Some(row) => row.clone(),
            None => {
                let mut row = metadata(ec, || facade.make_value(CONDITION))?;
                row.set("dataDocumentId", Value::string(&document));
                row
            }
        };
        row.set("fieldNameAlias", Value::string(alias));
        row.set("operator", Value::string(operator));
        row.set(
            "fieldValue",
            if value.is_empty() {
                Value::Null
            } else {
                Value::string(value)
            },
        );
        row.set(
            "toFieldNameAlias",
            if to_alias.is_empty() {
                Value::Null
            } else {
                Value::string(to_alias)
            },
        );
        // a post-query expression stays Tools'
        row.set("postQuery", Value::string("N"));
        pending.push((row, Some(current.is_none())));
    }
    fields_changed(ec, id, expected_version(&params))?;
    let mut all = existing.clone();
    metadata(ec, || {
        for (row, keep) in &mut pending {
            match keep {
                None => row.delete(&facade)?,
                Some(true) => {
                    row.set("conditionSeqId", Value::string(next_seq(&all, "conditionSeqId")));
                    row.create(&facade)?;
                    all.push(row.clone());
                }
                Some(false) => row.update(&facade)?,
            }
        }
        Ok(())
    })?;
    recompile(ec, &document)?;
    let saved = definitions::read(ec, id)?;
    Ok(output(json!({"dataset": describe(ec, &saved)?})))
}
