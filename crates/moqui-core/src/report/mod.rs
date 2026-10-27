//! Authorized report queries and shared columnar results. No telemetry SDK types cross this API.

pub mod alert;
pub mod annotations;
pub mod definitions;
pub mod frame;
pub mod time;

use crate::{
    context::ExecutionContext,
    native::{Parameters, ServiceInfo},
    search::{self, Query, SearchRequest, SortField, Target},
};
use indexmap::IndexMap;
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

fn fail(error: impl std::fmt::Display) -> Error {
    Error::new(error.to_string())
}
fn metadata<T>(ec: &ExecutionContext, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _guard = ec.disable_authz();
    body()
}
fn text<'a>(value: &'a Json, name: &str, default: &'a str) -> &'a str {
    value.get(name).and_then(Json::as_str).unwrap_or(default)
}
fn input(parameters: &Parameters) -> Json {
    Json::Object(
        parameters
            .iter()
            .map(|(k, v)| (k.clone(), moqui_lib::json::to_json(v)))
            .collect(),
    )
}
fn output(mut value: Json) -> Parameters {
    fn script_numbers(value: &mut Json) {
        match value {
            Json::Number(number) => {
                if number
                    .as_i64()
                    .is_some_and(|n| !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&n))
                    || number.as_u64().is_some_and(|n| n > 9_007_199_254_740_991)
                {
                    *value = Json::String(number.to_string());
                }
            }
            Json::Array(items) => {
                for value in items {
                    script_numbers(value)
                }
            }
            Json::Object(map) => {
                for value in map.values_mut() {
                    script_numbers(value)
                }
            }
            _ => {}
        }
    }
    script_numbers(&mut value);
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

/// An authorized dataset, resolved afresh for each execution so permission changes take effect.
pub struct Dataset {
    pub id: String,
    pub target: Target,
    pub time_field: String,
    pub filter: Query,
    pub service: Option<String>,
    pub telemetry: bool,
}

fn authorize_dataset(id: &str) -> Result<(), Error> {
    // A background caller must enter an owner context as ordinary service calls do.
    if !crate::user::context_entered() {
        return Err(fail("a report requires an entered viewer context"));
    }
    crate::user::authorize_own("AT_SERVICE", &format!("org.moqui.report.dataset.{id}"), "view").map_err(fail)
}

pub fn dataset(ec: &ExecutionContext, id: &str) -> Result<Dataset, Error> {
    authorize_dataset(id)?;
    let time = match id {
        "otel_spans" => Some("start"),
        "otel_logs" => Some("timestamp"),
        "otel_metrics" => Some("bucket_start"),
        "otel_errors" => Some("last_seen"),
        "monitor_annotations" => Some("time"),
        _ => None,
    };
    if let Some(time) = time {
        return Ok(Dataset {
            id: id.into(),
            target: Target::Index(id.into()),
            time_field: time.into(),
            filter: Query::MatchAll,
            service: None,
            telemetry: true,
        });
    }
    let definition = if let Some(index) = id.strip_prefix("index:") {
        json!({"type":"dataset","body":{"indexName":index,"timeField":"_timestamp"}})
    } else {
        match definitions::read(ec, id) {
            Ok(d) => d,
            Err(error) => {
                if !error.message().contains("does not exist") {
                    return Err(error);
                }
                let entity = ec
                    .factory()
                    .entity()
                    .ok_or_else(|| fail("entity facade is unavailable"))?;
                if metadata(ec, || {
                    entity
                        .find("moqui.entity.document.DataDocument")?
                        .eq("dataDocumentId", Value::string(id))
                        .one()
                })?
                .is_none()
                {
                    return Err(error);
                }
                json!({"type":"dataset","body":{"documentType":id,"timeField":"_timestamp"}})
            }
        }
    };
    if text(&definition, "type", "") != "dataset" {
        return Err(fail(format!("{id} is not a dataset")));
    }
    let body = &definition["body"];
    if let Some(service) = body["serviceName"].as_str() {
        crate::user::authorize_own("AT_SERVICE", service, "view").map_err(fail)?;
        return Ok(Dataset {
            id: id.into(),
            target: Target::Documents(Vec::new()),
            time_field: String::new(),
            filter: Query::MatchAll,
            service: Some(service.into()),
            telemetry: false,
        });
    }
    let target = if let Some(index) = body["indexName"].as_str() {
        Target::Index(index.into())
    } else {
        Target::Documents(vec![text(body, "documentType", id).into()])
    };
    let facade = search::current()?;
    let groups = crate::user::user_group_ids(crate::user::current_user_id().map_err(fail)?.as_deref()).map_err(fail)?;
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let mut allowed = Vec::new();
    for document in metadata(ec, || facade.target_documents(&target))? {
        if search::telemetry::is_telemetry_index(&document) {
            if body.get("rowFilter").is_none()
                && body["contextFilters"]
                    .as_array()
                    .is_none_or(|filters| filters.is_empty())
                && body["rowFilterService"].as_str().is_none()
            {
                return Err(fail("a telemetry dataset alias requires a server-owned row filter"));
            }
            allowed.push(document);
            continue;
        }
        let access = metadata(ec, || {
            entity
                .find("moqui.entity.document.DataDocumentUserGroup")?
                .eq("dataDocumentId", Value::string(&document))
                .list()
        })?;
        if access.iter().any(|row| {
            row.get("userGroupId")
                .and_then(Value::as_str)
                .is_some_and(|group| groups.contains(group))
        }) {
            allowed.push(document);
        }
    }
    if allowed.is_empty() {
        return Err(fail(format!("viewer may not read dataset {id}")));
    }
    // The restricted target is used for search, aggregations, facets and document expansion alike.
    let mut predicates = Vec::new();
    if let Some(filter) = body.get("rowFilter") {
        predicates.push(Query::from_value(&moqui_lib::json::from_json(filter))?);
    }
    if let Some(filters) = body["contextFilters"].as_array() {
        for filter in filters {
            let field = text(filter, "field", "");
            let key = text(filter, "contextKey", "");
            let value = identity_filter_value(ec, key)?;
            predicates.push(if let Value::List(values) = value {
                Query::is_in(field, values)
            } else {
                Query::eq(field, value)
            });
        }
    }
    if let Some(service) = body["rowFilterService"].as_str() {
        let user = crate::user::current_user_id()
            .map_err(fail)?
            .filter(|user| !user.is_empty())
            .ok_or_else(|| fail("a row-filter service requires an authenticated viewer"))?;
        let result = crate::service::call::sync_checked(
            service,
            &Parameters::from([("userId".into(), Value::string(user))]),
            &crate::service::call::CallOptions {
                ignore_previous_error: true,
                no_remember_parameters: true,
                ..Default::default()
            },
        )
        .map_err(fail)?;
        let filter = result
            .get("filter")
            .filter(|value| !value.is_null())
            .ok_or_else(|| fail(format!("dataset {id} row-filter service did not return filter")))?;
        predicates.push(Query::from_value(filter)?);
    }
    let telemetry = allowed.iter().any(|id| search::telemetry::is_telemetry_index(id));
    let default_time = allowed
        .first()
        .map(String::as_str)
        .and_then(|id| match id {
            "otel_spans" => Some("start"),
            "otel_logs" => Some("timestamp"),
            "otel_metrics" => Some("bucket_start"),
            "otel_errors" => Some("last_seen"),
            "monitor_annotations" => Some("time"),
            _ => None,
        })
        .unwrap_or("_timestamp");
    let time_field = text(body, "timeField", default_time).to_owned();
    Ok(Dataset {
        id: id.into(),
        target: Target::Documents(allowed),
        time_field,
        filter: Query::all(predicates),
        service: None,
        telemetry,
    })
}

/// Scope values come from the authenticated principal, never screen parameters or the
/// request-populated user context. Application organization ACLs belong in rowFilterService.
fn identity_filter_value(ec: &ExecutionContext, key: &str) -> Result<Value, Error> {
    let user = crate::user::current_user_id()
        .map_err(fail)?
        .filter(|user| !user.is_empty())
        .ok_or_else(|| fail("dataset identity filters require an authenticated viewer"))?;
    match key{
        "userId"=>Ok(Value::string(user)),
        "username"=>crate::user::current_username().map_err(fail)?.map(Value::string).ok_or_else(||fail("authenticated username is unavailable")),
        "userGroupIds"=>Ok(Value::List(crate::user::user_group_ids(Some(&user)).map_err(fail)?.into_iter().map(Value::string).collect())),
        key if key.starts_with("userAccount.")=>{let field=&key[12..];if field.is_empty()||!field.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_')||crate::context::secret_parameter(field){return Err(fail("dataset userAccount filter field is not permitted"));}
            let entity=ec.factory().entity().ok_or_else(||fail("entity facade is unavailable"))?;let account=metadata(ec,||entity.find("moqui.security.UserAccount")?.eq("userId",Value::string(&user)).one())?.ok_or_else(||fail("authenticated user account is unavailable"))?;
            account.get(field).cloned().filter(|value|!value.is_null()).ok_or_else(||fail(format!("authenticated userAccount field {field} is unavailable")))
        }
        _=>Err(fail(format!("untrusted dataset contextKey {key}; use userId, username, userGroupIds, userAccount.<field>, or an administrator-defined rowFilterService"))),
    }
}

