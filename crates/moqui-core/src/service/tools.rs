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

//! Rust bodies for the tools services whose Groovy holds bytes or reaches Java internals: the print jobs of
//! `PrintServices.xml`, the data document notifications, and the encrypted field rewrite of `EntityServices.xml`.
//!
//! A print job is a `PrintJob` record sent to its `NetworkPrinter` through the host's IPP client (`crate::print`), the
//! port of `moqui-cups` and its CUPS4J calls. A screen document is rendered as `html-print` and turned into PDF by the
//! host's engine. A send the server does not take, or cannot be reached for, is recorded on the job as `PtjSendFailed`
//! with the message, and the call returns that error.

use std::collections::{BTreeMap, BTreeSet};

use super::util::{has_error, map_text, param_text};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::{Condition, EntityFacade};
use crate::print::{JobAction, PrintDocument, PrintServer, PrinterInfo};
use crate::service::call::{sync_with, CallOptions};

const PRINT_DOCUMENT: &str = "org.moqui.impl.PrintServices.print#Document";
const SEND_INTERNAL: &str = "org.moqui.impl.PrintServices.send#PrintJobInternal";

const PRINT_JOB: &str = "moqui.basic.print.PrintJob";
const NETWORK_PRINTER: &str = "moqui.basic.print.NetworkPrinter";
/// CUPS4J's default port.
const IPP_PORT: u16 = 631;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, print_document)?;
    rust_service!(map, print_resource_document)?;
    rust_service!(map, print_screen_document)?;
    rust_service!(map, send_print_job_internal)?;
    rust_service!(map, get_server_printers)?;
    rust_service!(map, get_print_job_details)?;
    rust_service!(map, hold_print_job)?;
    rust_service!(map, release_print_job)?;
    rust_service!(map, cancel_print_job)?;
    rust_service!(map, send_document_notifications)?;
    rust_service!(map, rewrite_encrypted_fields)?;
    Ok(())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

/// A service call from a body, as an XML `service-call`: a failure is left as a message error.
fn call(name: &str, parameters: &BTreeMap<String, Value>, options: &CallOptions) -> BTreeMap<String, Value> {
    match sync_with(name, parameters, options) {
        Ok(results) => results,
        Err(err) => {
            let message = err.message().to_string();
            ExecutionContext::with_current(|ctx| {
                if !ctx.message().errors().iter().any(|known| known == &message) {
                    ctx.message().add_error(message);
                }
            });
            BTreeMap::new()
        }
    }
}

/// The document bytes: `serialBlob` or `inputStream` (Java's two forms), as bytes or text.
fn document_bytes(parameters: &BTreeMap<String, Value>) -> Option<Vec<u8>> {
    ["serialBlob", "inputStream"]
        .iter()
        .find_map(|name| match parameters.get(*name) {
            Some(Value::Bytes(bytes)) => Some(bytes.to_vec()),
            Some(Value::String(text)) => Some(text.as_bytes().to_vec()),
            _ => None,
        })
}

/// The `print#DocumentInterface` fields of a print job.
fn job_fields(parameters: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    [
        "networkPrinterId",
        "createdDate",
        "username",
        "jobName",
        "copies",
        "duplex",
        "pageRanges",
        "contentType",
    ]
    .iter()
    .filter_map(|name| {
        parameters
            .get(*name)
            .filter(|value| !value.is_null())
            .map(|value| ((*name).to_string(), value.clone()))
    })
    .collect()
}

