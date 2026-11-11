//! Authorized change markers and immutable build-release identities.
use super::{
    fail, input, json, output, search, text, time, Error, ExecutionContext, Json, Parameters, ServiceInfo, Value,
};

/// The source of a business event, and the artifact its writer holds.
pub(super) const BUSINESS: &str = "business";
const BUSINESS_ARTIFACT: &str = "org.moqui.report.annotation.business";
/// The registered sources of business events: `ReportAnnotationSource` enumerations, each naming (enumCode) a dataset
/// over the annotations a domain scopes with its own row filter.
const SOURCE_TYPE: &str = "ReportAnnotationSource";

pub fn create_annotation(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let options = input(params);
    let source = text(&options, "source", "user");
    // A business event (a price list published, a period closed) is its owner's to write: the artifact
    // org.moqui.report.annotation.business, which a domain service that has checked its own permission passes by writing
    // with authorization off after its commit. MONITOR_INGEST and the telemetry dataset's grant are the technical sources'
    // and write no business event.
    let business = source == BUSINESS;
    if business {
        moqui_core::user::authorize_own("AT_SERVICE", BUSINESS_ARTIFACT, "create").map_err(fail)?;
    } else if !moqui_core::user::has_permission("MONITOR_INGEST").map_err(fail)? {
        moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset.monitor_annotations", "create")
            .map_err(fail)?;
    }
    let now = ec.effective_time().unwrap_or_else(moqui_core::user::now_millis);
    let time = time::parse(text(&options, "time", "now"), now)?;
    if !matches!(source, "deploy" | "config" | "release" | "user" | BUSINESS) {
        return Err(fail(
            "annotation source must be deploy, config, release, user or business",
        ));
    }
    let message = text(&options, "text", "");
    let limit = if business { 255 } else { 8192 };
    if message.is_empty() || message.len() > limit {
        return Err(fail(format!("annotation text must contain 1 to {limit} bytes")));
    }
    let tags = options["tags"].as_array().cloned().unwrap_or_default();
    if tags.len() > 32 || tags.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 128)) {
        return Err(fail("annotation supports 32 string tags of at most 128 bytes"));
    }
    // A business event with a key (its owner's event and record) keeps one marker: writing it again replaces it.
    let id = match options["eventKey"].as_str().filter(|key| business && !key.is_empty()) {
        Some(key) if key.len() > 255 => return Err(fail("annotation eventKey exceeds 255 bytes")),
        Some(key) => format!("business-{}", &moqui_lib::json::canonical_sha256(&json!(key))[..32]),
        None => format!("marker-{}", moqui_core::user::random_string(24).map_err(fail)?),
    };
    let mut annotation = json!({"_id":id,"time":jiff::Timestamp::from_millisecond(time).map_err(fail)?.to_string(),"source":source,"text":moqui_core::telemetry::redact_text(message),"tags":tags,"created_by":moqui_core::user::current_user_id().map_err(fail)?});
    if let Some(end) = options["timeEnd"].as_str() {
        let end = time::parse(end, now)?;
        if end < time {
            return Err(fail("annotation timeEnd precedes time"));
        }
        annotation["time_end"] = json!(jiff::Timestamp::from_millisecond(end).map_err(fail)?.to_string());
    }
    for (name, input) in [
        ("scope", "scope"),
        ("service.name", "serviceName"),
        ("service.instance.id", "instanceId"),
        ("dashboard", "dashboardId"),
        ("panel", "panelId"),
        ("service.version", "version"),
        ("vcs.ref.head.revision", "revision"),
    ] {
        if let Some(value) = options[input].as_str() {
            if value.len() > 512 {
                return Err(fail(format!("annotation {input} exceeds 512 bytes")));
            }
            annotation[name] = json!(moqui_core::telemetry::redact_text(value));
        }
    }
    if business {
        business_fields(&options, &mut annotation)?;
    }
    if let Some(Value::String(source)) = ec.shared_map().get("monitor.ingest.source") {
        annotation["ingest_source"] = json!(source);
    }
    let Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
        unreachable!()
    };
    if search::current()?.index_entries("monitor_annotations", vec![document])? != 1 {
        return Err(fail("annotation could not be indexed"));
    }
    tracing::info!(annotation=%id,source,actor=?moqui_core::user::current_user_id().map_err(fail)?,"monitor annotation created");
    Ok(output(json!({"annotationId":id,"annotation":annotation})))
}