fn constraints(ec: &ExecutionContext, options: &Json, dataset: &Dataset, now: i64) -> Result<Query, Error> {
    let from = time::parse(text(options, "from", "now-7d"), now)?;
    let thru = time::parse(text(options, "thru", "now"), now)?;
    if from >= thru {
        return Err(fail("report time range must end after it starts"));
    }
    // Limits apply before reading the backend. No caller can remove the trusted dataset predicate.
    if thru.saturating_sub(from) > 3660_i64 * 86_400_000 {
        return Err(fail("report time range exceeds ten years"));
    }
    let stamp = |millis| Value::String(jiff::Timestamp::from_millisecond(millis).unwrap().to_string());
    let mut all = vec![
        dataset.filter.clone(),
        Query::range(&dataset.time_field, Some(stamp(from)), Some(stamp(thru))),
    ];
    let metric_source = match &dataset.target {
        Target::Index(index) => index == "otel_metrics",
        Target::Documents(documents) => documents.len() == 1 && documents[0] == "otel_metrics",
    };
    if metric_source && options["metricGranularity"] == "auto" {
        let minute_days = ec
            .factory()
            .conf_number::<i64>("telemetry", "minute-retention-days", 7)
            .max(1);
        let hour_days = ec
            .factory()
            .conf_number::<i64>("telemetry", "hour-retention-days", 90)
            .max(minute_days);
        let age = now.saturating_sub(from).max(thru.saturating_sub(from));
        let hour = now.div_euclid(3_600_000) * 3_600_000;
        let day = now.div_euclid(86_400_000) * 86_400_000;
        let branch = |granularity: &str, start: Option<i64>, end: Option<i64>| {
            Query::all(vec![
                Query::eq("granularity", Value::string(granularity)),
                Query::range(&dataset.time_field, start.map(stamp), end.map(stamp)),
            ])
        };
        if age <= minute_days.saturating_mul(86_400_000) {
            all.push(Query::eq("granularity", Value::string("minute")));
        } else if age <= hour_days.saturating_mul(86_400_000) {
            all.push(Query::any(vec![
                branch("hour", None, Some(hour)),
                branch("minute", Some(hour), None),
            ]));
        } else {
            all.push(Query::any(vec![
                branch("day", None, Some(day)),
                branch("hour", Some(day), Some(hour)),
                branch("minute", Some(hour), None),
            ]));
        }
    }
    if let Some(query) = options.get("query").filter(|v| !v.is_null()) {
        all.push(Query::from_value(&moqui_lib::json::from_json(query))?);
    }
    if let Some(query) = options["queryString"].as_str().filter(|s| !s.is_empty()) {
        all.push(search::parse::parse_query_string(query)?);
    }
    if let Some(filters) = options["filters"].as_array() {
        for filter in filters.iter().filter(|f| f["disabled"] != true) {
            let q = filter.get("query").unwrap_or(filter);
            let q = Query::from_value(&moqui_lib::json::from_json(q))?;
            all.push(if filter["negated"] == true {
                Query::Bool {
                    all: Vec::new(),
                    any: Vec::new(),
                    none: vec![q],
                    min_any: None,
                }
            } else {
                q
            });
        }
    }
    Ok(Query::all(all))
}

pub fn documents(ec: &ExecutionContext, options: &Json) -> Result<Json, Error> {
    search::limits::with_limits(Default::default(), || documents_inner(ec, options))
}
fn documents_inner(ec: &ExecutionContext, options: &Json) -> Result<Json, Error> {
    let id = text(options, "datasetId", "otel_spans");
    let dataset = dataset(ec, id)?;
    if dataset.service.is_some() {
        return Err(fail("a service dataset returns frames; run it as a report query"));
    }
    let facade = search::current()?;
    metadata(ec, || {
        for id in facade.target_documents(&dataset.target)? {
            facade.mapping(&id)?;
        }
        Ok(())
    })?;
    let mut request = SearchRequest::new(constraints(
        ec,
        options,
        &dataset,
        ec.effective_time().unwrap_or_else(crate::user::now_millis),
    )?);
    let mode = text(options, "retrievalMode", "lexical");
    if mode != "lexical" {
        let mut trusted = options.clone();
        trusted.as_object_mut().unwrap().remove("queryString");
        request.filter = Some(constraints(
            ec,
            &trusted,
            &dataset,
            ec.effective_time().unwrap_or_else(crate::user::now_millis),
        )?);
        let retrieval = search::hybrid::HybridOptions {
            text: text(options, "queryString", "").into(),
            ..Default::default()
        };
        request.retrieval = match mode {
            "semantic" => search::Retrieval::Semantic(retrieval),
            "hybrid" => search::Retrieval::Hybrid(retrieval),
            _ => return Err(fail("retrievalMode must be lexical, semantic or hybrid")),
        };
    }
    request.limit = options["pageSize"].as_u64().unwrap_or(50).clamp(1, 1000) as usize;
    request.offset = options["pageIndex"]
        .as_u64()
        .unwrap_or(0)
        .try_into()
        .unwrap_or(usize::MAX);
    request.offset = request
        .offset
        .checked_mul(request.limit)
        .ok_or_else(|| fail("page index is too large"))?;
    request.sort = options["sort"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(|name| match name {
                    "time" => dataset.time_field.clone(),
                    "-time" => format!("-{}", dataset.time_field),
                    other => other.into(),
                })
                .filter_map(|name| SortField::parse(&name))
                .collect()
        })
        .unwrap_or_else(|| vec![SortField::parse(&format!("-{}", dataset.time_field)).unwrap()]);
    if mode != "lexical" && options.get("sort").is_none() {
        request.sort.clear();
    }
    if mode == "lexical" && request.sort.last().is_none_or(|s| s.field != "__cursor") {
        request.sort.push(SortField::parse("__cursor").unwrap());
    }
    if let Some(cursor) = options.get("cursor").filter(|v| !v.is_null()) {
        if mode != "lexical" {
            return Err(fail("cursor paging is available for lexical retrieval"));
        }
        if cursor["datasetId"] != id
            || cursor["sort"]
                != json!(request
                    .sort
                    .iter()
                    .map(|s| format!("{}{}", if s.descending { "-" } else { "" }, s.field))
                    .collect::<Vec<_>>())
        {
            return Err(fail("cursor does not match this dataset and sort"));
        }
        let values = cursor["values"]
            .as_array()
            .ok_or_else(|| fail("cursor requires values"))?;
        request.search_after = Some(values.iter().map(moqui_lib::json::from_json).collect());
        request.offset = 0;
    }
    request.highlight = options["highlightFields"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .take(20)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let page = facade.search(&dataset.target, &request)?;
    let next_cursor=page.hits.last().filter(|_|mode=="lexical").map(|hit|{
        let values:Vec<Json>=request.sort.iter().map(|s|if s.field=="__cursor"{json!(format!("{}:{}",hit.document_id,hit.id))}else{hit.body.get(&s.field).map(moqui_lib::json::to_json).unwrap_or(Json::Null)}).collect();
        json!({"datasetId":id,"values":values,"sort":request.sort.iter().map(|s|format!("{}{}",if s.descending{"-"}else{""},s.field)).collect::<Vec<_>>()})
    });
    let mut definitions = BTreeMap::new();
    let mut hits = Vec::new();
    for hit in page.hits {
        let (title, links) = if search::telemetry::is_telemetry_index(&hit.document_id) {
            (String::new(), Vec::new())
        } else {
            decorate_document(ec,&facade,&hit,&mut definitions).unwrap_or_else(|error|{tracing::error!(document=%hit.document_id,id=%hit.id,%error,"report document metadata failed; keeping document");(hit.id.clone(),Vec::new())})
        };
        hits.push(json!({"documentId":hit.document_id,"id":hit.id,"documentTitle":title,"links":links,"body":moqui_lib::json::to_json(&Value::Map(hit.body)),"highlights":hit.highlights.into_iter().collect::<BTreeMap<_,_>>()}));
    }
    Ok(
        json!({"hits":hits,"total":page.total,"totalRelation":format!("{:?}",page.total_relation),"dataset":{"id":id,"timeField":dataset.time_field},"nextCursor":next_cursor,"notices":[]}),
    )
}