/// `print#Document`: create the `PrintJob` (not sent) in its own transaction, then send it.
fn print_document(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(bytes) = document_bytes(parameters) else {
        add_error("Both inputStream and serialBlob are null, must specify one or the other");
        return Ok(BTreeMap::new());
    };
    let store = !matches!(parameters.get("storeDocument"), Some(Value::Bool(false)));
    let mut job = job_fields(parameters);
    job.insert("statusId".to_string(), Value::string("PtjNotSent"));
    if store {
        job.insert("document".to_string(), Value::Bytes(bytes.clone()));
    }
    let force_new = CallOptions {
        require_new_transaction: Some(true),
        ..CallOptions::default()
    };
    let created = call(&format!("create#{PRINT_JOB}"), &job, &force_new);
    if has_error() {
        return Ok(BTreeMap::new());
    }
    let print_job_id = created.get("printJobId").cloned().unwrap_or(Value::Null);
    let mut out = BTreeMap::new();
    out.insert("printJobId".to_string(), print_job_id.clone());
    job.insert("printJobId".to_string(), print_job_id);
    job.insert("document".to_string(), Value::Bytes(bytes));
    let mut send = BTreeMap::new();
    send.insert("printJob".to_string(), Value::Map(job.into_iter().collect()));
    let sent = call(SEND_INTERNAL, &send, &CallOptions::default());
    if let Some(job_id) = sent.get("jobId") {
        out.insert("jobId".to_string(), job_id.clone());
    }
    Ok(out)
}

/// `print#ResourceDocument`: the bytes at a resource location, printed and not stored.
fn print_resource_document(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = param_text(parameters, "resourceLocation");
    let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(&location))
        .ok_or_else(|| LibError::new("no execution context is entered"))?;
    let resource = match found {
        Ok(resource) if resource.exists() => resource,
        _ => {
            add_error(format!("Could not find resource at [{location}]"));
            return Ok(BTreeMap::new());
        }
    };
    let bytes = resource.read_bytes().map_err(|err| LibError::new(err.message()))?;
    let mut print = parameters.clone();
    print.insert("inputStream".to_string(), Value::Bytes(bytes));
    if let Some(content_type) = moqui_lib::resource::content_type_for_name(&location) {
        print.insert("contentType".to_string(), Value::string(content_type));
    }
    print.insert("storeDocument".to_string(), Value::Bool(false));
    Ok(call(PRINT_DOCUMENT, &print, &CallOptions::default()))
}

/// `print#ScreenDocument`: render the screen as `html-print` (the `xsl-fo` mode), turn it into PDF, and pass the bytes
/// to `print#Document`, which stores the `PrintJob` and sends it. The screen parameters are `screenParameters` plus the
/// map `screenParametersStr` evaluates to; the job name defaults to the screen name and those parameter values.
fn print_screen_document(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = param_text(parameters, "screenLocation");
    let Some(factory) = ExecutionContext::with_current(|ctx| ctx.factory().clone()) else {
        return Err(LibError::new("no execution context is entered"));
    };
    let screens = crate::screen::ScreenFacade::of(&factory);
    let Ok(def) = screens.screen_def(&factory, &location) else {
        add_error(format!("Screen not found at [{location}]"));
        return Ok(BTreeMap::new());
    };
    // In the order given, as Java's `[:]` (a `LinkedHashMap`), for the job name; a `screenParametersStr` map comes in
    // key order, as `eval_map` returns it.
    let mut screen_parameters = IndexMap::new();
    if let Some(Value::Map(given)) = parameters.get("screenParameters") {
        screen_parameters.extend(given.iter().map(|(name, value)| (name.clone(), value.clone())));
    }
    let expression = param_text(parameters, "screenParametersStr");
    if !expression.is_empty() {
        let evaluated = crate::script::eval_map(&expression)
            .map_err(|err| LibError::new(format!("screenParametersStr: {}", err.message())))?;
        screen_parameters.extend(evaluated);
    }
    let content_type = match param_text(parameters, "contentType") {
        found if found.is_empty() => "application/pdf".to_string(),
        found => found,
    };
    // The PDF renderer makes only PDF, where Java's FOP takes other output types; and the screen renders in this
    // port's one webapp, `webroot` (`webappName`'s default).
    if content_type != "application/pdf" {
        add_error(format!(
            "A screen document prints as application/pdf, not {content_type}"
        ));
        return Ok(BTreeMap::new());
    }
    let rendered = crate::screen::make_render(&factory)
        .root_screen(&location)
        .render_mode("xsl-fo")
        .parameters(screen_parameters.clone().into_iter().collect())
        .render();
    let output = match rendered {
        Ok(output) => output,
        Err(err) => {
            add_error(format!("Could not render screen at [{location}]: {}", err.message()));
            return Ok(BTreeMap::new());
        }
    };
    if output.status != 200 {
        add_error(format!(
            "Could not render screen at [{location}]: status {}",
            output.status
        ));
        return Ok(BTreeMap::new());
    }
    let bytes = match crate::screen::PdfRenderer::render(&output.body_text()) {
        Ok(bytes) => bytes,
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    };
    let mut print = parameters.clone();
    print.extend(screen_parameters.clone());
    if param_text(parameters, "jobName").is_empty() {
        let mut job_name = def.screen_name.clone();
        for value in screen_parameters.values() {
            job_name.push(' ');
            job_name.push_str(&to_plain_string(value));
        }
        print.insert("jobName".to_string(), Value::string(job_name));
    }
    print.insert("inputStream".to_string(), Value::Bytes(bytes));
    print.insert("contentType".to_string(), Value::string(content_type));
    Ok(call(PRINT_DOCUMENT, &print, &CallOptions::default()))
}

