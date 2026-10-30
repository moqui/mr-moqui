//! Authorized change markers and immutable build-release identities.
use super::{fail, input, json, output, search, text, time, Error, ExecutionContext, Parameters, ServiceInfo, Value};

pub fn create_annotation(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    if !moqui_core::user::has_permission("MONITOR_INGEST").map_err(fail)? {
        moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset.monitor_annotations", "create")
            .map_err(fail)?;
    }
    let options = input(params);
    let now = ec.effective_time().unwrap_or_else(moqui_core::user::now_millis);
    let time = time::parse(text(&options, "time", "now"), now)?;
    let source = text(&options, "source", "user");
    if !matches!(source, "deploy" | "config" | "release" | "user") {
        return Err(fail("annotation source must be deploy, config, release or user"));
    }
    let message = text(&options, "text", "");
    if message.is_empty() || message.len() > 8192 {
        return Err(fail("annotation text must contain 1 to 8192 bytes"));
    }
    let tags = options["tags"].as_array().cloned().unwrap_or_default();
    if tags.len() > 32 || tags.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 128)) {
        return Err(fail("annotation supports 32 string tags of at most 128 bytes"));
    }
    let id = format!("marker-{}", moqui_core::user::random_string(24).map_err(fail)?);
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
    let annotation = json!({"_id":id,"time":jiff::Timestamp::now().to_string(),"source":"release","scope":"service","service.name":resource.service_name,"service.instance.id":resource.instance_id,"service.version":resource.version,"vcs.ref.head.revision":resource.revision,"attribution_uncertain":resource.attribution_uncertain,"text":format!("{} {} ({})",resource.service_name,resource.version,resource.revision),"tags":["release"]});
    let Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
        unreachable!()
    };
    search.index_entries("monitor_annotations", vec![document])?;
    Ok(())
}