fn decorate_document(
    ec: &ExecutionContext,
    facade: &search::SearchFacade,
    hit: &search::SearchHit,
    definitions: &mut BTreeMap<String, (String, Vec<crate::entity::EntityValue>)>,
) -> Result<(String, Vec<Json>), Error> {
    if !definitions.contains_key(&hit.document_id) {
        let entity = facade.entity();
        let row = metadata(ec, || {
            entity
                .find("moqui.entity.document.DataDocument")?
                .eq("dataDocumentId", Value::string(&hit.document_id))
                .one()
        })?;
        let title = row
            .as_ref()
            .and_then(|r| r.get("documentTitle"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let links = metadata(ec, || {
            entity
                .find("moqui.entity.document.DataDocumentLink")?
                .eq("dataDocumentId", Value::string(&hit.document_id))
                .list()
        })?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
        definitions.insert(hit.document_id.clone(), (title, links));
    }
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in moqui_lib::collection::flatten_nested_map(&hit.body) {
        crate::script::set_context_name(&name, value)?;
    }
    crate::script::set_context_name("_id", Value::string(&hit.id))?;
    crate::script::set_context_name("_type", Value::string(&hit.document_id))?;
    let (template, records) = &definitions[&hit.document_id];
    let title = crate::script::expand(template)?;
    let mut links = Vec::new();
    for link in records {
        let condition = link.get("linkCondition").and_then(Value::as_str).unwrap_or("");
        let result = (|| -> Result<Option<Json>, Error> {
            if !condition.is_empty() && !crate::script::eval_condition(condition)? {
                return Ok(None);
            }
            let expand = |name: &str| crate::script::expand(link.get(name).and_then(Value::as_str).unwrap_or(""));
            Ok(Some(
                json!({"url":expand("linkUrl")?,"label":expand("label")?,"urlType":link.get("urlType").and_then(Value::as_str).unwrap_or("plain")}),
            ))
        })();
        match result {
            Ok(Some(link)) => links.push(link),
            Ok(None) => {}
            Err(error) => tracing::error!(document=%hit.document_id,%error,"document link failed; continuing"),
        }
    }
    Ok((title, links))
}

pub fn query_documents(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, Error> {
    documents(ec, &input(parameters)).map(output)
}
pub fn run_query(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(parameters);
    let options = params.get("query").unwrap_or(&params);
    execute_query(ec, options).map(output)
}

pub fn execute_query(ec: &ExecutionContext, options: &Json) -> Result<Json, Error> {
    let limits = search::limits::QueryLimits {
        timeout: std::time::Duration::from_millis(
            options["timeoutMillis"].as_u64().unwrap_or(10_000).clamp(100, 60_000),
        ),
        buckets: options["maxBuckets"].as_u64().unwrap_or(10_000).clamp(1, 65_536) as usize,
    };
    search::limits::with_limits(limits, || execute_query_inner(ec, options, &mut 64))
}

fn execute_query_inner(ec: &ExecutionContext, options: &Json, budget: &mut usize) -> Result<Json, Error> {
    if *budget == 0 {
        return Err(fail("report exceeds 64 queries"));
    }
    *budget -= 1;
    let now = ec.effective_time().unwrap_or_else(crate::user::now_millis);
    let range = json!({"from":jiff::Timestamp::from_millisecond(time::parse(text(options,"from","now-7d"),now)?).map_err(fail)?.to_string(),"thru":jiff::Timestamp::from_millisecond(time::parse(text(options,"thru","now"),now)?).map_err(fail)?.to_string()});
    if options["comparePrevious"] == true {
        return execute_comparison(ec, options, budget, &range);
    }
    if options["mode"] == "documents" {
        let mut query = options.clone();
        query["from"] = range["from"].clone();
        query["thru"] = range["thru"].clone();
        let documents = documents_inner(ec, &query)?;
        let rows = documents["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|hit| {
                let mut row = hit["body"]
                    .as_object()
                    .map(|map| {
                        map.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<BTreeMap<_, _>>()
                    })
                    .unwrap_or_default();
                row.insert("_id".into(), hit["id"].clone());
                row.insert("_type".into(), hit["documentId"].clone());
                row
            })
            .collect::<Vec<_>>();
        let mut frame = frame::Frame::from_rows(&rows);
        let dataset = dataset(ec, text(options, "datasetId", "otel_spans"))?;
        configure_frame(ec, &dataset, options, &mut frame)?;
        let (frames, inspection) = apply_transformations(vec![frame], options)?;
        return Ok(
            json!({"frames":frames,"inspection":inspection,"notices":documents["notices"],"range":range,"sourceCount":documents["total"],"nextCursor":documents["nextCursor"]}),
        );
    }
    if let Some(queries) = options["queries"].as_array() {
        let mut frames = Vec::new();
        let mut notices = Vec::new();
        let mut inspections: Vec<Json> = Vec::new();
        let mut requests = Vec::new();
        let mut inspection_budget = 1_048_576;
        for (query_index, query) in queries.iter().enumerate() {
            let mut query = query.clone();
            if options["inspect"] == true {
                query["inspect"] = json!(true);
            }
            if query.get("from").is_none() {
                query["from"] = range["from"].clone();
            }
            if query.get("thru").is_none() {
                query["thru"] = range["thru"].clone();
            }
            let result = match execute_query_inner(ec, &query, budget) {
                Ok(result) => result,
                Err(error) => {
                    tracing::error!(%error,"independent report query failed");
                    notices.push(json!({"level":"error","message":error.message(),"complete":false}));
                    continue;
                }
            };
            for frame in result["frames"].as_array().into_iter().flatten() {
                frames.push(serde_json::from_value::<frame::Frame>(frame.clone()).map_err(fail)?);
            }
            notices.extend(result["notices"].as_array().into_iter().flatten().cloned());
            if options["inspect"] == true {
                for mut step in result["inspection"].as_array().into_iter().flatten().cloned() {
                    step["queryIndex"] = json!(query_index);
                    append_inspection(&mut inspections, step, &mut inspection_budget);
                }
                requests
                    .push(json!({"queryIndex":query_index,"request":result["request"],"requests":result["requests"]}));
            }
        }
        let (frames, inspection) = apply_transformations(frames, options)?;
        for step in inspection {
            append_inspection(&mut inspections, step, &mut inspection_budget);
        }
        return Ok(
            json!({"frames":frames,"inspection":inspections,"requests":requests,"notices":notices,"range":range}),
        );
    }
    let dataset = dataset(ec, text(options, "datasetId", "otel_metrics"))?;
    if let Some(service) = &dataset.service {
        if options["filters"]
            .as_array()
            .is_some_and(|filters| filters.iter().any(|filter| filter["disabled"] != true))
        {
            return Err(fail("document filter pills cannot be applied to a service dataset; define authorized service parameters instead"));
        }
        let mut parameters = match moqui_lib::json::from_json(&options["parameters"]) {
            Value::Map(map) => map.into_iter().collect(),
            _ => Parameters::new(),
        };
        let definition = definitions::read(ec, &dataset.id)?;
        for key in ["from", "thru"] {
            if let Some(parameter) = definition["body"]["timeParameters"][key].as_str() {
                parameters.insert(parameter.into(), moqui_lib::json::from_json(&range[key]));
            }
        }
        let result =
            crate::service::call::sync_checked(service, &parameters, &crate::service::call::CallOptions::default())
                .map_err(fail)?;
        let result = input(&result);
        let mut frame = if let Some(frame) = result.get("frame") {
            serde_json::from_value::<frame::Frame>(frame.clone()).map_err(fail)?
        } else {
            let rows = result["rows"]
                .as_array()
                .ok_or_else(|| fail("report source service must return rows or a frame"))?;
            if rows.len() > 10_000 {
                return Err(fail("service report exceeds 10000 rows"));
            }
            let rows = rows
                .iter()
                .map(|row| {
                    row.as_object()
                        .map(|map| {
                            map.iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect::<BTreeMap<_, _>>()
                        })
                        .ok_or_else(|| fail("service source rows must be objects"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            frame::Frame::from_rows(&rows)
        };
        configure_frame(ec, &dataset, options, &mut frame)?;
        let (frames, inspection) = apply_transformations(vec![frame], options)?;
        return Ok(json!({"frames":frames,"inspection":inspection,"notices":[],"range":range}));
    }
    let filter = constraints(ec, options, &dataset, now)?;
    let mut normalized = options.clone();
    let mut notices = Vec::new();
    normalize_intervals(&mut normalized, &range, &mut notices)?;
    let mut aggs = compile_aggregations(&normalized)?;
    fn aggregation_count(aggs: &IndexMap<String, search::Agg>) -> usize {
        aggs.len()
            + aggs
                .values()
                .filter_map(search::Agg::sub_aggs)
                .map(aggregation_count)
                .sum::<usize>()
    }
    if aggregation_count(&aggs) > 64 {
        return Err(fail("report exceeds 64 aggregation nodes"));
    }
    aggs.insert(
        "__report_population".into(),
        search::Agg::Filter {
            query: Query::MatchAll,
            aggs: IndexMap::new(),
        },
    );
    let facade = search::current()?;
    metadata(ec, || {
        for id in facade.target_documents(&dataset.target)? {
            facade.mapping(&id)?;
        }
        Ok(())
    })?;
    let mut result = match facade.aggregate(&dataset.target, &filter, &aggs) {
        Ok(result) => result,
        Err(error) if error.message().contains("groups") || error.message().contains("too many buckets") => {
            return Ok(
                json!({"frames":[],"range":range,"notices":[{"level":"warning","code":"too_many_groups","message":error.message(),"complete":false}]}),
            )
        }
        Err(error) => return Err(error),
    };
    let population = match result.shift_remove("__report_population") {
        Some(Value::Map(map)) => map
            .get("doc_count")
            .map(moqui_lib::object::to_plain_string)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0),
        _ => 0,
    };
    let mut partials = Vec::new();
    enrich_buckets(&dataset.id, &aggs, &mut result, &Query::MatchAll, true, &mut partials)?;
    let mut only_partial = false;
    if !partials.is_empty() {
        let filters = partials
            .iter()
            .enumerate()
            .map(|(index, partial)| {
                (
                    index.to_string(),
                    Query::all(vec![
                        partial.scope.clone(),
                        Query::any(
                            partial
                                .spec
                                .ranges
                                .iter()
                                .map(|(from, thru)| {
                                    Query::range(
                                        &partial.spec.field,
                                        Some(Value::Timestamp(*from)),
                                        Some(Value::Timestamp(*thru)),
                                    )
                                })
                                .collect(),
                        ),
                    ]),
                )
            })
            .collect();
        let counted = facade.aggregate(
            &dataset.target,
            &filter,
            &IndexMap::from([(
                "excluded".into(),
                search::Agg::NamedFilters {
                    filters,
                    aggs: IndexMap::new(),
                },
            )]),
        )?;
        let counts = match counted.get("excluded") {
            Some(Value::Map(map)) => match map.get("buckets") {
                Some(Value::List(buckets)) => buckets
                    .iter()
                    .filter_map(|bucket| {
                        if let Value::Map(map) = bucket {
                            Some((
                                map.get("key")
                                    .map(moqui_lib::object::to_plain_string)
                                    .unwrap_or_default(),
                                map.get("doc_count")
                                    .map(moqui_lib::object::to_plain_string)
                                    .and_then(|count| count.parse::<u64>().ok())
                                    .unwrap_or(0),
                            ))
                        } else {
                            None
                        }
                    })
                    .collect::<BTreeMap<_, _>>(),
                _ => BTreeMap::new(),
            },
            _ => BTreeMap::new(),
        };
        for (index, partial) in partials.iter().enumerate() {
            let count = counts.get(&index.to_string()).copied().unwrap_or(0);
            if count == 0 {
                continue;
            }
            if partial.root && partial.spec.drop && count >= population {
                only_partial = true;
            }
            let mut notice = json!({"level":if partial.spec.drop{"warning"}else{"info"},"code":if partial.spec.drop{"partial_buckets_excluded"}else{"partial_buckets_retained"},"aggregation":partial.spec.id,"observations":count,"ranges":partial.spec.ranges.iter().map(|(from,thru)|json!({"from":from.to_string(),"thru":thru.to_string()})).collect::<Vec<_>>(),"message":if partial.spec.drop{"Observations in partial time buckets were excluded within this bucket's filters"}else{"The first or last time bucket covers only part of its interval"}});
            if partial.spec.drop {
                notice["complete"] = json!(false);
            }
            notices.push(notice);
        }
    }
    if only_partial {
        notices.push(json!({"level":"info","code":"no_data","message":"No completed time bucket contains matching observations"}));
        return Ok(json!({"frames":[],"notices":notices,"sourceCount":population,"range":range}));
    }
    if population == 0 && dataset.telemetry {
        return Ok(
            json!({"frames":[],"notices":[{"level":"info","code":"no_data","message":"No matching telemetry observations"}],"sourceCount":0,"range":range}),
        );
    }
    let mut result = Json::Object(
        result
            .into_iter()
            .map(|(k, v)| (k, moqui_lib::json::to_json(&v)))
            .collect(),
    );
    let subtotals = options["subtotals"] == true;
    if subtotals {
        result
            .as_object_mut()
            .unwrap()
            .entry("count")
            .or_insert_with(|| json!(population));
    }
    let mut frame = frame::tabify_with_subtotals(&result, subtotals);
    let mut date_fields = std::collections::BTreeSet::new();
    fn dates(aggs: &IndexMap<String, search::Agg>, prefix: &str, fields: &mut std::collections::BTreeSet<String>) {
        for (name, agg) in aggs {
            if matches!(agg, search::Agg::DateHistogram(_)) {
                fields.insert(name.clone());
                fields.insert(format!("{prefix}{name}"));
            }
            if let Some(children) = agg.sub_aggs() {
                dates(children, &format!("{prefix}{name}."), fields);
            }
        }
    }
    dates(&aggs, "", &mut date_fields);
    for field in &mut frame.fields {
        if date_fields.contains(&field.name) {
            field.kind = "time".into();
        }
    }
    configure_frame(ec, &dataset, options, &mut frame)?;
    let (frames, inspection) = apply_transformations(vec![frame], options)?;
    Ok(
        json!({"frames":frames,"inspection":inspection,"request":if options["inspect"]==true{Some(&normalized)}else{None},"notices":notices,"sourceCount":population,"range":range}),
    )
}

fn apply_transformations(frames: Vec<frame::Frame>, options: &Json) -> Result<(Vec<frame::Frame>, Vec<Json>), Error> {
    let bucket_sources = frames
        .iter()
        .map(|frame| frame::Frame {
            fields: frame
                .fields
                .iter()
                .filter(|field| field.config.contains_key("bucket"))
                .cloned()
                .collect(),
        })
        .collect::<Vec<_>>();
    let steps = options["transformations"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if steps.len() > 64 {
        return Err(fail("report exceeds 64 transformation steps"));
    }
    let mut per_frame = frames
        .iter()
        .map(|frame| {
            frame
                .fields
                .iter()
                .map(|field| (field.name.clone(), (field.unit.clone(), field.config.clone())))
                .collect::<BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();
    let mut metadata = BTreeMap::new();
    for (index, fields) in per_frame.iter().enumerate() {
        for (name, config) in fields {
            metadata.entry(name.clone()).or_insert_with(|| config.clone());
            metadata.insert(format!("{name}.{}", index + 1), config.clone());
        }
    }
    for step in steps
        .iter()
        .filter(|step| step["disabled"] != true && step["type"] == "organize")
    {
        for (old, new) in step["rename"].as_object().into_iter().flat_map(|map| map.iter()) {
            for fields in &mut per_frame {
                if let (Some(new), Some(config)) = (new.as_str(), fields.remove(old)) {
                    fields.insert(new.into(), config);
                }
            }
            if let (Some(new), Some(config)) = (new.as_str(), metadata.remove(old)) {
                metadata.insert(new.into(), config);
            }
        }
    }
    let (mut frames, inspection) = if options["inspect"] == true {
        frame::transform_inspected(frames, steps)?
    } else {
        (frame::transform_frames(frames, steps)?, Vec::new())
    };
    let combines = steps.iter().any(|step| {
        step["disabled"] != true
            && matches!(
                text(step, "type", ""),
                "join"
                    | "joinByTime"
                    | "merge"
                    | "comparePrevious"
                    | "longToWide"
                    | "wideToLong"
                    | "partition"
                    | "partitionByValues"
            )
    });
    let separate = !combines && frames.len() == per_frame.len();
    for (index, frame) in frames.iter_mut().enumerate() {
        let rebuild = frame
            .fields
            .iter()
            .map(|field| {
                field
                    .config
                    .get("bucket")
                    .and_then(|bucket| bucket["filters"].as_array())
                    .is_none_or(|filters| filters.len() != field.values.len())
            })
            .collect::<Vec<_>>();
        for field in &mut frame.fields {
            let config = if separate {
                per_frame[index].get(&field.name).or_else(|| metadata.get(&field.name))
            } else {
                metadata.get(&field.name)
            };
            if let Some((unit, config)) = config {
                if field.unit.is_none() {
                    field.unit = unit.clone();
                }
                for (key, value) in config {
                    field.config.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        restore_bucket_filters(frame, index, separate, &bucket_sources, steps, &rebuild)?;
    }
    Ok((frames, inspection))
}

fn restore_bucket_filters(
    output: &mut frame::Frame,
    output_index: usize,
    separate: bool,
    sources: &[frame::Frame],
    steps: &[Json],
    rebuild: &[bool],
) -> Result<(), Error> {
    let output_names = output
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let name = |source_index: usize, original: &str| {
        let duplicate = !separate
            && sources
                .iter()
                .take(source_index)
                .any(|source| source.fields.iter().any(|field| field.name == original));
        let mut name = if duplicate {
            format!("{original}.{}", source_index + 1)
        } else {
            original.into()
        };
        for step in steps
            .iter()
            .filter(|step| step["disabled"] != true && step["type"] == "organize")
        {
            if let Some(renamed) = step["rename"][&name].as_str() {
                name = renamed.into();
            }
        }
        name
    };
    let rows = output.rows();
    for (field_index, field) in output.fields.iter_mut().enumerate() {
        if !rebuild[field_index] || !field.config.contains_key("bucket") {
            continue;
        }
        let mut grouped = BTreeMap::<String, Vec<Json>>::new();
        let mut labels = BTreeMap::<String, Json>::new();
        let mut key_fields = Vec::new();
        for (source_index, source) in sources.iter().enumerate() {
            if separate && source_index != output_index {
                continue;
            }
            let Some(origin) = source
                .fields
                .iter()
                .find(|origin| name(source_index, &origin.name) == field.name)
            else {
                continue;
            };
            let dimensions = source
                .fields
                .iter()
                .filter_map(|source| {
                    let target = name(source_index, &source.name);
                    output_names.contains(&target).then_some((source, target))
                })
                .collect::<Vec<_>>();
            key_fields = dimensions.iter().map(|(_, target)| target.clone()).collect();
            for index in 0..origin.values.len() {
                let key = serde_json::to_string(
                    &dimensions
                        .iter()
                        .map(|(field, _)| field.values.get(index).cloned().unwrap_or(Json::Null))
                        .collect::<Vec<_>>(),
                )
                .map_err(fail)?;
                if let Some(query) = origin.config["bucket"]["filters"]
                    .get(index)
                    .filter(|query| !query.is_null())
                {
                    grouped.entry(key.clone()).or_default().push(query.clone());
                }
                if let Some(label) = origin.config["bucket"]["labels"].get(index) {
                    labels.entry(key).or_insert_with(|| label.clone());
                }
            }
        }
        let mut filters = Vec::new();
        let mut display = Vec::new();
        for row in &rows {
            let key = serde_json::to_string(
                &key_fields
                    .iter()
                    .map(|field| row.get(field).cloned().unwrap_or(Json::Null))
                    .collect::<Vec<_>>(),
            )
            .map_err(fail)?;
            let mut unique = std::collections::BTreeSet::new();
            let queries = grouped
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|query| unique.insert(query.to_string()))
                .map(|query| Query::from_value(&moqui_lib::json::from_json(query)))
                .collect::<Result<Vec<_>, _>>()?;
            filters.push(if queries.is_empty() {
                Json::Null
            } else {
                moqui_lib::json::to_json(&Query::any(queries).to_value())
            });
            display.push(labels.get(&key).cloned().unwrap_or(Json::Null));
        }
        field.config.get_mut("bucket").unwrap()["filters"] = json!(filters);
        field.config.get_mut("bucket").unwrap()["labels"] = json!(display);
    }
    Ok(())
}

fn append_inspection(output: &mut Vec<Json>, mut step: Json, remaining: &mut usize) {
    let size = step.to_string().len();
    if size > *remaining {
        step["input"] = json!({"truncated":true,"reason":"inspection snapshot budget exhausted"});
        step["output"] = json!({"truncated":true,"reason":"inspection snapshot budget exhausted"});
    } else {
        *remaining -= size;
    }
    output.push(step);
}

fn dataset_field_defaults(ec: &ExecutionContext, dataset: &Dataset) -> Result<BTreeMap<String, Json>, Error> {
    if search::telemetry::is_telemetry_index(&dataset.id) || dataset.id.starts_with("index:") {
        return Ok(BTreeMap::new());
    }
    match definitions::read(ec, &dataset.id) {
        Ok(definition) => definition["body"]["fieldDefaults"]
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(name, config)| (name.clone(), config.clone()))
                    .collect()
            })
            .ok_or_else(|| fail("dataset fieldDefaults must be a field-name map"))
            .or_else(|error| {
                if definition["body"]["fieldDefaults"].is_null() {
                    Ok(BTreeMap::new())
                } else {
                    Err(error)
                }
            }),
        Err(error) if error.message().contains("does not exist") => Ok(BTreeMap::new()),
        Err(error) => Err(error),
    }
}

/// Resolve only the keys present in the authorized result, with one cached entity query per
/// field. Formatting never replaces the raw key used for filters, joins, alerts or exports.
fn configure_frame(
    ec: &ExecutionContext,
    dataset: &Dataset,
    query: &Json,
    frame: &mut frame::Frame,
) -> Result<(), Error> {
    let defaults = dataset_field_defaults(ec, dataset)?;
    let mut origins = BTreeMap::new();
    fn collect(value: &Json, origins: &mut BTreeMap<String, String>) {
        for (name, agg) in value.as_object().into_iter().flat_map(|map| map.iter()) {
            for params in agg.as_object().into_iter().flat_map(|map| map.values()) {
                if let Some(field) = params["field"].as_str() {
                    origins.insert(name.clone(), field.into());
                }
            }
            if let Some(nested) = agg.get("aggs") {
                collect(nested, origins);
            }
        }
    }
    collect(&query["aggs"], &mut origins);
    for agg in query["aggregations"].as_array().into_iter().flatten() {
        if let (Some(name), Some(field)) = (agg["id"].as_str(), agg["params"]["field"].as_str()) {
            origins.insert(name.into(), field.into());
        }
    }
    for field in &mut frame.fields {
        let source = origins.get(&field.name).map(String::as_str).unwrap_or(&field.name);
        let Some(config) = defaults.get(&field.name).or_else(|| defaults.get(source)) else {
            continue;
        };
        let Some(config) = config.as_object() else {
            return Err(fail(format!("dataset fieldDefaults.{source} must be a map")));
        };
        field
            .config
            .extend(config.iter().map(|(name, value)| (name.clone(), value.clone())));
        if let Some(unit) = config.get("unit").and_then(Json::as_str) {
            field.unit = Some(unit.into());
        }
        let lookup = config.get("lookup").cloned().or_else(||config.get("statusTypeId").map(|id|json!({"entityName":"moqui.basic.StatusItem","keyField":"statusId","displayField":"description","conditions":{"statusTypeId":id}}))).or_else(||config.get("enumTypeId").map(|id|json!({"entityName":"moqui.basic.Enumeration","keyField":"enumId","displayField":"description","conditions":{"enumTypeId":id}})));
        let Some(lookup) = lookup else { continue };
        let keys = field
            .values
            .iter()
            .filter(|value| !value.is_null() && !value.is_array() && !value.is_object())
            .map(Json::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        if keys.len() > 1000 {
            tracing::warn!(dataset=%dataset.id,field=%field.name,"report lookup exceeds 1000 distinct keys; keeping raw values");
            continue;
        }
        if keys.is_empty() {
            continue;
        }
        let resolve = || -> Result<Vec<Json>, Error> {
            let entity = ec
                .factory()
                .entity()
                .ok_or_else(|| fail("entity facade is unavailable"))?;
            let key = text(&lookup, "keyField", "");
            let display = text(&lookup, "displayField", "description");
            if key.is_empty() {
                return Err(fail("dataset lookup needs keyField"));
            }
            let mut find = entity
                .find(text(&lookup, "entityName", ""))?
                .condition(crate::entity::Condition::is_in(
                    key,
                    keys.iter()
                        .map(|key| {
                            serde_json::from_str::<Json>(key)
                                .map(|value| moqui_lib::json::from_json(&value))
                                .map_err(fail)
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ))
                .use_cache(true)
                .limit(1000);
            for (name, value) in lookup["conditions"].as_object().into_iter().flat_map(|map| map.iter()) {
                find = find.eq(name, moqui_lib::json::from_json(value));
            }
            let rows = find.list()?;
            Ok(rows.iter().filter_map(|row|row.get(key).zip(row.get(display))).map(|(key,display)|json!({"value":moqui_lib::json::to_json(key),"text":moqui_lib::object::to_plain_string(display)})).collect())
        };
        match resolve() {
            Ok(mappings) => {
                let existing = field.config.entry("mappings".into()).or_insert_with(|| json!([]));
                if let Some(existing) = existing.as_array_mut() {
                    existing.extend(mappings);
                }
            }
            Err(error) => {
                tracing::error!(dataset=%dataset.id,field=%field.name,%error,"report field lookup failed; keeping authorized raw values")
            }
        }
    }
    Ok(())
}

fn execute_comparison(ec: &ExecutionContext, options: &Json, budget: &mut usize, range: &Json) -> Result<Json, Error> {
    fn capable(ec: &ExecutionContext, query: &Json) -> Result<bool, Error> {
        if query["mode"] == "documents" {
            return Err(fail(
                "previous-period comparisons require aggregate or time-aware service queries",
            ));
        }
        if let Some(queries) = query["queries"].as_array() {
            let mut service = false;
            for query in queries {
                service |= capable(ec, query)?;
            }
            return Ok(service);
        }
        let dataset = dataset(ec, text(query, "datasetId", "otel_metrics"))?;
        if dataset.service.is_some() {
            let definition = definitions::read(ec, &dataset.id)?;
            if !definition["body"]["timeParameters"]["from"].is_string()
                || !definition["body"]["timeParameters"]["thru"].is_string()
            {
                return Err(fail(
                    "a service dataset needs timeParameters.from/thru for a previous-period comparison",
                ));
            }
            return Ok(true);
        }
        Ok(false)
    }
    let has_service = capable(ec, options)?;
    let from = time::parse(range["from"].as_str().unwrap(), 0)?;
    let thru = time::parse(range["thru"].as_str().unwrap(), 0)?;
    let width = thru
        .checked_sub(from)
        .filter(|v| *v > 0)
        .ok_or_else(|| fail("comparison requires a positive time range"))?;
    let mut current_options = options.clone();
    current_options.as_object_mut().unwrap().remove("comparePrevious");
    current_options["from"] = range["from"].clone();
    current_options["thru"] = range["thru"].clone();
    fn child_ranges(query: &mut Json, anchor: i64, shift: i64) -> Result<(), Error> {
        if let Some(queries) = query["queries"].as_array_mut() {
            for query in queries {
                for key in ["from", "thru"] {
                    if let Some(value) = query[key].as_str() {
                        query[key] = json!(jiff::Timestamp::from_millisecond(
                            time::parse(value, anchor)?
                                .checked_add(shift)
                                .ok_or_else(|| fail("comparison time overflow"))?
                        )
                        .map_err(fail)?
                        .to_string());
                    }
                }
                child_ranges(query, anchor, shift)?;
            }
        }
        Ok(())
    }
    let now = ec.effective_time().unwrap_or_else(crate::user::now_millis);
    child_ranges(&mut current_options, now, 0)?;
    let mut previous_options = current_options.clone();
    previous_options["from"] = json!(jiff::Timestamp::from_millisecond(
        from.checked_sub(width)
            .ok_or_else(|| fail("comparison time overflow"))?
    )
    .map_err(fail)?
    .to_string());
    previous_options["thru"] = range["from"].clone();
    child_ranges(&mut previous_options, now, -width)?;
    let current = execute_query_inner(ec, &current_options, budget)?;
    let previous = execute_query_inner(ec, &previous_options, budget)?;
    let frames = |result: &Json| {
        result["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|frame| serde_json::from_value::<frame::Frame>(frame.clone()).map_err(fail))
            .collect::<Result<Vec<_>, _>>()
    };
    fn bucket_ids(query: &Json, ids: &mut BTreeMap<String, ()>) {
        if let Some(queries) = query["queries"].as_array() {
            for query in queries {
                bucket_ids(query, ids);
            }
        }
        for agg in query["aggregations"].as_array().into_iter().flatten() {
            if text(agg, "role", "metric") != "metric" {
                ids.insert(text(agg, "id", "").into(), ());
            }
        }
        fn raw(value: &Json, ids: &mut BTreeMap<String, ()>) {
            if let Some(aggs) = value.as_object() {
                for (name, agg) in aggs {
                    if ["terms", "date_histogram", "histogram", "range", "date_range", "filters"]
                        .iter()
                        .any(|kind| agg.get(*kind).is_some())
                    {
                        ids.insert(name.clone(), ());
                    }
                    if let Some(nested) = agg.get("aggs") {
                        raw(nested, ids);
                    }
                }
            }
        }
        if let Some(aggs) = query.get("aggs") {
            raw(aggs, ids);
        }
        for step in query["transformations"].as_array().into_iter().flatten() {
            if step["type"] == "organize" {
                for (old, new) in step["rename"].as_object().into_iter().flat_map(|map| map.iter()) {
                    if ids.remove(old).is_some() {
                        if let Some(new) = new.as_str() {
                            ids.insert(new.into(), ());
                        }
                    }
                }
            }
            if step["type"] == "groupBy" {
                for field in step["fields"].as_array().into_iter().flatten().filter_map(Json::as_str) {
                    ids.insert(field.into(), ());
                }
            }
        }
    }
    let mut keys = BTreeMap::new();
    bucket_ids(options, &mut keys);
    let keys = options["comparisonKeys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| keys.into_keys().collect());
    let compared = frame::compare_previous(
        frames(&current)?,
        frames(&previous)?,
        width,
        if has_service && options.get("comparisonKeys").is_none() {
            None
        } else {
            Some(&keys)
        },
    )?;
    let mut post_options = options.clone();
    post_options["transformations"] = options
        .get("comparisonTransformations")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let (frames, mut inspection) = apply_transformations(compared, &post_options)?;
    for (period, result) in [("current", &current), ("previous", &previous)] {
        for mut step in result["inspection"].as_array().into_iter().flatten().cloned() {
            step["period"] = json!(period);
            inspection.push(step);
        }
    }
    let mut notices = current["notices"].as_array().cloned().unwrap_or_default();
    for mut notice in previous["notices"].as_array().into_iter().flatten().cloned() {
        notice["period"] = json!("previous");
        if notice["code"] == "no_data" {
            notice["complete"] = json!(false);
        }
        notices.push(notice);
    }
    let mut bounded = Vec::new();
    let mut remaining = 1_048_576;
    for step in inspection {
        append_inspection(&mut bounded, step, &mut remaining);
    }
    Ok(
        json!({"frames":frames,"inspection":bounded,"requests":[{"period":"current","request":current["request"],"requests":current["requests"]},{"period":"previous","request":previous["request"],"requests":previous["requests"]}],"range":range,"previousRange":previous["range"],"sourceCount":current["sourceCount"],"notices":notices}),
    )
}

struct PartialHistogram {
    id: String,
    field: String,
    drop: bool,
    ranges: Vec<(jiff::Timestamp, jiff::Timestamp)>,
}
struct ScopedPartial {
    spec: PartialHistogram,
    scope: Query,
    root: bool,
}

fn bucket_predicate(
    agg: &search::Agg,
    bucket: &IndexMap<String, Value>,
    siblings: &[Value],
) -> Result<Option<Query>, Error> {
    let key = bucket.get("key").cloned().unwrap_or(Value::Null);
    let exists = |field: &str| {
        Query::from_value(&moqui_lib::json::from_json(
            &json!({"field":{"name":field,"op":"exists"}}),
        ))
    };
    Ok(Some(match agg {
        search::Agg::Terms(terms) => {
            if bucket.get("missing") == Some(&Value::Bool(true)) {
                Query::exclude(exists(&terms.field)?)
            } else if bucket.get("other") == Some(&Value::Bool(true)) {
                let mut excluded = Vec::new();
                let mut keys = Vec::new();
                for other in siblings {
                    if let Value::Map(other) = other {
                        if other.get("other") == Some(&Value::Bool(true)) {
                            continue;
                        }
                        if other.get("missing") == Some(&Value::Bool(true)) {
                            excluded.push(Query::exclude(exists(&terms.field)?));
                        } else if let Some(key) = other.get("key") {
                            keys.push(key.clone());
                        }
                    }
                }
                if !keys.is_empty() {
                    excluded.push(Query::is_in(&terms.field, keys));
                }
                if excluded.is_empty() {
                    Query::MatchAll
                } else {
                    Query::exclude(Query::any(excluded))
                }
            } else {
                Query::eq(&terms.field, key)
            }
        }
        search::Agg::DateHistogram(histogram) => {
            let from = match key {
                Value::Timestamp(time) => time,
                value => {
                    let value = moqui_lib::object::to_plain_string(&value);
                    if let Ok(millis) = value.parse::<i64>() {
                        jiff::Timestamp::from_millisecond(millis).map_err(fail)?
                    } else {
                        value.parse::<jiff::Timestamp>().map_err(fail)?
                    }
                }
            };
            let thru = if let Some(millis) = histogram.fixed_millis {
                jiff::Timestamp::from_millisecond(
                    from.as_millisecond()
                        .checked_add(millis)
                        .ok_or_else(|| fail("bucket time overflow"))?,
                )
                .map_err(fail)?
            } else {
                search::agg::next_bucket(
                    from,
                    histogram.unit,
                    &search::agg::histogram_zone(histogram.time_zone.as_deref())?,
                    histogram.offset,
                )?
            };
            Query::range(
                &histogram.field,
                Some(Value::Timestamp(from)),
                Some(Value::Timestamp(thru)),
            )
        }
        search::Agg::Histogram { field, interval, .. } => {
            let from = moqui_lib::object::to_plain_string(&key).parse::<f64>().map_err(fail)?;
            Query::range(field, Some(Value::F64(from)), Some(Value::F64(from + interval)))
        }
        search::Agg::NamedFilters { filters, .. } => match filters.get(&moqui_lib::object::to_plain_string(&key)) {
            Some(filter) => filter.clone(),
            None => return Ok(None),
        },
        _ => return Ok(None),
    }))
}

fn enrich_buckets(
    dataset: &str,
    aggs: &IndexMap<String, search::Agg>,
    results: &mut IndexMap<String, Value>,
    scope: &Query,
    root: bool,
    partials: &mut Vec<ScopedPartial>,
) -> Result<(), Error> {
    for (name, agg) in aggs {
        let Some(Value::Map(result)) = results.get_mut(name) else {
            continue;
        };
        if let search::Agg::Filter { query, aggs } = agg {
            enrich_buckets(
                dataset,
                aggs,
                result,
                &Query::all(vec![scope.clone(), query.clone()]),
                false,
                partials,
            )?;
            continue;
        }
        if let search::Agg::DateHistogram(histogram) = agg {
            let mut histogram = histogram.clone();
            histogram.aggs.clear();
            for spec in partial_histograms(&IndexMap::from([(name.clone(), search::Agg::DateHistogram(histogram))]))? {
                partials.push(ScopedPartial {
                    spec,
                    scope: scope.clone(),
                    root,
                });
            }
        }
        let Some(Value::List(buckets)) = result.get_mut("buckets") else {
            continue;
        };
        let siblings = buckets.clone();
        for bucket in buckets {
            let Value::Map(bucket) = bucket else { continue };
            let Some(predicate) = bucket_predicate(agg, bucket, &siblings)? else {
                continue;
            };
            let query = Query::all(vec![scope.clone(), predicate]);
            let (kind, field) = match agg {
                search::Agg::Terms(terms) => ("terms", terms.field.as_str()),
                search::Agg::DateHistogram(histogram) => ("date_histogram", histogram.field.as_str()),
                search::Agg::Histogram { field, .. } => ("histogram", field.as_str()),
                search::Agg::NamedFilters { .. } => ("filters", ""),
                _ => ("", ""),
            };
            let label = bucket
                .get("key_display")
                .or_else(|| bucket.get("key"))
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            bucket.insert("__bucket".into(),moqui_lib::json::from_json(&json!({"datasetId":dataset,"kind":kind,"field":field,"query":moqui_lib::json::to_json(&query.to_value()),"label":label})));
            if let Some(nested) = agg.sub_aggs() {
                enrich_buckets(dataset, nested, bucket, &query, false, partials)?;
            }
        }
    }
    Ok(())
}

fn partial_histograms(aggs: &IndexMap<String, search::Agg>) -> Result<Vec<PartialHistogram>, Error> {
    let mut out = Vec::new();
    for (id, agg) in aggs {
        if let search::Agg::DateHistogram(histogram) = agg {
            if let (Some(from), Some(thru)) = (histogram.range_start, histogram.range_end) {
                let zone = search::agg::histogram_zone(histogram.time_zone.as_deref())?;
                let start = |at: jiff::Timestamp| -> Result<jiff::Timestamp, Error> {
                    if let Some(width) = histogram.fixed_millis {
                        let offset = i64::try_from(histogram.offset.as_millis()).map_err(fail)?;
                        let shifted = at
                            .as_millisecond()
                            .checked_sub(offset)
                            .ok_or_else(|| fail("histogram time overflow"))?;
                        let aligned = shifted
                            .div_euclid(width)
                            .checked_mul(width)
                            .and_then(|at| at.checked_add(offset))
                            .ok_or_else(|| fail("histogram time overflow"))?;
                        jiff::Timestamp::from_millisecond(aligned).map_err(fail)
                    } else {
                        search::agg::bucket_start(at, histogram.unit, &zone, histogram.offset)
                    }
                };
                let end = |at: jiff::Timestamp| -> Result<jiff::Timestamp, Error> {
                    if let Some(width) = histogram.fixed_millis {
                        jiff::Timestamp::from_millisecond(
                            at.as_millisecond()
                                .checked_add(width)
                                .ok_or_else(|| fail("histogram time overflow"))?,
                        )
                        .map_err(fail)
                    } else {
                        search::agg::next_bucket(at, histogram.unit, &zone, histogram.offset)
                    }
                };
                let first = start(from)?;
                let last =
                    start(jiff::Timestamp::from_millisecond(thru.as_millisecond().saturating_sub(1)).map_err(fail)?)?;
                let mut ranges = Vec::new();
                if first < from {
                    ranges.push((from, end(first)?.min(thru)));
                }
                if end(last)? > thru {
                    let range = (last.max(from), thru);
                    if !ranges.contains(&range) {
                        ranges.push(range);
                    }
                }
                if !ranges.is_empty() {
                    out.push(PartialHistogram {
                        id: id.clone(),
                        field: histogram.field.clone(),
                        drop: histogram.drop_partial,
                        ranges,
                    });
                }
            }
        }
        if let Some(children) = agg.sub_aggs() {
            out.extend(partial_histograms(children)?);
        }
    }
    Ok(out)
}

fn normalize_intervals(options: &mut Json, range: &Json, notices: &mut Vec<Json>) -> Result<(), Error> {
    let from = range["from"]
        .as_str()
        .unwrap()
        .parse::<jiff::Timestamp>()
        .map_err(fail)?
        .as_millisecond();
    let thru = range["thru"]
        .as_str()
        .unwrap()
        .parse::<jiff::Timestamp>()
        .map_err(fail)?
        .as_millisecond();
    let width = thru.saturating_sub(from);
    fn visit(params: &mut Json, from: i64, thru: i64, width: i64, notices: &mut Vec<Json>) -> Result<(), Error> {
        let target = params["targetBars"].as_i64().unwrap_or(60).clamp(1, 2000);
        let wanted = width.div_euclid(target).max(1);
        let table = [
            1_000,
            5_000,
            10_000,
            30_000,
            60_000,
            300_000,
            600_000,
            1_800_000,
            3_600_000,
            10_800_000,
            21_600_000,
            43_200_000,
            86_400_000,
            604_800_000,
            2_592_000_000,
            7_776_000_000,
            31_536_000_000,
        ];
        let auto = params["interval"] == "auto" || params["fixed_interval"] == "auto";
        let requested = params["fixed_interval"]
            .as_str()
            .filter(|s| *s != "auto")
            .map(search::agg::fixed_interval)
            .transpose()?;
        if auto || requested.is_some_and(|ms| ms < wanted) {
            let chosen = table.into_iter().find(|ms| *ms >= wanted).unwrap_or(31_536_000_000_i64);
            if requested.is_some() {
                notices.push(json!({"code":"interval_scaled","message":"Requested interval increased to keep the query bounded","intervalMillis":chosen}));
            }
            params["fixed_interval"] = json!(format!("{chosen}ms"));
            if let Some(map) = params.as_object_mut() {
                map.remove("interval");
                map.remove("unit");
                map.remove("calendar_interval");
            }
        }
        if params["min_doc_count"].as_i64().unwrap_or(0) == 0 && params.get("extended_bounds").is_none() {
            params["extended_bounds"] = json!({"min":from,"max":thru.saturating_sub(1)});
        }
        params["range_start"] = json!(from);
        params["range_end"] = json!(thru);
        if params.get("drop_partial").is_none() {
            params["drop_partial"] = json!(true);
        }
        Ok(())
    }
    if let Some(items) = options["aggregations"].as_array_mut() {
        for item in items {
            if item["type"] == "date_histogram" {
                visit(&mut item["params"], from, thru, width, notices)?;
            }
        }
    }
    fn tree(value: &mut Json, from: i64, thru: i64, width: i64, notices: &mut Vec<Json>) -> Result<(), Error> {
        if let Some(map) = value.as_object_mut() {
            for (key, value) in map {
                if key == "date_histogram" {
                    visit(value, from, thru, width, notices)?;
                } else {
                    tree(value, from, thru, width, notices)?;
                }
            }
        }
        Ok(())
    }
    if let Some(aggs) = options.get_mut("aggs") {
        tree(aggs, from, thru, width, notices)?;
    }
    Ok(())
}

fn compile_aggregations(options: &Json) -> Result<IndexMap<String, search::Agg>, Error> {
    let mut nested = serde_json::Map::new();
    let mut metrics = serde_json::Map::new();
    if let Some(items) = options["aggregations"].as_array() {
        for item in items.iter().rev() {
            let id = text(item, "id", "");
            let kind = text(item, "type", "");
            if id.is_empty() || kind.is_empty() {
                return Err(fail("report aggregation needs id and type"));
            }
            let mut body = json!({kind:item.get("params").cloned().unwrap_or_else(||json!({}))});
            if text(item, "role", "metric") == "metric" {
                metrics.insert(id.into(), body);
            } else {
                nested.extend(std::mem::take(&mut metrics));
                if !nested.is_empty() {
                    body["aggs"] = Json::Object(std::mem::take(&mut nested));
                }
                nested.insert(id.into(), body);
            }
        }
    }
    nested.extend(metrics);
    let mut aggs = options.get("aggs").cloned().unwrap_or(Json::Object(nested));
    if options["subtotals"] == true {
        add_subtotal_metrics(&mut aggs)?;
    }
    search::Agg::map_from_value(&moqui_lib::json::from_json(&aggs))
}

fn add_subtotal_metrics(value: &mut Json) -> Result<serde_json::Map<String, Json>, Error> {
    let Some(aggs) = value.as_object_mut() else {
        return Err(fail("report aggregations must be a map"));
    };
    let mut projected = serde_json::Map::new();
    for (name, aggregation) in aggs.iter_mut() {
        let bucket = ["terms", "date_histogram", "histogram", "range", "date_range", "filters"]
            .iter()
            .any(|kind| aggregation.get(*kind).is_some());
        let child = aggregation
            .get_mut("aggs")
            .map(add_subtotal_metrics)
            .transpose()?
            .unwrap_or_default();
        if bucket {
            for (key, metric) in child {
                if projected.get(&key).is_some_and(|existing| existing != &metric) {
                    return Err(fail(format!(
                        "subtotal metric {key} has conflicting definitions at different grains"
                    )));
                }
                projected.insert(key, metric);
            }
        } else {
            let mut metric = aggregation.clone();
            if metric.get("aggs").is_some() {
                metric["aggs"] = Json::Object(child);
            }
            projected.insert(name.clone(), metric);
        }
    }
    for (name, metric) in &projected {
        aggs.entry(name.clone()).or_insert_with(|| metric.clone());
    }
    Ok(projected)
}

pub fn list_datasets(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, Error> {
    let mut items = Vec::new();
    for id in [
        "otel_spans",
        "otel_logs",
        "otel_metrics",
        "otel_errors",
        "monitor_annotations",
    ] {
        if let Ok(dataset) = dataset(ec, id) {
            items.push(json!({"id":id,"timeField":dataset.time_field}));
        }
    }
    for definition in definitions::list(ec, Some("dataset"))? {
        let id = text(&definition, "id", "");
        if let Ok(dataset) = dataset(ec, id) {
            items.push(json!({"id":id,"title":definition["title"],"timeField":dataset.time_field}));
        }
    }
    if let Some(entity) = ec.factory().entity() {
        let rows = metadata(ec, || entity.find("moqui.entity.document.DataDocument")?.list())?;
        let mut indexes = std::collections::BTreeSet::new();
        for row in rows.iter() {
            if let Some(index) = row.get("indexName").and_then(Value::as_str) {
                indexes.insert(index.to_owned());
            }
        }
        for index in indexes {
            let id = format!("index:{index}");
            if let Ok(dataset) = dataset(ec, &id) {
                items.push(json!({"id":id,"title":index,"timeField":dataset.time_field}));
            }
        }
    }
    Ok(output(json!({"datasets":items})))
}

pub fn get_dataset(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let id = text(&params, "datasetId", "");
    let dataset = dataset(ec, id)?;
    let facade = search::current()?;
    let mut fields = BTreeMap::new();
    fn collect(prefix: &str, object: &search::mapping::ObjectMapping, fields: &mut BTreeMap<String, Json>) {
        for (name, field) in &object.fields {
            let name = format!("{prefix}{name}");
            fields.insert(name.clone(),json!({"name":name,"type":match field.kind{search::FieldKind::Date=>"time",search::FieldKind::Long|search::FieldKind::Double=>"number",search::FieldKind::Boolean=>"boolean",_=>"string"},"kind":field.kind.search_type(),"aggregatable":field.can_sort(),"searchable":field.kind!=search::FieldKind::Binary}));
        }
        for (name, child) in &object.children {
            if name == search::telemetry::ATTRIBUTE_VALUES {
                continue;
            }
            collect(&format!("{prefix}{name}."), child, fields);
        }
    }
    metadata(ec, || {
        for id in facade.target_documents(&dataset.target)? {
            collect("", &facade.mapping(&id)?.root, &mut fields);
        }
        Ok(())
    })?;
    for (name, config) in dataset_field_defaults(ec, &dataset)? {
        let field = fields.entry(name.clone()).or_insert_with(|| json!({"name":name,"type":config["type"].as_str().unwrap_or("string"),"searchable":false,"aggregatable":false}));
        field["config"] = config;
    }
    Ok(output(
        json!({"dataset":{"id":id,"timeField":dataset.time_field,"fields":fields.into_values().collect::<Vec<_>>(),"capabilities":facade.capabilities().iter().map(|c|c.name()).collect::<Vec<_>>()}}),
    ))
}

pub use alert::evaluate_rules;
pub use annotations::{create_annotation, record_release};
pub use definitions::{
    copy_definition, export_definitions, find_definition_uses, get_definition, get_definition_diff, import_definitions,
    list_definitions, load_shipped, restore_definition, save_definition, versions_definition,
};

/// One rendering path for browser embeds and offline reports. Each panel retains its own
/// diagnostic result so an independent query failure does not suppress the other panels.
pub fn dashboard_frames(
    ec: &ExecutionContext,
    definition: &Json,
    vars: &Json,
    from: &str,
    thru: &str,
) -> Result<Json, Error> {
    dashboard_frames_filtered(ec, definition, vars, from, thru, &json!([]))
}

pub fn dashboard_frames_filtered(
    ec: &ExecutionContext,
    definition: &Json,
    vars: &Json,
    from: &str,
    thru: &str,
    filters: &Json,
) -> Result<Json, Error> {
    let filters = filters
        .as_array()
        .ok_or_else(|| fail("dashboard filters must be a list"))?;
    if filters.len() > 64 {
        return Err(fail("dashboard exceeds 64 filter pills"));
    }
    fn filter_queries(query: &mut Json, filters: &[Json]) -> Result<(), Error> {
        if let Some(queries) = query["queries"].as_array_mut() {
            for query in queries {
                filter_queries(query, filters)?;
            }
            return Ok(());
        }
        for pill in filters {
            if pill["datasetId"]
                .as_str()
                .is_some_and(|dataset| dataset != text(query, "datasetId", "otel_metrics"))
            {
                continue;
            }
            let mut predicate = pill.get("query").cloned().unwrap_or_else(|| pill.clone());
            if let Some(map) = predicate.as_object_mut() {
                for name in ["datasetId", "label", "pinned", "disabled", "negated"] {
                    map.remove(name);
                }
            }
            let predicate = Query::from_value(&moqui_lib::json::from_json(&predicate))?;
            if query["filters"].is_null() {
                query["filters"] = json!([]);
            }
            let list = query["filters"]
                .as_array_mut()
                .ok_or_else(|| fail("query filters must be a list"))?;
            list.push(json!({"query":moqui_lib::json::to_json(&predicate.to_value()),"disabled":pill["disabled"]==true,"negated":pill["negated"]==true,"label":pill["label"],"pinned":pill["pinned"]}));
        }
        Ok(())
    }
    fn variables(value: &mut Json, vars: &Json) {
        match value {
            Json::String(text) => {
                let name = text
                    .strip_prefix("${")
                    .and_then(|s| s.strip_suffix('}'))
                    .or_else(|| text.strip_prefix('$'));
                if let Some(replacement) = name.and_then(|name| vars.get(name)) {
                    *value = replacement.clone();
                }
            }
            Json::Array(items) => {
                for item in items {
                    variables(item, vars);
                }
            }
            Json::Object(map) => {
                for value in map.values_mut() {
                    variables(value, vars);
                }
            }
            _ => {}
        }
    }
    let body = definition.get("body").unwrap_or(definition);
    let mut all_filters = body["filters"].as_array().cloned().unwrap_or_default();
    all_filters.extend_from_slice(filters);
    if all_filters.len() > 64 {
        return Err(fail("dashboard exceeds 64 saved and runtime filter pills"));
    }
    let panels = body["panels"]
        .as_object()
        .ok_or_else(|| fail("dashboard requires a panel map"))?;
    if panels.len() > 100 {
        return Err(fail("dashboard exceeds 100 panels"));
    }
    let now = ec.effective_time().unwrap_or_else(crate::user::now_millis);
    let from = jiff::Timestamp::from_millisecond(time::parse(from, now)?)
        .map_err(fail)?
        .to_string();
    let thru = jiff::Timestamp::from_millisecond(time::parse(thru, now)?)
        .map_err(fail)?
        .to_string();
    let mut resolved = vars.as_object().cloned().unwrap_or_default();
    let variable_definitions = body["variables"].as_array().cloned().unwrap_or_default();
    for variable in &variable_definitions {
        let name = text(variable, "name", "");
        if !resolved.contains_key(name) {
            let mut value = variable.get("default").cloned().unwrap_or_else(|| {
                if variable["includeAll"] == true {
                    json!("$__all")
                } else {
                    variable["options"]
                        .as_array()
                        .and_then(|options| options.first())
                        .cloned()
                        .unwrap_or(Json::Null)
                }
            });
            if value.is_object() {
                value = value["value"].clone();
            }
            if value.is_null() && variable["kind"] == "query" && !text(variable, "field", "").is_empty() {
                let options = execute_query(
                    ec,
                    &json!({"datasetId":variable["datasetId"],"from":from,"thru":thru,"aggregations":[{"id":"option","type":"terms","role":"x-axis","params":{"field":variable["field"],"size":1000}}]}),
                )?;
                value = options["frames"][0]["fields"]
                    .as_array()
                    .and_then(|fields| fields.iter().find(|field| field["name"] == "option"))
                    .and_then(|field| field["values"].as_array())
                    .and_then(|values| values.first())
                    .cloned()
                    .unwrap_or(Json::Null);
            }
            resolved.insert(name.to_owned(), value);
        }
    }
    fn apply_variables(query: &mut Json, definitions: &[Json], values: &serde_json::Map<String, Json>) {
        if let Some(queries) = query["queries"].as_array_mut() {
            for query in queries {
                apply_variables(query, definitions, values);
            }
            return;
        }
        for variable in definitions {
            let Some(value) = values.get(text(variable, "name", "")) else {
                continue;
            };
            let kind = text(variable, "kind", "custom");
            if kind == "constant" || value.is_null() {
                continue;
            }
            let selected = value.as_array().cloned().unwrap_or_else(|| vec![value.clone()]);
            if selected.is_empty() || selected.iter().any(|v| v == "$__all" || v == "") {
                continue;
            }
            if let Some(dataset) = variable["datasetId"].as_str() {
                if query["datasetId"] != dataset {
                    continue;
                }
            }
            if matches!(kind, "groupBy" | "group-by") {
                if let Some(items) = query["aggregations"].as_array_mut() {
                    if let Some(bucket) = items.iter_mut().find(|a| text(a, "role", "metric") != "metric") {
                        bucket["params"]["field"] = selected[0].clone();
                    }
                }
            } else if kind == "interval" {
                if let Some(items) = query["aggregations"].as_array_mut() {
                    for item in items.iter_mut().filter(|a| a["type"] == "date_histogram") {
                        item["params"]["fixed_interval"] = selected[0].clone();
                    }
                }
            } else if let Some(field) = variable["field"].as_str() {
                if query["filters"].is_null() {
                    query["filters"] = json!([]);
                }
                if let Some(filters) = query["filters"].as_array_mut() {
                    filters.push(json!({"field":{"name":field,"op":"in","values":selected}}));
                }
            }
        }
    }
    let resolved = Json::Object(resolved);
    let mut annotations = Vec::new();
    let mut annotation_notices = Vec::new();
    let mut marker_query = json!({"datasetId":"monitor_annotations","from":from,"thru":thru,"pageSize":200,"query":{"bool":{"any":[{"field":{"name":"dashboard","op":"exists","negate":true}},{"field":{"name":"dashboard","op":"eq","value":definition["id"]}}]}}});
    let mut scope_filters = Vec::new();
    for (variable, alternate, scope, field) in [
        ("service", "serviceName", "service", "service.name"),
        ("instance", "instanceId", "instance", "service.instance.id"),
    ] {
        if let Some(value) = resolved.get(variable).or_else(|| resolved.get(alternate)) {
            let values = value.as_array().cloned().unwrap_or_else(|| vec![value.clone()]);
            if !values.is_empty() && !values.iter().any(|v| v == "$__all" || v.is_null() || v == "") {
                scope_filters.push(json!({"bool":{"any":[{"field":{"name":"scope","op":"eq","value":scope,"negate":true}},{"field":{"name":field,"op":"in","values":values}}]}}));
            }
        }
    }
    marker_query["filters"] = json!(scope_filters);
    match documents(ec, &marker_query) {
        Ok(result) => {
            annotations.extend(result["hits"].as_array().into_iter().flatten().map(|hit| {
                let mut marker = hit["body"].clone();
                marker["id"] = hit["id"].clone();
                marker
            }));
            if result["total"].as_u64().unwrap_or(0) > 200 {
                annotation_notices.push(json!({"level":"warning","code":"annotation_limit","message":"Showing the latest 200 change markers"}));
            }
        }
        Err(error) => {
            tracing::debug!(%error,"dashboard annotations unavailable for viewer");
            annotation_notices.push(json!({"level":"info","code":"annotations_unavailable","message":"Change markers are unavailable for this viewer"}));
        }
    }
    for source in body["annotations"].as_array().into_iter().flatten() {
        let mut query = source
            .get("query")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| source.clone());
        query["from"] = json!(from);
        query["thru"] = json!(thru);
        query["pageSize"] = json!(200);
        match documents(ec, &query) {
            Ok(result) => {
                for hit in result["hits"].as_array().into_iter().flatten() {
                    let body = &hit["body"];
                    annotations.push(json!({"id":hit["id"],"time":body[text(source,"timeField","time")],"time_end":body[text(source,"timeEndField","time_end")],"text":body[text(source,"textField","text")],"source":"query"}));
                }
            }
            Err(error) => {
                tracing::error!(%error,"independent dashboard annotation query failed");
                annotation_notices.push(
                    json!({"level":"warning","code":"annotation_error","message":"An annotation query could not run"}),
                );
            }
        }
    }
    let mut results = serde_json::Map::new();
    for (id, panel) in panels {
        let panel = if let Some(reference) = panel["panelRef"].as_str() {
            match definitions::read(ec, reference){Ok(definition)=>definition,
                Err(error)=>{results.insert(id.clone(),json!({"title":id,"frames":[],"notices":[{"level":"error","complete":false,"message":error.message()}]}));continue;}}
                .get("body")
                .cloned()
                .ok_or_else(|| fail("library panel requires a body"))?
        } else {
            panel.clone()
        };
        let mut frames = Vec::new();
        let mut notices = Vec::new();
        let mut inspection = Vec::new();
        let mut requests = Vec::new();
        if text(&panel, "visualization", text(&panel, "type", "")) == "text" {
            // Static content has no data source and remains renderable during a telemetry outage.
        } else if panel["queries"].as_array().is_none_or(Vec::is_empty) {
            notices.push(json!({"level":"error","code":"missing_query","complete":false,"message":"A data panel requires at least one query"}));
        } else {
            let panel_from = panel["time"]["from"]
                .as_str()
                .or_else(|| panel["timeFrom"].as_str())
                .unwrap_or(&from);
            let panel_thru = panel["time"]["thru"].as_str().unwrap_or(&thru);
            let mut panel_from = time::parse(panel_from, now)?;
            let mut panel_thru = time::parse(panel_thru, now)?;
            if let Some(shift) = panel["time"]["shift"].as_str().or_else(|| panel["timeShift"].as_str()) {
                let shift = if shift.starts_with("now") {
                    shift.to_owned()
                } else {
                    format!("now-{shift}")
                };
                panel_from = time::parse(&shift, panel_from)?;
                panel_thru = time::parse(&shift, panel_thru)?;
            }
            let mut query = json!({"queries":panel["queries"],"transformations":panel["transformations"],"from":jiff::Timestamp::from_millisecond(panel_from).map_err(fail)?.to_string(),"thru":jiff::Timestamp::from_millisecond(panel_thru).map_err(fail)?.to_string()});
            query["comparePrevious"] =
                json!(panel["comparePrevious"] == true || panel["time"]["comparePrevious"] == true);
            query["inspect"] = json!(panel["inspect"] == true || body["inspect"] == true);
            variables(&mut query, &resolved);
            apply_variables(&mut query, &variable_definitions, resolved.as_object().unwrap());
            filter_queries(&mut query, &all_filters)?;
            match execute_query(ec, &query) {
                Ok(result) => {
                    frames.extend(result["frames"].as_array().into_iter().flatten().cloned());
                    notices.extend(result["notices"].as_array().into_iter().flatten().cloned());
                    inspection.extend(result["inspection"].as_array().into_iter().flatten().cloned());
                    requests.extend(result["requests"].as_array().into_iter().flatten().cloned());
                }
                Err(error) => {
                    tracing::error!(panel=%id,error=%error,"report panel query failed");
                    notices.push(json!({"level":"error","complete":false,"message":error.message()}));
                }
            }
        }
        let mut rendered = panel;
        rendered["frames"] = json!(frames);
        rendered["notices"] = json!(notices);
        rendered["annotations"] = json!(annotations);
        rendered["inspection"] = json!(inspection);
        rendered["requests"] = json!(requests);
        results.insert(id.clone(), rendered);
    }
    Ok(
        json!({"title":definition["title"],"layout":body["layout"],"panels":results,"annotations":annotations,"notices":annotation_notices,"variableValues":resolved,"filters":filters,"range":{"from":from,"thru":thru}}),
    )
}

pub fn render_dashboard(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let mut definition = definitions::read(ec, text(&params, "definitionId", ""))?;
    if params["inspect"] == true {
        definition["body"]["inspect"] = json!(true);
    }
    if let Some(panel) = params["panelId"].as_str().filter(|p| !p.is_empty()) {
        let selected = definition["body"]["panels"]
            .get(panel)
            .cloned()
            .ok_or_else(|| fail("dashboard panel does not exist"))?;
        definition["body"]["panels"] = json!({panel:selected});
    }
    dashboard_frames_filtered(
        ec,
        &definition,
        &params["variables"],
        text(&params, "from", "now-7d"),
        text(&params, "thru", "now"),
        params.get("filters").unwrap_or(&json!([])),
    )
    .map(|dashboard| output(json!({"dashboard":dashboard})))
}

pub fn register(registry: &mut crate::native::NativeRegistry) -> Result<(), crate::context::Error> {
    use crate::native::NativeService;
    for (name, function) in [
        ("query_documents", query_documents as NativeService),
        ("run_query", run_query),
        ("list_datasets", list_datasets),
        ("get_dataset", get_dataset),
        ("get_definition", get_definition),
        ("list_definitions", list_definitions),
        ("save_definition", save_definition),
        ("copy_definition", copy_definition),
        ("versions_definition", versions_definition),
        ("restore_definition", restore_definition),
        ("export_definitions", export_definitions),
        ("import_definitions", import_definitions),
        ("render_dashboard", render_dashboard),
        ("evaluate_rules", evaluate_rules),
        ("create_annotation", create_annotation),
        ("find_definition_uses", find_definition_uses),
        ("get_definition_diff", get_definition_diff),
    ] {
        registry.service_function(&format!("moqui_core::report::{name}"), function)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtotals_preserve_filter_scope_and_native_metric_calculation() {
        let mut aggs = json!({"region":{"terms":{"field":"region"},"aggs":{"paid":{"filter":{"term":{"status":"paid"}},"aggs":{"product":{"terms":{"field":"product"},"aggs":{"average":{"avg":{"field":"amount"}}}}}}}}});
        add_subtotal_metrics(&mut aggs).unwrap();
        assert_eq!(
            aggs["region"]["aggs"]["paid"]["aggs"]["average"]["avg"]["field"],
            "amount"
        );
        assert_eq!(aggs["paid"]["filter"]["term"]["status"], "paid");
        assert!(aggs["paid"]["aggs"].get("product").is_none());
        assert!(
            aggs.get("average").is_none(),
            "a filtered metric may not become an unfiltered total"
        );
    }
    #[test]
    fn native_boundary_preserves_nanoseconds_as_decimal_text() {
        let result = output(json!({"hits":[{"start_nano":1_800_000_000_000_000_001_i64,"count":3}]}));
        let result = input(&result);
        assert_eq!(result["hits"][0]["start_nano"], "1800000000000000001");
        assert_eq!(result["hits"][0]["count"], 3);
    }
}