/// A record's fields, a `PrintJob` or a `NetworkPrinter`.
type Fields = IndexMap<String, Value>;
/// A `NetworkPrinter`, its server, and the server's printer.
type Resolved = (Fields, PrintServer, PrinterInfo);

fn force_new() -> CallOptions {
    CallOptions {
        require_new_transaction: Some(true),
        ..CallOptions::default()
    }
}

fn update_print_job(print_job_id: &str, fields: &[(&str, Value)], options: &CallOptions) {
    let mut update = BTreeMap::new();
    update.insert("printJobId".to_string(), Value::string(print_job_id));
    for (name, value) in fields {
        update.insert((*name).to_string(), value.clone());
    }
    call(&format!("update#{PRINT_JOB}"), &update, options);
}

/// The server of a `NetworkPrinter`: no `serverPort`, or 0, is 631, the field's documented default. Java's
/// `(int) networkPrinter.serverPort ?: 631` casts before the default, so a record with no port fails there (accepted,
/// P8a-38).
fn server_of(printer: &IndexMap<String, Value>) -> PrintServer {
    let port = match printer.get("serverPort") {
        Some(value) if !is_empty(value) => to_plain_string(value).parse().unwrap_or(IPP_PORT),
        _ => IPP_PORT,
    };
    PrintServer {
        host: map_text(printer, "serverHost"),
        port: if port == 0 { IPP_PORT } else { port },
    }
}

/// `make#CupsPrinter`: the `NetworkPrinter` and the server's printer of that name, or the server's default printer when
/// the record names none.
fn make_printer(facade: &EntityFacade, network_printer_id: &str) -> Result<Result<Resolved, String>, LibError> {
    let Some(record) = facade
        .find(NETWORK_PRINTER)?
        .eq("networkPrinterId", Value::string(network_printer_id))
        .one()?
    else {
        return Ok(Err(format!("Could not find NetworkPrinter [{network_printer_id}]")));
    };
    let record: IndexMap<String, Value> = record.fields().clone().into_iter().collect();
    let server = server_of(&record);
    let client = crate::print::client().map_err(|err| LibError::new(err.message()))?;
    let printer_name = map_text(&record, "printerName");
    let found = if printer_name.is_empty() {
        client.default_printer(&server)
    } else {
        client
            .printers(&server)
            .map(|printers| printers.into_iter().find(|printer| printer.name == printer_name))
    };
    match found {
        Ok(Some(printer)) => Ok(Ok((record, server, printer))),
        Ok(None) => Ok(Err(format!(
            "Print not found with name [{printer_name}] on server [{}] for NetworkPrinter [{network_printer_id}]",
            server.host
        ))),
        Err(err) => Ok(Err(err.message().to_string())),
    }
}

