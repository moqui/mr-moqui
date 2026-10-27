//! Dashboard reports use the existing scheduling/delivery records and the authorized frame executor.
use crate::{
    context::ExecutionContext,
    native::{NativeRegistry, Parameters, ServiceInfo},
};
use base64::Engine;
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
const SCHEDULE: &str = "moqui.screen.ScreenScheduled";
const LOCK: &str = "moqui.screen.ScreenScheduledLock";
fn fail(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}
fn text<'a>(p: &'a Parameters, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("")
}
fn jtext<'a>(p: &'a Json, k: &str, default: &'a str) -> &'a str {
    p[k].as_str().unwrap_or(default)
}
fn viewer() -> Result<String, Error> {
    crate::user::current_user_id()?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("an authenticated report owner is required"))
}
fn entity(ec: &ExecutionContext) -> Result<crate::entity::EntityFacade, Error> {
    ec.factory().entity().ok_or_else(|| fail("entity facade unavailable"))
}
fn out(value: Json) -> Parameters {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => Parameters::new(),
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn register_fn(
    registry: &mut NativeRegistry,
    name: &str,
    function: crate::native::NativeService,
) -> Result<(), crate::context::Error> {
    registry.service_function(&format!("moqui_core::report_schedule::{name}"), function)
}
pub fn register(registry: &mut NativeRegistry) -> Result<(), crate::context::Error> {
    for (name, f) in [
        ("create_schedule", create_schedule as crate::native::NativeService),
        ("list_schedules", list_schedules),
        ("pause_schedule", pause_schedule),
        ("preview_report", preview_report),
        ("send_report", send_report),
        ("run_scheduled", run_scheduled),
    ] {
        register_fn(registry, name, f)?;
    }
    Ok(())
}
fn create_schedule(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let owner = viewer()?;
    let id = text(p, "definitionId");
    let definition = crate::report::definitions::read(ec, id)?;
    if !matches!(jtext(&definition, "type", ""), "report" | "dashboard") {
        return Err(fail("a schedule requires a dashboard or report definition"));
    }
    let cron = text(p, "cronExpression");
    croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Required)
        .build()
        .parse(cron)
        .map_err(fail)?;
    if text(p, "emailTemplateId").is_empty() && text(p, "saveToLocation").is_empty() {
        return Err(fail("a report schedule needs an email template or save location"));
    }
    if !text(p, "saveToLocation").is_empty() {
        require_file_delivery()?;
    }
    let mode = if text(p, "renderMode").is_empty() {
        "pdf"
    } else {
        text(p, "renderMode")
    };
    validate_mode(mode)?;
    let facade = entity(ec)?;
    let _metadata = ec.disable_authz();
    let mut row = facade.make_value(SCHEDULE)?;
    row.set("screenPath", Value::string("/apps/dashboards/Reports"));
    row.set("reportDefinitionId", Value::string(id));
    row.set("reportOwnerId", Value::string(owner));
    row.set("paused", Value::string("N"));
    row.set("noResultsAbort", Value::string("Y"));
    row.set("renderMode", Value::string(mode));
    row.set("cronExpression", Value::string(cron));
    for key in [
        "emailTemplateId",
        "userId",
        "userGroupId",
        "saveToLocation",
        "emailSubject",
    ] {
        if let Some(value) = p.get(key) {
            row.set(key, value.clone());
        }
    }
    row.set_sequenced_id_primary(&facade)?;
    row.create(&facade)?;
    let mut schedule_lock = facade.make_value(LOCK)?;
    schedule_lock.set(
        "screenScheduledId",
        row.get("screenScheduledId").cloned().unwrap_or(Value::Null),
    );
    schedule_lock.create(&facade)?;
    Ok(BTreeMap::from([(
        "screenScheduledId".into(),
        row.get("screenScheduledId").cloned().unwrap_or(Value::Null),
    )]))
}
fn list_schedules(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let owner = viewer()?;
    let _metadata = ec.disable_authz();
    let rows = entity(ec)?
        .find(SCHEDULE)?
        .use_cache(false)
        .eq("reportOwnerId", Value::string(owner))
        .list()?;
    let rows:Vec<Json>=rows.iter().map(|row|json!({"screenScheduledId":row.get("screenScheduledId").map(moqui_lib::json::to_json),"definitionId":row.get("reportDefinitionId").map(moqui_lib::json::to_json),"cronExpression":row.get("cronExpression").map(moqui_lib::json::to_json),"renderMode":row.get("renderMode").map(moqui_lib::json::to_json),"paused":row.get("paused").and_then(Value::as_str)==Some("Y")})).collect();
    Ok(out(json!({"schedules":rows})))
}
fn owned(ec: &ExecutionContext, id: &str) -> Result<crate::entity::EntityValue, Error> {
    let owner = viewer()?;
    let _metadata = ec.disable_authz();
    entity(ec)?
        .find(SCHEDULE)?
        .use_cache(false)
        .eq("screenScheduledId", Value::string(id))
        .one()?
        .filter(|row| row.get("reportOwnerId").and_then(Value::as_str) == Some(owner.as_str()))
        .ok_or_else(|| fail("report schedule not found for this owner"))
}
fn pause_schedule(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let mut row = owned(ec, text(p, "screenScheduledId"))?;
    let paused = p
        .get("paused")
        .is_some_and(|v| matches!(v, Value::Bool(true)) || v.as_str() == Some("true") || v.as_str() == Some("Y"));
    row.set("paused", Value::string(if paused { "Y" } else { "N" }));
    let _metadata = ec.disable_authz();
    row.update(&entity(ec)?)?;
    Ok(Parameters::new())
}
fn require_file_delivery() -> Result<(), Error> {
    if crate::user::has_permission("REPORT_SAVE_LOCATION")? {
        Ok(())
    } else {
        Err(fail("REPORT_SAVE_LOCATION permission is required for report files"))
    }
}
fn validate_mode(mode: &str) -> Result<(), Error> {
    if matches!(mode, "pdf" | "png" | "csv" | "xlsx" | "html") {
        Ok(())
    } else {
        Err(fail("report format must be pdf, png, csv, xlsx or html"))
    }
}