/// The datasets registered as sources of business events (each enumCode of a `ReportAnnotationSource` enumeration). A
/// dashboard over business data reads each of them as its viewer though its definition names none, so a scope
/// fingerprint counts them too (delivery.rs). When they cannot be read, none: logged, and the dashboard goes on.
pub(super) fn source_datasets(ec: &ExecutionContext) -> Vec<String> {
    let sources = super::metadata(ec, || {
        ec.factory()
            .entity()
            .ok_or_else(|| fail("entity facade is unavailable"))?
            .find("moqui.basic.Enumeration")?
            .eq("enumTypeId", Value::string(SOURCE_TYPE))
            .use_cache(true)
            .list()
    });
    match sources {
        Ok(sources) => sources
            .iter()
            .filter_map(|row| row.get("enumCode").and_then(Value::as_str))
            .filter(|dataset| !dataset.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(error) => {
            tracing::error!(%error, "business annotation sources could not be read; continuing without them");
            Vec::new()
        }
    }
}

/// The business events of a time range the viewer may read, for a dashboard over business data: each registered source
/// (a `ReportAnnotationSource` enumeration whose enumCode is a dataset over the annotations) is read as the viewer, so its
/// own artifact and its row filter (the viewer's organizations) decide what shows, and only `business` markers are taken
/// from it whatever the dataset says. A source the viewer may not read, or one that fails, adds nothing; the others go on.
pub(super) fn business_markers(ec: &ExecutionContext, from: &str, thru: &str) -> Vec<Json> {
    let mut markers = Vec::new();
    for dataset in &source_datasets(ec) {
        let query = json!({"datasetId":dataset,"from":from,"thru":thru,"pageSize":200,
            "query":{"field":{"name":"source","op":"eq","value":BUSINESS}}});
        match super::documents(ec, &query) {
            Ok(result) => {
                for hit in result["hits"].as_array().into_iter().flatten() {
                    let body = &hit["body"];
                    let mut marker = json!({"id":hit["id"],"source":BUSINESS});
                    for field in [
                        "time",
                        "time_end",
                        "text",
                        "event",
                        "organizationPartyId",
                        "recordType",
                        "recordId",
                        "link",
                    ] {
                        if !body[field].is_null() {
                            marker[field] = body[field].clone();
                        }
                    }
                    markers.push(marker);
                }
            }
            Err(error) => tracing::debug!(%dataset, %error, "business annotation source not read for this viewer"),
        }
    }
    markers
}

/// The fields of a business event (BOSS impl-insights P6, F13, F31): the organization it belongs to, which the
/// reading dataset's row filter scopes; the event's type; and the record behind it or a link, which the app that shows the
/// marker resolves by the viewer's own permissions. It carries a name and a time, never an amount, and no technical scope.
fn business_fields(options: &Json, annotation: &mut Json) -> Result<(), Error> {
    for name in ["scope", "service.name", "service.instance.id", "dashboard", "panel"] {
        if let Some(map) = annotation.as_object_mut() {
            map.remove(name);
        }
    }
    let organization = text(options, "organizationPartyId", "");
    if organization.is_empty() {
        return Err(fail("a business annotation requires organizationPartyId"));
    }
    let name = |value: &str| {
        !value.is_empty()
            && value.len() <= 255
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b':'))
    };
    for (field, input, required) in [
        ("organizationPartyId", "organizationPartyId", true),
        ("event", "event", true),
        ("recordType", "recordType", false),
        ("recordId", "recordId", false),
    ] {
        match options[input].as_str().filter(|value| !value.is_empty()) {
            Some(value) if name(value) => annotation[field] = json!(value),
            Some(_) => {
                return Err(fail(format!(
                    "annotation {input} must be an id of at most 255 characters"
                )))
            }
            None if required => return Err(fail(format!("a business annotation requires {input}"))),
            None => {}
        }
    }
    if let Some(link) = options["link"].as_str().filter(|link| !link.is_empty()) {
        // a path on this server, never another host
        if link.len() > 512 || !link.starts_with('/') || link.starts_with("//") || link.contains(['\\', '\n', '\r']) {
            return Err(fail(
                "annotation link must be a path on this server of at most 512 bytes",
            ));
        }
        annotation["link"] = json!(link);
    }
    Ok(())
}

/// Startup uses the same resource identity as the SDK. Repeated starts/instances of one build
/// refer to one release marker; a failed telemetry store must never prevent server startup.
pub fn record_release(factory: &moqui_core::context::ExecutionContextFactory) -> Result<(), Error> {
    let telemetry = moqui_core::telemetry::for_factory(factory);
    if !telemetry.config().enabled {
        return Ok(());
    }
    let resource = telemetry.resource();
    let search = search::installed(factory).ok_or_else(|| fail("search is unavailable for release annotation"))?;
    let identity = json!({"service":resource.service_name,"version":resource.version,"revision":resource.revision});
    let id = format!("release-{}", moqui_lib::json::canonical_sha256(&identity));
    if search.get_document("monitor_annotations", &id)?.is_some() {
        return Ok(());
    }
    let mut annotation = json!({"_id":id,"time":jiff::Timestamp::now().to_string(),"source":"release","scope":"service","service.name":resource.service_name,"service.instance.id":resource.instance_id,"service.version":resource.version,"vcs.ref.head.revision":resource.revision,"attribution_uncertain":resource.attribution_uncertain,"text":format!("{} {} ({})",resource.service_name,resource.version,resource.revision),"tags":["release"]});
    if let Some(entity) = factory.entity() {
        annotation["moqui.schema.hash"] = json!(schema_hash(&entity));
    }
    let Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
        unreachable!()
    };
    search.index_entries("monitor_annotations", vec![document])?;
    Ok(())
}

/// A fingerprint of the entity tables, columns and types: two releases with the same one read and write the same
/// schema, which a recovery rollback between them needs (plans/Monitoring.md, Later: Recovery).
pub fn schema_hash(entity: &moqui_core::entity::EntityFacade) -> String {
    use sha2::{Digest, Sha256};
    let mut tables: Vec<String> = entity.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|definition| definition.member_entities.is_empty())
            .map(|definition| {
                let mut columns: Vec<String> = definition
                    .fields
                    .iter()
                    .map(|field| format!("{}:{}", field.column_name, field.field_type))
                    .collect();
                columns.sort();
                format!("{}({})", definition.table_name, columns.join(","))
            })
            .collect()
    });
    tables.sort();
    let mut hash = moqui_lib::codec::hex_encode(&Sha256::digest(tables.join(";").as_bytes()));
    hash.truncate(32);
    hash
}