/// `send#PrintJobInternal`: print the job's document on its printer. Taken, the job is `PtjPending` with the server's
/// job id; refused or unreachable, it is `PtjSendFailed` with the message, written in its own transaction, and the call
/// returns the error.
fn send_print_job_internal(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        add_error("Printing is disabled in LLM sim session");
        return Ok(BTreeMap::new());
    }
    // A map from `print#Document`, or the entity value `send#PrintJob` finds.
    let job = parameters
        .get("printJob")
        .and_then(crate::script::value_map)
        .unwrap_or_default();
    let print_job_id = map_text(&job, "printJobId");
    let (job_id, error) = match send_job(&job, facade)? {
        Ok(result) => (result.job_id, result.error),
        Err(message) => (None, Some(message)),
    };
    // A refused job the server gave no id is stored and returned with none, where cups4j's `PrintRequestResult` says -1
    // (accepted, P8a-40).
    let mut out = BTreeMap::new();
    if let Some(job_id) = job_id {
        out.insert("jobId".to_string(), Value::I32(job_id));
    }
    let job_id_value = job_id.map_or(Value::Null, Value::I32);
    if let Some(message) = error {
        if !print_job_id.is_empty() {
            update_print_job(
                &print_job_id,
                &[
                    ("jobId", job_id_value),
                    ("statusId", Value::string("PtjSendFailed")),
                    ("errorMessage", Value::string(&message)),
                ],
                &force_new(),
            );
        }
        add_error(message);
    } else if !print_job_id.is_empty() {
        update_print_job(
            &print_job_id,
            &[("jobId", job_id_value), ("statusId", Value::string("PtjPending"))],
            &force_new(),
        );
    }
    Ok(out)
}

fn send_job(
    job: &IndexMap<String, Value>,
    facade: &EntityFacade,
) -> Result<Result<crate::print::PrintResult, String>, LibError> {
    // A printer that cannot be had (no record, no such printer, no server or no client) is a failed send recorded on
    // the job; Java's `make#CupsPrinter` runs before its `try`, so its job stays `PtjNotSent` with no `errorMessage`
    // (accepted, P8a-39).
    let (_, server, printer) = match make_printer(facade, &map_text(job, "networkPrinterId")) {
        Ok(Ok(found)) => found,
        Ok(Err(message)) => return Ok(Err(message)),
        Err(err) => return Ok(Err(err.message().to_string())),
    };
    // No document fails the send, as cups4j's `PrintJob.Builder` does on a null `document`.
    let bytes = match job.get("document") {
        Some(Value::Bytes(bytes)) => bytes.clone(),
        Some(Value::String(text)) => text.as_bytes().to_vec(),
        _ => {
            return Ok(Err(format!(
                "PrintJob [{}] has no document",
                map_text(job, "printJobId")
            )))
        }
    };
    let optional = |name: &str| Some(map_text(job, name)).filter(|text| !text.is_empty());
    // The Groovy sets `copies` and `duplex` only when the job has them, else cups4j's `PrintJob.Builder` defaults
    // apply: no copies (or 0) is 1, a negative count is not sent, and no duplex is one-sided.
    let copies = optional("copies")
        .and_then(|text| text.parse::<i32>().ok())
        .filter(|copies| *copies != 0)
        .unwrap_or(1);
    let document = PrintDocument {
        bytes,
        content_type: map_text(job, "contentType"),
        user_name: optional("username"),
        job_name: optional("jobName"),
        copies: Some(copies).filter(|copies| *copies > 0),
        duplex: Some(map_text(job, "duplex") == "Y"),
        page_ranges: optional("pageRanges"),
    };
    let client = match crate::print::client() {
        Ok(client) => client,
        Err(err) => return Ok(Err(err.message().to_string())),
    };
    Ok(client
        .print(&server, &printer, &document)
        .map_err(|err| err.message().to_string()))
}