struct Rendered {
    bytes: Vec<u8>,
    mime: &'static str,
    extension: &'static str,
    empty: bool,
    html: String,
    title: String,
    link: String,
    /// Data rows across all frames, for the export audit.
    rows: usize,
}
fn render(
    ec: &ExecutionContext,
    id: &str,
    mode: &str,
    skip_empty: bool,
    selection: Option<&Parameters>,
) -> Result<Rendered, Error> {
    validate_mode(mode)?;
    let definition = crate::report::definitions::read(ec, id)?;
    let panel_id = selection.map(|parameters| text(parameters, "panelId")).unwrap_or("");
    if !panel_id.is_empty() && definition["type"] == "report" {
        return Err(fail("panel export requires a dashboard definition"));
    }
    let specs = if definition["type"] == "report" {
        definition["body"]["dashboards"]
            .as_array()
            .cloned()
            .ok_or_else(|| fail("report needs dashboards"))?
    } else {
        vec![json!({"dashboardRef":id})]
    };
    let mut dashboards = Vec::new();
    let mut link = String::new();
    for spec in specs {
        let mut dashboard =
            crate::report::definitions::read(ec, jtext(&spec, "dashboardRef", jtext(&spec, "definitionId", "")))?;
        if !dashboard["body"].is_object() {
            return Err(fail("dashboard body must be an object"));
        }
        if let Some(layout) = spec.get("layout").filter(|value| !value.is_null()) {
            if !layout.is_array() && !layout.is_object() {
                return Err(fail("report selection layout must be an object or list"));
            }
            dashboard["body"]["layout"] = layout.clone();
        }
        if !panel_id.is_empty() {
            crate::report::select_panel(&mut dashboard, panel_id)?;
        }
        let from = selection
            .map(|parameters| text(parameters, "from"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| jtext(&spec, "from", "now-7d"));
        let thru = selection
            .map(|parameters| text(parameters, "thru"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| jtext(&spec, "thru", "now"));
        let variables = selection
            .and_then(|parameters| parameters.get("variables"))
            .map(moqui_lib::json::to_json)
            .filter(Json::is_object)
            .unwrap_or_else(|| spec["variables"].clone());
        let mut filters = match spec.get("filters") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(filters)) => filters.clone(),
            _ => return Err(fail("report selection filters must be a list")),
        };
        if let Some(selected) = selection.and_then(|parameters| parameters.get("filters")) {
            let selected = moqui_lib::json::to_json(selected);
            if !selected.is_null() {
                filters.extend(
                    selected
                        .as_array()
                        .ok_or_else(|| fail("report filters must be a list"))?
                        .iter()
                        .cloned(),
                );
            }
        }
        let rendered =
            crate::report::dashboard_frames_filtered(ec, &dashboard, &variables, from, thru, &Json::Array(filters))?;
        if link.is_empty() {
            link = live_link(jtext(&dashboard, "id", id), &rendered);
        }
        dashboards.push(rendered);
    }
    if dashboards.iter().any(|dashboard| {
        dashboard["panels"].as_object().is_some_and(|panels| {
            panels.values().any(|panel| {
                panel["notices"].as_array().is_some_and(|notices| {
                    notices
                        .iter()
                        .any(|notice| notice["complete"] == false || notice["level"] == "error")
                })
            })
        })
    }) {
        return Err(fail("report has incomplete or unavailable panel data"));
    }
    let frames: Vec<Json> = dashboards
        .iter()
        .flat_map(|d| {
            d["panels"]
                .as_object()
                .into_iter()
                .flat_map(|p| p.values())
                .flat_map(|p| p["frames"].as_array().into_iter().flatten().cloned())
        })
        .collect();
    for frame in &frames {
        let fields = frame["fields"]
            .as_array()
            .ok_or_else(|| fail("report frame fields are unavailable"))?;
        let mut length = None;
        for field in fields {
            let values = field["values"]
                .as_array()
                .ok_or_else(|| fail("report frame values are unavailable"))?;
            if length.is_some_and(|length| length != values.len()) {
                return Err(fail("report frame columns have inconsistent lengths"));
            }
            length = Some(values.len());
        }
    }
    let empty = frames_empty(&frames);
    let rows = frames
        .iter()
        .map(|frame| {
            frame["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|field| field["values"].as_array().map_or(0, Vec::len))
                .max()
                .unwrap_or(0)
        })
        .sum();
    if empty && skip_empty {
        return Ok(Rendered {
            bytes: Vec::new(),
            mime: "application/octet-stream",
            extension: "",
            empty: true,
            html: String::new(),
            title: jtext(&definition, "title", "Report").to_string(),
            link,
            rows,
        });
    }
    let title = jtext(&definition, "title", "Report");
    let html = print_document(ec, title, &dashboards, &definition["body"])?;
    let (bytes, mime, extension) = match mode {
        "csv" => (csv_frames(&frames).into_bytes(), "text/csv", "csv"),
        "xlsx" => (
            xlsx_frames(&frames)?,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xlsx",
        ),
        "png" => (
            crate::screen::PdfRenderer::render_png(&html).map_err(fail)?,
            "image/png",
            "png",
        ),
        "html" => (html.clone().into_bytes(), "text/html", "html"),
        _ => (
            crate::screen::PdfRenderer::render(&html).map_err(fail)?,
            "application/pdf",
            "pdf",
        ),
    };
    Ok(Rendered {
        bytes,
        mime,
        extension,
        empty,
        html,
        title: title.to_string(),
        link,
        rows,
    })
}
fn live_link(id: &str, dashboard: &Json) -> String {
    fn component(value: &str) -> String {
        let mut out = String::new();
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                out.push(char::from(byte));
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{byte:02X}");
            }
        }
        out
    }
    let mut link = format!("/apps/dashboards/Dashboard?definitionId={}", component(id));
    for key in ["from", "thru"] {
        if let Some(value) = dashboard["range"][key].as_str() {
            link.push_str(&format!("&{key}={}", component(value)));
        }
    }
    if let Some(variables) = dashboard["variableValues"].as_object() {
        for (name, value) in variables {
            for value in value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_else(|| std::slice::from_ref(value))
            {
                if value.is_null() {
                    continue;
                }
                link.push_str(&format!(
                    "&{}={}",
                    component(&format!("var-{name}")),
                    component(&value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()))
                ));
            }
        }
    }
    if let Some(filters) = dashboard["filters"].as_array().filter(|filters| !filters.is_empty()) {
        link.push_str(&format!(
            "&filtersJson={}",
            component(&Json::Array(filters.clone()).to_string())
        ));
    }
    let root = crate::web::webapp_root_url("webroot", true, None);
    format!("{}{link}", root.trim_end_matches('/'))
}
fn frames_empty(frames: &[Json]) -> bool {
    frames.iter().all(|frame| {
        frame["fields"].as_array().is_none_or(|fields| {
            fields.iter().all(|field| {
                field["values"]
                    .as_array()
                    .is_none_or(|values| values.iter().all(Json::is_null))
            })
        })
    })
}
fn csv_cell(v: &Json) -> String {
    let text = if let Some(s) = v.as_str() {
        s.to_owned()
    } else if v.is_null() {
        String::new()
    } else {
        v.to_string()
    };
    let text = if v.is_string() && text.trim_start().starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text
    };
    format!("\"{}\"", text.replace('"', "\"\""))
}
fn csv_frames(frames: &[Json]) -> String {
    let mut out = String::new();
    for frame in frames {
        let Some(fields) = frame["fields"].as_array() else {
            continue;
        };
        out.push_str(
            &fields
                .iter()
                .map(|f| csv_cell(&f["name"]))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
        let len = fields
            .iter()
            .map(|f| f["values"].as_array().map_or(0, Vec::len))
            .max()
            .unwrap_or(0);
        for row in 0..len {
            out.push_str(
                &fields
                    .iter()
                    .map(|f| csv_cell(&f["values"][row]))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
    }
    out
}
#[cfg(feature = "xlsx")]
fn xlsx_frames(frames: &[Json]) -> Result<Vec<u8>, Error> {
    let mut book = rust_xlsxwriter::Workbook::new();
    for (index, frame) in frames.iter().enumerate() {
        let sheet = book.add_worksheet();
        sheet.set_name(format!("Table{}", index + 1)).map_err(fail)?;
        if let Some(fields) = frame["fields"].as_array() {
            for (col, field) in fields.iter().enumerate() {
                let col = u16::try_from(col).map_err(fail)?;
                sheet.write_string(0, col, jtext(field, "name", "")).map_err(fail)?;
                if let Some(values) = field["values"].as_array() {
                    for (row, value) in values.iter().enumerate() {
                        let row = u32::try_from(row + 1).map_err(fail)?;
                        if let Some(number) = value.as_f64() {
                            sheet.write_number(row, col, number).map_err(fail)?;
                        } else {
                            sheet
                                .write_string(row, col, value.as_str().unwrap_or(""))
                                .map_err(fail)?;
                        }
                    }
                }
            }
        }
    }
    if frames.is_empty() {
        book.add_worksheet();
    }
    book.save_to_buffer().map_err(fail)
}
#[cfg(not(feature = "xlsx"))]
fn xlsx_frames(_: &[Json]) -> Result<Vec<u8>, Error> {
    Err(fail("xlsx is not built into this host"))
}
fn print_document(ec: &ExecutionContext, title: &str, dashboards: &[Json], options: &Json) -> Result<String, Error> {
    let resource = ec.factory().resource();
    let mut scripts = String::new();
    for path in [
        "component://webroot/screen/webroot/libs/vue/vue.runtime.global.prod.js",
        "component://webroot/screen/webroot/libs/chartjs/chart.umd.min.js",
        "component://webroot/screen/webroot/libs/marked/marked.umd.min.js",
        "component://webroot/screen/webroot/libs/dompurify/purify.min.js",
        "component://webroot/screen/webroot/js/WebrootReport.js",
    ] {
        let bytes = resource
            .location_bytes(path)?
            .ok_or_else(|| fail(format!("missing report renderer {path}")))?;
        let source = String::from_utf8(bytes)
            .map_err(fail)?
            .replace("</script", "<\\/script");
        scripts.push_str(&format!("<script>{source}</script>"));
    }
    let renderer = resource
        .location_bytes("component://framework/template/report/PrintReport.js")?
        .ok_or_else(|| fail("missing offline print renderer"))?;
    let renderer = String::from_utf8(renderer)
        .map_err(fail)?
        .replace("</script", "<\\/script");
    let data = serde_json::to_string(&json!({"title":title,"dashboards":dashboards,
        "rows":options["layout"] == "rows" || options["layout"] == "onePanelPerRow",
        "appendix":options["appendix"] == true || options["tableAppendix"] == true}))
    .map_err(fail)?
    .replace('<', "\\u003c");
    let orientation = if options["orientation"] == "landscape" {
        "landscape"
    } else {
        "portrait"
    };
    let paper = if options["paper"] == "a4" { "A4" } else { "letter" };
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="UTF-8"><meta name="moqui-report" content="1"><title>{}</title>
<style>@page{{size:{paper} {orientation};margin:12mm}}:root{{--q-primary:#1976d2;--q-positive:#218838;--q-warning:#b8860b;--q-info:#008c9e;--q-negative:#c62828;--q-secondary:#687580;--q-accent:#8e44ad}}
body{{font:13px sans-serif;color:#222;background:white;color-scheme:light}}section,article{{break-inside:avoid;min-width:0}}.print-dashboard{{break-before:page}}.print-dashboard:first-child{{break-before:auto}}.print-auto{{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}}.print-grid{{display:grid;grid-template-columns:repeat(24,minmax(0,1fr));gap:12px}}.print-chart{{height:260px;overflow:hidden}}canvas{{max-width:100%;height:260px!important}}.m-report-panel{{padding:8px;border:1px solid #ddd;margin-bottom:12px}}.m-report-panel>div:first-child{{font-weight:bold;margin-bottom:8px}}.m-report-panel>div:first-child button,.m-report-panel>div:first-child a{{display:none}}.text-h4{{font-size:28px}}table{{border-collapse:collapse;width:100%;font-size:11px}}th,td{{border:1px solid #ddd;padding:4px;text-align:left;overflow-wrap:anywhere}}pre{{white-space:pre-wrap}}.range,.notice{{color:#555}}.appendix{{break-before:page}}</style>
<script>window.moquiReportRendered=false;window.moqui={{}};window.moquiPrintData={data};window.addEventListener('error',event=>{{window.moquiReportError=String(event.message)}});</script>{scripts}</head><body><h1>{}</h1><div id="report"></div><script>{renderer}</script></body></html>"#,
        escape(title),
        escape(title)
    ))
}

fn preview_report(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let mode = if text(p, "renderMode").is_empty() {
        "pdf"
    } else {
        text(p, "renderMode")
    };
    let result = render(ec, text(p, "definitionId"), mode, false, Some(p))?;
    crate::report::audit_export(
        if text(p, "panelId").is_empty() {
            "report"
        } else {
            "panel"
        },
        result.extension,
        text(p, "definitionId"),
        result.rows,
        &[],
    );
    Ok(out(
        json!({"contentBase64":base64::engine::general_purpose::STANDARD.encode(&result.bytes),"contentType":result.mime,"filename":format!("report.{}",result.extension),"empty":result.empty}),
    ))
}
fn send_report(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let schedule = owned(ec, text(p, "screenScheduledId"))?;
    deliver(ec, &schedule, false)
}
fn run_scheduled(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    // The internal ScreenScheduled runner is the caller; this service is never exposed remotely.
    let _authz = ec.disable_authz();
    let row = entity(ec)?
        .find(SCHEDULE)?
        .eq("screenScheduledId", Value::string(text(p, "screenScheduledId")))
        .one()?
        .ok_or_else(|| fail("schedule not found"))?;
    drop(_authz);
    deliver(ec, &row, true)
}

struct SuspendedGroups(Vec<crate::transaction::TransactionManager>);
impl SuspendedGroups {
    fn resume_all(&mut self) -> Result<(), Error> {
        let mut first = None;
        while let Some(tx) = self.0.pop() {
            if let Err(error) = tx.resume() {
                tracing::error!(%error,"could not resume a report caller transaction");
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
}
impl Drop for SuspendedGroups {
    fn drop(&mut self) {
        let _ = self.resume_all();
    }
}
fn with_suspended_groups<T>(
    facade: &crate::entity::EntityFacade,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let mut suspended = SuspendedGroups(Vec::new());
    for group in facade.inner.groups.values() {
        if group.transactions.suspend()? {
            suspended.0.push(group.transactions.clone());
        }
    }
    if let Some(tx) = facade.sim_transactions() {
        if tx.suspend()? {
            suspended.0.push(tx);
        }
    }
    let result = body();
    let resumed = suspended.resume_all();
    match (result, resumed) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(result), Ok(())) => Ok(result),
    }
}

fn database_now(tx: &crate::transaction::TransactionManager) -> Result<i64, Error> {
    tx.with_connection(|connection| {
        let rows = connection.query("SELECT CURRENT_TIMESTAMP AS report_now", &[])?;
        let value = rows
            .first()
            .and_then(|row| row.get("report_now"))
            .ok_or_else(|| fail("report lease database clock is unavailable"))?;
        crate::user::value_millis(value)
            .or_else(|| crate::user::value_millis(&moqui_lib::object::basic_convert(value, "Timestamp").value))
            .ok_or_else(|| fail("report lease database clock could not be read"))
    })
}

fn owns_lease(row: &crate::entity::EntityValue, token: &str, now: i64) -> bool {
    same_run(row, token)
        && row
            .get("reportRunningUntil")
            .and_then(crate::user::value_millis)
            .is_some_and(|until| until > now)
}

fn same_run(row: &crate::entity::EntityValue, token: &str) -> bool {
    row.get("reportRunId").and_then(Value::as_str) == Some(token)
}

struct ReportLease {
    factory: std::sync::Weak<crate::context::ExecutionContextFactory>,
    id: String,
    token: String,
    owner: String,
    definition: String,
    scheduled: bool,
    stop: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
    lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ReportLease {
    fn start(
        factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
        id: &str,
        token: &str,
        owner: &str,
        definition: &str,
        scheduled: bool,
    ) -> Self {
        let (stop, receiver) = std::sync::mpsc::channel();
        let lost = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let weak = std::sync::Arc::downgrade(factory);
        let run_id = token.to_string();
        let schedule_id = id.to_string();
        let expected_owner = owner.to_string();
        let expected_definition = definition.to_string();
        let failed = std::sync::Arc::clone(&lost);
        let worker=std::thread::Builder::new().name("report-lease".into()).spawn(move||{
            let _suppressed=crate::telemetry::suppress();
            while matches!(receiver.recv_timeout(std::time::Duration::from_secs(30)),Err(std::sync::mpsc::RecvTimeoutError::Timeout)){
                let result=weak.upgrade().ok_or_else(||fail("report factory stopped")).and_then(|factory|renew_lease(&factory,&schedule_id,&run_id,&expected_owner,&expected_definition,scheduled));
                if let Err(error)=result{failed.store(true,std::sync::atomic::Ordering::Release);tracing::error!(schedule=%schedule_id,%error,"report lease renewal failed; further delivery is blocked");break;}
            }
        });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                lost.store(true, std::sync::atomic::Ordering::Release);
                tracing::error!(schedule=id,%error,"report lease worker could not start");
                None
            }
        };
        Self {
            factory: std::sync::Arc::downgrade(factory),
            id: id.into(),
            token: token.into(),
            owner: owner.into(),
            definition: definition.into(),
            scheduled,
            stop,
            worker,
            lost,
        }
    }
    fn check(&self) -> Result<(), Error> {
        if self.lost.load(std::sync::atomic::Ordering::Acquire) {
            return Err(fail("report lease was lost; delivery is blocked"));
        }
        let factory = self.factory.upgrade().ok_or_else(|| fail("report factory stopped"))?;
        renew_lease(
            &factory,
            &self.id,
            &self.token,
            &self.owner,
            &self.definition,
            self.scheduled,
        )
    }
}
impl Drop for ReportLease {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
fn renew_lease(
    factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
    id: &str,
    token: &str,
    owner: &str,
    definition: &str,
    scheduled: bool,
) -> Result<(), Error> {
    let context = factory.new_context();
    let _entered = context.enter();
    let facade = entity(&context)?;
    // Rendering may be long: reauthenticate in a fresh context before every renewal/effect
    // rather than carrying the initial owner's authorization cache through the delivery.
    let account = {
        let _metadata = context.disable_authz();
        facade
            .find("moqui.security.UserAccount")?
            .use_cache(false)
            .eq("userId", Value::string(owner))
            .one()?
            .ok_or_else(|| fail("report owner no longer exists"))?
    };
    crate::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or(""))?;
    crate::report::definitions::read(&context, definition)?;
    let _metadata = context.disable_authz();
    let tx = facade.transactions_for(LOCK)?;
    tx.run(
        crate::transaction::TransactionAttribute::ForceNew,
        Some(std::time::Duration::from_secs(5)),
        || {
            let now = database_now(&tx)?;
            let schedule = facade
                .find(SCHEDULE)?
                .eq("screenScheduledId", Value::string(id))
                .for_update(true)
                .one()?
                .ok_or_else(|| fail("report schedule disappeared"))?;
            if schedule.get("reportOwnerId").and_then(Value::as_str) != Some(owner)
                || schedule.get("reportDefinitionId").and_then(Value::as_str) != Some(definition)
                || scheduled && schedule.get("paused").and_then(Value::as_str) == Some("Y")
            {
                return Err(fail("report schedule changed while rendering; delivery is blocked"));
            }
            let mut lease = facade
                .find(LOCK)?
                .eq("screenScheduledId", Value::string(id))
                .for_update(true)
                .one()?
                .ok_or_else(|| fail("report lease disappeared"))?;
            if !owns_lease(&lease, token, now) {
                return Err(fail("report lease was replaced or expired; delivery is blocked"));
            }
            lease.set("reportRunningUntil", Value::I64(now + 600_000));
            lease.update(&facade)
        },
    )
}
fn deliver(ec: &ExecutionContext, schedule: &crate::entity::EntityValue, scheduled: bool) -> Result<Parameters, Error> {
    let facade = entity(ec)?;
    let id = schedule
        .get("screenScheduledId")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("schedule id missing"))?;
    let tx = facade.transactions_for(LOCK)?;
    let token = crate::entity::uuid_v4()?;
    let expected_owner = if scheduled { None } else { Some(viewer()?) };
    let claimed = tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        let _authz = ec.disable_authz();
        // Lock the existing parent even on the first run, when no lock row has been loaded yet.
        let current = facade
            .find(SCHEDULE)?
            .eq("screenScheduledId", Value::string(id))
            .for_update(true)
            .one()?
            .ok_or_else(|| fail("report schedule disappeared"))?;
        if expected_owner
            .as_deref()
            .is_some_and(|owner| current.get("reportOwnerId").and_then(Value::as_str) != Some(owner))
        {
            return Err(fail("report schedule not found for this owner"));
        }
        if scheduled && current.get("paused").and_then(Value::as_str) == Some("Y") {
            return Ok(None);
        }
        let now = database_now(&tx)?;
        if scheduled
            && (current
                .get("fromDate")
                .and_then(crate::user::value_millis)
                .is_some_and(|from| from > now)
                || current
                    .get("thruDate")
                    .and_then(crate::user::value_millis)
                    .is_some_and(|thru| thru <= now))
        {
            return Ok(None);
        }
        let mut lock = facade
            .find(LOCK)?
            .eq("screenScheduledId", Value::string(id))
            .for_update(true)
            .one()?
            .ok_or_else(|| fail("report schedule has no lease record"))?;
        if scheduled {
            let cron = croner::parser::CronParser::builder()
                .seconds(croner::parser::Seconds::Optional)
                .alternative_weekdays(true)
                .sloppy_ranges(true)
                .build()
                .parse(current.get("cronExpression").and_then(Value::as_str).unwrap_or(""))
                .map_err(fail)?;
            let zoned = jiff::Timestamp::from_millisecond(now)
                .map_err(fail)?
                .to_zoned(crate::service::job_time_zone());
            let Ok(previous) = cron.find_previous_occurrence(&zoned, true) else {
                return Ok(None);
            };
            if lock
                .get("lastRunTime")
                .and_then(crate::user::value_millis)
                .is_some_and(|last| last >= previous.timestamp().as_millisecond())
            {
                return Ok(None);
            }
        }
        if lock.get("reportRunningUntil").is_some_and(|v| {
            moqui_lib::object::to_plain_string(v)
                .parse::<i64>()
                .is_ok_and(|until| until > now)
        }) {
            return Ok(None);
        }
        lock.set("screenScheduledId", Value::string(id));
        lock.set("reportRunningUntil", Value::I64(now + 600_000));
        lock.set("reportRunId", Value::string(&token));
        lock.update(&facade)?;
        Ok(Some((current, now)))
    })?;
    let Some((schedule, now)) = claimed else {
        return Ok(out(json!({"skipped":true,"reason":"report already running"})));
    };
    let lease = ReportLease::start(
        ec.factory(),
        id,
        &token,
        schedule.get("reportOwnerId").and_then(Value::as_str).unwrap_or(""),
        schedule.get("reportDefinitionId").and_then(Value::as_str).unwrap_or(""),
        scheduled,
    );
    let mut attempted = false;
    let result = with_suspended_groups(&facade, || -> Result<Parameters, Error> {
        lease.check()?;
        let owner = schedule
            .get("reportOwnerId")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("report owner missing"))?;
        let account = {
            let _authz = ec.disable_authz();
            facade
                .find("moqui.security.UserAccount")?
                .eq("userId", Value::string(owner))
                .one()?
                .ok_or_else(|| fail("report owner no longer exists"))?
        };
        let context = ec.factory().new_context();
        let _entered = context.enter();
        crate::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or(""))?;
        let definition_id = schedule
            .get("reportDefinitionId")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("report definition missing"))?;
        let rendered = render(
            &context,
            definition_id,
            schedule.get("renderMode").and_then(Value::as_str).unwrap_or("pdf"),
            schedule.get("noResultsAbort").and_then(Value::as_str) == Some("Y"),
            None,
        )?;
        if rendered.empty && schedule.get("noResultsAbort").and_then(Value::as_str) == Some("Y") {
            return Ok(out(json!({"skipped":true,"reason":"empty report"})));
        }
        let mut destination_errors = Vec::new();
        let mut destinations = 0;
        let mut delivered = Vec::new();
        if let Some(location) = schedule
            .get("saveToLocation")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            destinations += 1;
            lease.check()?;
            let written = (|| {
                let permissions = ec.factory().new_context();
                let _entered = permissions.enter();
                crate::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or(""))?;
                require_file_delivery()?;
                attempted = true;
                context.factory().resource().write_bytes(location, &rendered.bytes)
            })();
            match written {
                Ok(()) => delivered.push(format!("file:{location}")),
                Err(error) => {
                    tracing::error!(schedule=id,%error,"report file delivery failed; other destinations continue");
                    destination_errors.push(format!("file delivery: {error}"));
                }
            }
        }
        if let Some(template) = schedule
            .get("emailTemplateId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            destinations += 1;
            lease.check()?;
            let emailed = (|| -> Result<Vec<String>, Error> {
                let mut recipients = Vec::new();
                let _authz = context.disable_authz();
                if let Some(user) = schedule.get("userId").and_then(Value::as_str) {
                    if let Some(account) = facade
                        .find("moqui.security.UserAccount")?
                        .use_cache(false)
                        .eq("userId", Value::string(user))
                        .one()?
                    {
                        if account.get("disabled").and_then(Value::as_str) != Some("Y")
                            && !account
                                .get("terminateDate")
                                .and_then(crate::user::value_millis)
                                .is_some_and(|date| date <= now)
                        {
                            if let Some(address) = account
                                .get("emailAddress")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                            {
                                recipients.push(address.to_owned());
                            }
                        }
                    }
                }
                if let Some(group) = schedule.get("userGroupId").and_then(Value::as_str) {
                    for account in facade
                        .find("moqui.security.UserGroupMemberUser")?
                        .use_cache(false)
                        .eq("userGroupId", Value::string(group))
                        .list()?
                        .iter()
                    {
                        if account
                            .get("fromDate")
                            .and_then(crate::user::value_millis)
                            .is_some_and(|date| date > now)
                            || account
                                .get("thruDate")
                                .and_then(crate::user::value_millis)
                                .is_some_and(|date| date <= now)
                        {
                            continue;
                        }
                        if account.get("disabled").and_then(Value::as_str) != Some("Y")
                            && !account
                                .get("terminateDate")
                                .and_then(crate::user::value_millis)
                                .is_some_and(|date| date <= now)
                        {
                            if let Some(address) = account
                                .get("emailAddress")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                            {
                                recipients.push(address.to_owned());
                            }
                        }
                    }
                }
                drop(_authz);
                recipients.sort();
                recipients.dedup();
                if recipients.is_empty() {
                    return Err(fail("report has no deliverable email recipients"));
                }
                let image = if rendered.extension == "png" {
                    rendered.bytes.clone()
                } else {
                    crate::screen::PdfRenderer::render_png(&rendered.html).map_err(fail)?
                };
                let image_id = "report-image@moqui";
                let inline = Value::Map(indexmap::IndexMap::from([
                    ("contentId".into(), Value::string(image_id)),
                    ("fileName".into(), Value::string("dashboard.png")),
                    ("contentType".into(), Value::string("image/png")),
                    ("contentBytes".into(), Value::Bytes(image)),
                ]));
                let attachment = Value::Map(indexmap::IndexMap::from([
                    (
                        "fileName".into(),
                        Value::string(format!("report.{}", rendered.extension)),
                    ),
                    ("contentType".into(), Value::string(rendered.mime)),
                    ("contentBytes".into(), Value::Bytes(rendered.bytes.clone())),
                ]));
                lease.check()?;
                attempted = true;
                let mail = crate::service::call::sync_checked(
                    "org.moqui.impl.EmailServices.send#EmailTemplate",
                    &BTreeMap::from([
                        ("emailTemplateId".into(), Value::string(template)),
                        ("toAddresses".into(), Value::string(recipients.join(","))),
                        ("attachments".into(), Value::List(vec![attachment])),
                        ("inlineAttachments".into(), Value::List(vec![inline])),
                        ("createEmailMessage".into(), Value::Bool(false)),
                        (
                            "bodyParameters".into(),
                            Value::Map(indexmap::IndexMap::from([
                                ("reportLink".into(), Value::string(&rendered.link)),
                                (
                                    "title".into(),
                                    Value::string(
                                        schedule
                                            .get("emailSubject")
                                            .and_then(Value::as_str)
                                            .filter(|subject| !subject.is_empty())
                                            .unwrap_or(&rendered.title),
                                    ),
                                ),
                                (
                                    "emailSubject".into(),
                                    schedule
                                        .get("emailSubject")
                                        .cloned()
                                        .unwrap_or_else(|| Value::string(&rendered.title)),
                                ),
                                ("reportImageCid".into(), Value::string(image_id)),
                            ])),
                        ),
                    ]),
                    &crate::service::call::CallOptions {
                        ignore_transaction: Some(true),
                        require_new_transaction: Some(true),
                        no_remember_parameters: true,
                        ..Default::default()
                    },
                )?;
                if mail
                    .get("sendError")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
                {
                    return Err(fail("report email delivery failed"));
                }
                if mail.get("messageId").and_then(Value::as_str).is_none_or(str::is_empty) {
                    return Err(fail("report email did not produce a delivery message id"));
                }
                Ok(recipients)
            })();
            match emailed {
                Ok(recipients) => delivered.extend(recipients),
                Err(error) => {
                    tracing::error!(schedule=id,template,%error,"report email delivery failed; other destinations continue");
                    destination_errors.push(format!("email delivery: {error}"));
                }
            }
        }
        if !delivered.is_empty() {
            crate::report::audit_export(
                "scheduled",
                rendered.extension,
                definition_id,
                rendered.rows,
                &delivered,
            );
        }
        if destinations == 0 {
            return Err(fail("report schedule has no delivery destination"));
        }
        if !destination_errors.is_empty() {
            return Err(fail(destination_errors.join("; ")));
        }
        Ok(out(json!({"sent":true})))
    });
    drop(lease);
    let released = tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        let _authz = ec.disable_authz();
        if let Some(mut lock) = facade
            .find(LOCK)?
            .eq("screenScheduledId", Value::string(id))
            .for_update(true)
            .one()?
        {
            if !same_run(&lock, &token) {
                return Ok(());
            }
            lock.set("reportRunningUntil", Value::Null);
            lock.set("reportRunId", Value::Null);
            // Once an external effect was attempted, do not automatically duplicate it in the
            // same cron period. Failed destinations remain visible in the service job outcome.
            if result.is_ok() || attempted {
                lock.set("lastRunTime", Value::I64(now));
            }
            lock.update(&facade)?;
        }
        Ok(())
    });
    if let Err(error) = released {
        tracing::error!(schedule=id,%error,"could not release report schedule lease");
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_keep_rows_and_guard_formulas() {
        let frames =
            vec![json!({"fields":[{"name":"name","values":["=BAD","<name>"]},{"name":"value","values":[3,null]}]})];
        let csv = csv_frames(&frames);
        assert!(csv.contains("\"'=BAD\",\"3\""));
        assert!(csv.contains("\"<name>\",\"\""));
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(csv_cell(&json!(-3)), "\"-3\"");
        assert_eq!(csv_cell(&json!("-3")), "\"'-3\"");
        assert!(frames_empty(&[json!({"fields":[{"name":"value","values":[null]}]})]));
        assert!(!frames_empty(&[json!({"fields":[{"name":"value","values":[0]}]})]));
    }

    #[test]
    fn panel_exports_select_before_querying_and_links_keep_frozen_context() {
        let mut dashboard = json!({"body":{"panels":{"selected":{"queries":[{"datasetId":"public"}]},"other":{"queries":[{"datasetId":"private"}]}},
            "layout":{"type":"grid","items":[{"panelId":"other","x":0,"y":0,"w":12,"h":1},{"panelId":"selected","x":12,"y":0,"w":12,"h":1}]}}});
        crate::report::select_panel(&mut dashboard, "selected").unwrap();
        assert_eq!(dashboard["body"]["panels"].as_object().unwrap().len(), 1);
        assert!(dashboard["body"]["panels"].get("other").is_none());
        assert_eq!(
            dashboard["body"]["layout"],
            json!({"type":"autoGrid","items":[{"panelId":"selected"}]}),
            "the single-panel layout names no other panel"
        );
        assert!(crate::report::select_panel(&mut dashboard, "missing").is_err());
        assert!(crate::report::select_panel(&mut json!({"body":5}), "selected").is_err());
        let link = live_link(
            "id with%value",
            &json!({"range":{"from":"2026-10-26T00:00:00Z","thru":"2026-10-27T00:00:00Z"},"variableValues":{"status":["Approved","Held"],"literal":"%26"},"filters":[{"datasetId":"visible","query":{"field":{"name":"status","op":"eq","value":"Approved"}}}]}),
        );
        assert!(link.contains("definitionId=id%20with%25value"));
        assert!(link.contains("from=2026-10-26T00%3A00%3A00Z"));
        assert!(link.contains("var-literal=%2526"));
        assert!(link.contains("var-status=Approved"));
        assert!(link.contains("var-status=Held"));
        assert!(link.contains("filtersJson=%5B%7B"));
        assert!(link.contains("%22datasetId%22%3A%22visible%22"));
    }

    #[test]
    fn lease_ownership_does_not_clear_a_newer_runner() {
        let facade = crate::testing::open();
        let mut row = facade.make_value(LOCK).unwrap();
        row.set("reportRunId", Value::string("new"));
        row.set("reportRunningUntil", Value::I64(200));
        assert!(!same_run(&row, "old"));
        assert!(same_run(&row, "new"));
        assert!(owns_lease(&row, "new", 100));
        assert!(!owns_lease(&row, "new", 200));
    }

    #[test]
    fn a_paused_schedule_is_reread_after_the_discovery_snapshot() {
        // Test workers are reused: isolate transaction/cache TLS and model discovery and
        // the later pause as separate committed operations, as separate callers do.
        std::thread::spawn(|| {
            let context = crate::context::ExecutionContextFactory::test_context();
            let _entered = context.enter();
            let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let facade = crate::testing::open_with(
                &crate::testing::framework_dir(),
                crate::testing::Options {
                    tape: Some(tape.clone()),
                    ..Default::default()
                },
            );
            context.factory().set_entity(facade.clone());
            let tx = facade.transactions_for(SCHEDULE).unwrap();
            let snapshot = {
                let _authz = context.disable_authz();
                tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                    let mut row = facade.make_value(SCHEDULE)?;
                    row.set("screenScheduledId", Value::string("paused-snapshot"));
                    row.set("paused", Value::string("N"));
                    row.create(&facade)
                })
                .unwrap();
                let snapshot = tx
                    .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                        facade
                            .find(SCHEDULE)?
                            .use_cache(false)
                            .eq("screenScheduledId", Value::string("paused-snapshot"))
                            .one()
                    })
                    .unwrap()
                    .unwrap();
                assert_eq!(snapshot.get("paused").and_then(Value::as_str), Some("N"));
                let changed = tx
                    .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                        facade
                            .find(SCHEDULE)?
                            .eq("screenScheduledId", Value::string("paused-snapshot"))
                            .update_all([("paused".into(), Value::string("Y"))])
                    })
                    .unwrap();
                assert_eq!(changed, 1);
                let current = tx
                    .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                        facade
                            .find(SCHEDULE)?
                            .for_update(true)
                            .eq("screenScheduledId", Value::string("paused-snapshot"))
                            .one()
                    })
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    current.get("paused").and_then(Value::as_str),
                    Some("Y"),
                    "pause must be committed before the stale snapshot is delivered"
                );
                snapshot
            };
            tape.lock().unwrap().clear();
            let result = deliver(&context, &snapshot, true).unwrap();
            assert_eq!(result.get("skipped"), Some(&Value::Bool(true)));
            let statements = tape.lock().unwrap();
            assert!(
                statements
                    .iter()
                    .any(|sql| sql.contains("screen_scheduled") && sql.contains("FOR UPDATE")),
                "{statements:?}"
            );
            assert!(
                !statements.iter().any(|sql| sql.contains("CURRENT_TIMESTAMP")),
                "paused discovery must stop before lease clock/claim: {statements:?}"
            );
        })
        .join()
        .unwrap();
    }
}