/// The `PrintJob`, its printer, and the server for a `printJobId` (`make#CupsClient`).
fn job_and_server(facade: &EntityFacade, print_job_id: &str) -> Result<Option<(Fields, PrintServer)>, LibError> {
    let Some(job) = facade
        .find(PRINT_JOB)?
        .eq("printJobId", Value::string(print_job_id))
        .one()?
    else {
        add_error(format!("Could not find PrintJob [{print_job_id}]"));
        return Ok(None);
    };
    let job: IndexMap<String, Value> = job.fields().clone().into_iter().collect();
    let Some(printer) = facade
        .find(NETWORK_PRINTER)?
        .eq("networkPrinterId", Value::string(map_text(&job, "networkPrinterId")))
        .one()?
    else {
        add_error(format!("Could not find NetworkPrinter for PrintJob [{print_job_id}]"));
        return Ok(None);
    };
    let printer: IndexMap<String, Value> = printer.fields().clone().into_iter().collect();
    Ok(Some((job, server_of(&printer))))
}

fn job_id_of(job: &IndexMap<String, Value>) -> Option<i32> {
    map_text(job, "jobId").parse().ok()
}

/// `get#ServerPrinters`: a `NetworkPrinter` for each printer of the server that has none yet, and the ids of all of
/// that server's records.
fn get_server_printers(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let host = param_text(parameters, "serverHost");
    let port = param_text(parameters, "serverPort").parse().unwrap_or(IPP_PORT);
    let existing = facade
        .find(NETWORK_PRINTER)?
        .eq("serverHost", Value::string(&host))
        .eq("serverPort", Value::I64(i64::from(port)))
        .list()?;
    let mut ids: Vec<Value> = Vec::new();
    let mut names = BTreeSet::new();
    for row in existing.values() {
        if let Some(id) = row.get("networkPrinterId") {
            ids.push(id.clone());
        }
        names.insert(row.get("printerName").map(to_plain_string).unwrap_or_default());
    }
    let client = crate::print::client().map_err(|err| LibError::new(err.message()))?;
    let server = PrintServer {
        host: host.clone(),
        port,
    };
    let printers = client.printers(&server).map_err(|err| LibError::new(err.message()))?;
    for printer in printers.into_iter().filter(|printer| !names.contains(&printer.name)) {
        let mut create = BTreeMap::new();
        create.insert("serverHost".to_string(), Value::string(&host));
        create.insert("serverPort".to_string(), Value::I64(i64::from(port)));
        create.insert("printerName".to_string(), Value::string(&printer.name));
        create.insert("description".to_string(), Value::string(&printer.description));
        create.insert("location".to_string(), Value::string(&printer.location));
        let created = call(&format!("create#{NETWORK_PRINTER}"), &create, &CallOptions::default());
        if has_error() {
            break;
        }
        if let Some(id) = created.get("networkPrinterId") {
            ids.push(id.clone());
        }
    }
    let mut out = BTreeMap::new();
    out.insert("networkPrinterIdList".to_string(), Value::List(ids));
    Ok(out)
}

/// cups4j's `JobStateEnum` names to `PrintJob` statuses.
fn status_of_state(state: &str) -> Option<&'static str> {
    Some(match state {
        "aborted" => "PtjAborted",
        "canceled" => "PtjCanceled",
        "completed" => "PtjCompleted",
        "pending" => "PtjPending",
        "pending-held" => "PtjPendingHeld",
        "processing" => "PtjProcessing",
        "processing-stopped" => "PtjProcessingStopped",
        _ => return None,
    })
}

/// `get#PrintJobDetailsFromServer`: the `PrintJob` fields and the job's attributes from the server; a status that
/// changed is saved on the job.
fn get_print_job_details(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let print_job_id = param_text(parameters, "printJobId");
    let Some((job, server)) = job_and_server(facade, &print_job_id)? else {
        return Ok(BTreeMap::new());
    };
    let mut out: BTreeMap<String, Value> = job
        .iter()
        .filter(|(name, _)| name.as_str() != "printJobId")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let Some(job_id) = job_id_of(&job) else {
        add_error(format!(
            "PrintJob [{print_job_id}] has not been sent to the print server"
        ));
        return Ok(out);
    };
    let client = crate::print::client().map_err(|err| LibError::new(err.message()))?;
    let attributes = client
        .job_attributes(&server, job_id)
        .map_err(|err| LibError::new(err.message()))?;
    // A number the server does not give is null, where cups4j's `PrintJobAttributes` ints say 0 (accepted, P8a-40).
    let stamp = |seconds: Option<i64>| {
        seconds
            .and_then(|seconds| jiff::Timestamp::from_second(seconds).ok())
            .map_or(Value::Null, Value::Timestamp)
    };
    out.insert("completeTime".to_string(), stamp(attributes.complete_time));
    out.insert("createTime".to_string(), stamp(attributes.create_time));
    out.insert(
        "jobUrl".to_string(),
        attributes.job_url.map_or(Value::Null, Value::String),
    );
    out.insert(
        "pagesPrinted".to_string(),
        attributes.pages_printed.map_or(Value::Null, Value::I32),
    );
    out.insert(
        "printerUrl".to_string(),
        attributes.printer_url.map_or(Value::Null, Value::String),
    );
    out.insert("size".to_string(), attributes.size.map_or(Value::Null, Value::I32));
    if let Some(status) = status_of_state(&attributes.state) {
        out.insert("statusId".to_string(), Value::string(status));
        if map_text(&job, "statusId") != status {
            update_print_job(
                &print_job_id,
                &[("statusId", Value::string(status))],
                &CallOptions::default(),
            );
        }
    }
    Ok(out)
}

fn hold_print_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    job_action(parameters, facade, JobAction::Hold)
}

fn release_print_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    job_action(parameters, facade, JobAction::Release)
}

fn cancel_print_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    job_action(parameters, facade, JobAction::Cancel)
}

/// `hold#`, `release#` and `cancel#PrintJob`: the action on the server, and the job's new status when it was done.
fn job_action(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    action: JobAction,
) -> Result<BTreeMap<String, Value>, LibError> {
    let print_job_id = param_text(parameters, "printJobId");
    let Some((job, server)) = job_and_server(facade, &print_job_id)? else {
        return Ok(BTreeMap::new());
    };
    let (_, _, printer) = match make_printer(facade, &map_text(&job, "networkPrinterId"))? {
        Ok(found) => found,
        Err(message) => {
            add_error(message);
            return Ok(BTreeMap::new());
        }
    };
    let (verb, status) = match action {
        JobAction::Hold => ("hold", "PtjPendingHeld"),
        JobAction::Release => ("release", "PtjPending"),
        JobAction::Cancel => ("cancel", "PtjCanceled"),
    };
    // A job with no server id fails the call, as Java's `(int) printJob.jobId` does.
    let Some(job_id) = job_id_of(&job) else {
        add_error(format!(
            "PrintJob [{print_job_id}] has not been sent to the print server"
        ));
        return Ok(BTreeMap::new());
    };
    let done = crate::print::client()
        .map_err(|err| LibError::new(err.message()))?
        .job_action(&server, &printer, job_id, action)
        .map_err(|err| LibError::new(err.message()))?;
    if done {
        update_print_job(
            &print_job_id,
            &[("statusId", Value::string(status))],
            &CallOptions::default(),
        );
    } else {
        let text = format!(
            "Could not {verb} job [{job_id}] on print server at [{}] for PrintJob [{print_job_id}]",
            server.host
        );
        ExecutionContext::with_current(|ctx| ctx.message().add_message(text));
    }
    Ok(BTreeMap::new())
}

/// `CollectionUtilities.findAllFieldsNestedMap`: every value of `key` in the map, its nested maps and the maps in its
/// lists (not in a list in a list). An empty value names no user, so it is left out where Java adds it: its
/// `NotificationMessageUser` for user `""` would fail the send for every user of the document.
fn find_nested(key: &str, value: &Value, found: &mut BTreeSet<String>) {
    let Value::Map(map) = value else {
        return;
    };
    for (name, item) in map {
        if name == key && !is_empty(item) {
            found.insert(to_plain_string(item));
        }
        match item {
            Value::Map(_) => find_nested(key, item, found),
            Value::List(items) => items.iter().for_each(|entry| find_nested(key, entry, found)),
            _ => {}
        }
    }
}

/// `send#DataDocumentNotifications`: a notification for each document to the users named anywhere in it (`userId`,
/// `fromUserId`, `toUserId`). The topic is the document type; with no `NotificationTopic` for it the title is the
/// document's title and the link is its first link.
fn send_document_notifications(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let documents = match parameters.get("documentList") {
        Some(Value::List(items)) => items.clone(),
        _ => Vec::new(),
    };
    for document in &documents {
        let Value::Map(map) = document else {
            continue;
        };
        let doc_type = map.get("_type").map(to_plain_string).unwrap_or_default();
        let mut user_ids = BTreeSet::new();
        for key in ["userId", "fromUserId", "toUserId"] {
            find_nested(key, document, &mut user_ids);
        }
        if user_ids.is_empty() {
            tracing::info!("Not sending {doc_type} DataDocument notification, no userIds found");
            continue;
        }
        let mut message = crate::notify::NotificationMessage::new();
        message
            .topic(doc_type.clone())
            .message_map(map.clone())
            .user_ids(user_ids.iter().cloned());
        let topic = facade
            .find("moqui.security.user.NotificationTopic")?
            .disable_authz()
            .eq("topic", Value::string(&doc_type))
            .one()?;
        if topic.is_none() && !doc_type.is_empty() {
            let data_document = facade
                .find("moqui.entity.document.DataDocument")?
                .disable_authz()
                .use_cache(true)
                .eq("dataDocumentId", Value::string(&doc_type))
                .one()?;
            if let Some(title) = data_document
                .as_ref()
                .and_then(|row| row.get("documentTitle"))
                .filter(|title| !is_empty(title))
            {
                message.title(to_plain_string(title));
            }
            let links = facade
                .find("moqui.entity.document.DataDocumentLink")?
                .disable_authz()
                .use_cache(true)
                .eq("dataDocumentId", Value::string(&doc_type))
                .list()?;
            if let Some(link) = links
                .get(0)
                .and_then(|row| row.get("linkUrl"))
                .filter(|link| !is_empty(link))
            {
                message.link(to_plain_string(link));
            }
        }
        message.send().map_err(|err| LibError::new(err.message()))?;
    }
    Ok(BTreeMap::new())
}

/// Rows read per page by [`rewrite_encrypted_fields`].
const REWRITE_PAGE: i64 = 500;

/// `rewrite#EntityEncryptedFields`: read each record with an encrypted field set and write it back, so it is stored
/// with the current key and algorithm. As in Java, a record with a field that does not decrypt is logged and skipped.
fn rewrite_encrypted_fields(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    rewrite_encrypted_in_pages(&param_text(parameters, "entityName"), facade, REWRITE_PAGE)
}

/// Java updates each record while its iterator is open. A stream holds the MySQL connection, so this reads pages in
/// primary key order instead, each starting after the last key of the one before, all in the caller's transaction. The
/// rewrite changes neither a key nor which encrypted fields are null, so the pages hold still under the updates.
fn rewrite_encrypted_in_pages(
    entity_name: &str,
    facade: &EntityFacade,
    page_size: i64,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (full_name, pk_names, encrypt_names) = facade.with_entity(entity_name, |entity, _, _| {
        Ok((
            entity.full_name.clone(),
            entity
                .pk_field_names()
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            entity
                .fields
                .iter()
                .filter(|field| field.encrypt)
                .map(|field| field.name.clone())
                .collect::<Vec<_>>(),
        ))
    })?;
    let mut out = BTreeMap::new();
    if encrypt_names.is_empty() {
        tracing::warn!("Not rewriting encrypted fields for entity {entity_name}, has no encrypted fields");
        return Ok(out);
    }
    let not_null: Vec<Condition> = encrypt_names
        .iter()
        .map(|name| Condition::is_not_null(name.as_str()))
        .collect();
    let mut updated = 0_i64;
    let mut last: Option<IndexMap<String, Value>> = None;
    loop {
        let mut find = facade
            .find(&full_name)?
            .disable_authz()
            .use_cache(false)
            .condition(Condition::or(not_null.clone()))
            .limit(page_size);
        if let Some(last) = &last {
            find = find.condition(key_after(&pk_names, last));
        }
        for name in &pk_names {
            find = find.order_by(name, false);
        }
        for name in pk_names.iter().chain(encrypt_names.iter()) {
            find = find.select_field(name);
        }
        let page = find.list()?;
        let read = page.len();
        for value in page.values() {
            let mut row: IndexMap<String, Value> = IndexMap::new();
            for name in pk_names.iter().chain(encrypt_names.iter()) {
                row.insert(name.clone(), value.get(name).cloned().unwrap_or(Value::Null));
            }
            last = Some(row.clone());
            if encrypt_names
                .iter()
                .all(|name| row.get(name).is_none_or(Value::is_null))
            {
                continue;
            }
            let pk_text = || {
                pk_names
                    .iter()
                    .map(|name| row.get(name).map(to_plain_string).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("::")
            };
            if let Some(failed) = encrypt_names
                .iter()
                .find(|name| row.get(*name).and_then(Value::as_str) == Some(crate::entity::field_rw::DECRYPT_FAILED))
            {
                tracing::error!(
                    "rewrite#EntityEncryptedFields decrypt failed for {entity_name} pk {} field {failed}, skipping record",
                    pk_text()
                );
                continue;
            }
            let mut rewrite = facade.make_value(&full_name)?;
            for (name, field_value) in &row {
                rewrite.set(name.clone(), field_value.clone());
            }
            // One record that will not write is logged, and the rest are still rewritten.
            match rewrite.update(facade) {
                Ok(()) => updated += 1,
                Err(err) => tracing::error!(
                    error = %err,
                    "rewrite#EntityEncryptedFields could not update {entity_name} pk {}, skipping record",
                    pk_text()
                ),
            }
        }
        if i64::try_from(read).unwrap_or(i64::MAX) < page_size {
            break;
        }
    }
    out.insert("recordsUpdated".to_string(), Value::I64(updated));
    Ok(out)
}

/// The rows whose primary key sorts after `last`'s: `p1 > v1 OR (p1 = v1 AND p2 > v2) OR ...`.
fn key_after(pk_names: &[String], last: &IndexMap<String, Value>) -> Condition {
    let value = |name: &String| last.get(name).cloned().unwrap_or(Value::Null);
    let mut options = Vec::with_capacity(pk_names.len());
    for (index, name) in pk_names.iter().enumerate() {
        let mut parts: Vec<Condition> = pk_names[..index]
            .iter()
            .map(|before| Condition::equals(before.as_str(), value(before)))
            .collect();
        parts.push(Condition::greater(name.as_str(), value(name)));
        options.push(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Condition::and(parts)
        });
    }
    if options.len() == 1 {
        options.remove(0)
    } else {
        Condition::or(options)
    }
}

#[cfg(test)]
mod tests;
