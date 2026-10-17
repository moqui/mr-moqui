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

//! The IPP client for network printing (the `moqui-cups` port), installed as core's [`PrintClient`]. The calls are the
//! ones CUPS4J made: CUPS-Get-Printers and CUPS-Get-Default to the server, Print-Job, Hold-Job, Release-Job and
//! Cancel-Job to the printer's URI, and Get-Job-Attributes by job URI under `/jobs/`. The messages are the `ipp` crate's
//! encoding, posted as `application/ipp` with the async `reqwest` on the edge runtime.
//!
//! A server is `http://host:port`, as CUPS4J connected; port 443, or a host that names `https://`, is TLS. The
//! requesting user is the job's username, else the process user, as CUPS4J used `user.name`.
//!
//! Where the wire differs from CUPS4J's: a job carries its `document-format`, the job's `contentType`, where CUPS4J
//! sends none and the server types the document; the `printer-uri` is the one the server gave, posted to on this
//! server, where CUPS4J writes this server's host with no port and the path; a hold, release or cancel names the job by
//! `printer-uri` and `job-id`, where CUPS4J posts a `job-uri` to `/jobs/<id>`; and a CUPS-Get-Printers the server
//! refuses for any reason but not-found is an error, where CUPS4J reads it as no printers (accepted, P8a-37).

use std::io::{Cursor, Read};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ipp::attribute::{IppAttribute, IppAttributeGroup};
use ipp::model::{DelimiterTag, IppVersion, Operation};
use ipp::parser::IppParser;
use ipp::payload::IppPayload;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use moqui_core::print::{JobAction, JobAttributes, PrintClient, PrintDocument, PrintResult, PrintServer, PrinterInfo};
use moqui_lib::error::Error;

/// A print request carries the document; allow a slow printer to take it.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Install the IPP client as the framework's print client.
pub fn install() {
    moqui_core::print::install_print_client(Arc::new(IppPrintClient));
}

#[derive(Default)]
pub struct IppPrintClient;

/// The HTTP scheme and the `host:port` authority of a server. A host that names its scheme or its port keeps them;
/// otherwise the scheme is `https` on port 443 and `http` on any other, and the port is the server's.
fn endpoint(server: &PrintServer) -> (&'static str, String) {
    let host = server.host.trim_end_matches('/');
    let (named, rest) = if let Some(rest) = host.strip_prefix("https://") {
        (Some("https"), rest)
    } else if let Some(rest) = host.strip_prefix("http://") {
        (Some("http"), rest)
    } else {
        (None, host)
    };
    // a bare IPv6 address is not a host and a port: its last group is not a port, and a URL brackets it
    let bare_v6 = rest.parse::<std::net::Ipv6Addr>().is_ok();
    let has_port = !bare_v6
        && rest
            .rsplit_once(':')
            .is_some_and(|(_, tail)| !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()));
    let authority = if has_port {
        rest.to_string()
    } else if bare_v6 {
        format!("[{rest}]:{}", server.port)
    } else {
        format!("{rest}:{}", server.port)
    };
    let scheme = named.unwrap_or(if server.port == 443 { "https" } else { "http" });
    (scheme, authority)
}

/// The HTTP base of a server: `http://host:port`, or `https://` on 443 or when the host names it.
fn base_url(server: &PrintServer) -> String {
    let (scheme, authority) = endpoint(server);
    format!("{scheme}://{authority}")
}

/// The `ipp://` URI of a path on the server, as the attribute value CUPS expects.
fn ipp_uri(server: &PrintServer, path: &str) -> String {
    let (_, authority) = endpoint(server);
    format!("ipp://{authority}{path}")
}

/// The one HTTP client for every print server, so connections and the TLS setup are reused.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The path of a printer URI (`ipp://host:631/printers/name` is `/printers/name`), where its requests are posted.
fn uri_path(uri: &str) -> String {
    uri.split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|index| rest[index..].to_string()))
        .unwrap_or_else(|| "/".to_string())
}

fn user_name(given: Option<&str>) -> String {
    given
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("USER").ok().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| "anonymous".to_string())
}

fn parse_error(err: impl std::fmt::Display) -> Error {
    Error::new(format!("IPP: {err}"))
}

fn attribute(name: &str, value: IppValue) -> Result<IppAttribute, Error> {
    Ok(IppAttribute::new(name.try_into().map_err(parse_error)?, value))
}

fn name_value(text: &str) -> Result<IppValue, Error> {
    Ok(IppValue::NameWithoutLanguage(text.try_into().map_err(parse_error)?))
}

fn keyword(text: &str) -> Result<IppValue, Error> {
    Ok(IppValue::Keyword(text.try_into().map_err(parse_error)?))
}

fn request(operation: Operation, printer_uri: Option<&str>) -> Result<IppRequestResponse, Error> {
    let uri = printer_uri
        .map(|uri| uri.parse::<http::Uri>().map_err(parse_error))
        .transpose()?;
    IppRequestResponse::new(IppVersion::v1_1(), operation, uri).map_err(parse_error)
}

fn add_operation(request: &mut IppRequestResponse, name: &str, value: IppValue) -> Result<(), Error> {
    request
        .attributes_mut()
        .add(DelimiterTag::OperationAttributes, attribute(name, value)?);
    Ok(())
}

fn add_job(request: &mut IppRequestResponse, name: &str, value: IppValue) -> Result<(), Error> {
    request
        .attributes_mut()
        .add(DelimiterTag::JobAttributes, attribute(name, value)?);
    Ok(())
}

/// Post one request and parse the response.
fn send(server: &PrintServer, path: &str, request: IppRequestResponse) -> Result<IppRequestResponse, Error> {
    let mut body = Vec::new();
    request
        .into_read()
        .read_to_end(&mut body)
        .map_err(|err| Error::new(format!("could not encode the IPP request: {err}")))?;
    let url = format!("{}{path}", base_url(server));
    let shown = url.clone();
    let response = crate::edge::block_on(async move {
        let response = client()?
            .post(&url)
            .header("Content-Type", "application/ipp")
            .body(body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|err| err.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP status {status}"));
        }
        Ok(bytes.to_vec())
    })?
    .map_err(|err| Error::new(format!("Could not reach print server at {shown}: {err}")))?;
    IppParser::new(ipp::reader::IppReader::new(Cursor::new(response)))
        .parse()
        .map_err(|err| Error::new(format!("Could not read the response of print server at {shown}: {err}")))
}

fn is_success(response: &IppRequestResponse) -> bool {
    response.header().status_code().is_success()
}

/// The server's reason for a status that is not success: its `status-message`, else the status code.
fn status_text(response: &IppRequestResponse) -> String {
    response
        .attributes()
        .groups_of(DelimiterTag::OperationAttributes)
        .find_map(|group| {
            group
                .attributes()
                .iter()
                .find(|attr| attr.name().as_str() == "status-message")
        })
        .map_or_else(
            || response.header().status_code().to_string(),
            |attr| attr.value().to_string(),
        )
}

fn text_of(group: &IppAttributeGroup, name: &str) -> Option<String> {
    let attr = group.attributes().iter().find(|attr| attr.name().as_str() == name)?;
    let value = match attr.value() {
        IppValue::Array(values) => values.first()?,
        value => value,
    };
    Some(value.to_string())
}

fn int_of(group: &IppAttributeGroup, name: &str) -> Option<i32> {
    let attr = group.attributes().iter().find(|attr| attr.name().as_str() == name)?;
    match attr.value() {
        IppValue::Integer(number) | IppValue::Enum(number) => Some(*number),
        _ => None,
    }
}

fn printer_of(group: &IppAttributeGroup) -> PrinterInfo {
    PrinterInfo {
        name: text_of(group, "printer-name").unwrap_or_default(),
        description: text_of(group, "printer-info").unwrap_or_default(),
        location: text_of(group, "printer-location").unwrap_or_default(),
        uri: text_of(group, "printer-uri-supported").unwrap_or_default(),
    }
}

fn printers_of(response: &IppRequestResponse) -> Vec<PrinterInfo> {
    response
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
        .map(printer_of)
        .collect()
}

/// `page-ranges` text (`1-3,5`) as the IPP ranges: none for empty text or `1-` (every page), as CUPS4J. Text that is
/// not page numbers fails the send, as CUPS4J's `Integer.parseInt` does, rather than printing every page.
fn page_ranges(text: &str) -> Result<Option<IppValue>, Error> {
    let text = text.trim();
    if text.is_empty() || text == "1-" {
        return Ok(None);
    }
    let unreadable = || {
        Error::new(format!(
            "Could not read page ranges [{text}]: give pages and ranges such as 1-3,5"
        ))
    };
    let mut ranges = Vec::new();
    for part in text.split(',').map(str::trim).filter(|part| !part.is_empty()) {
        let (low, high) = part.split_once('-').unwrap_or((part, part));
        ranges.push(IppValue::RangeOfInteger {
            min: low.trim().parse().map_err(|_| unreadable())?,
            max: high.trim().parse().map_err(|_| unreadable())?,
        });
    }
    Ok(match ranges.len() {
        0 => None,
        1 => ranges.pop(),
        _ => Some(IppValue::Array(ranges)),
    })
}

/// A job or printer URI as CUPS4J's `PrintJobAttributes` gives it, a URL with `ipp://` made `http://`.
fn http_url(uri: String) -> String {
    uri.replace("ipp://", "http://")
}

/// cups4j's `JobStateEnum` text of an IPP `job-state`.
fn job_state(value: i32) -> &'static str {
    match value {
        3 => "pending",
        4 => "pending-held",
        5 => "processing",
        6 => "processing-stopped",
        7 => "canceled",
        8 => "aborted",
        9 => "completed",
        _ => "",
    }
}

impl PrintClient for IppPrintClient {
    fn printers(&self, server: &PrintServer) -> Result<Vec<PrinterInfo>, Error> {
        let response = send(server, "/", request(Operation::CupsGetPrinters, None)?)?;
        if !is_success(&response) && response.header().status_code() != ipp::model::StatusCode::ClientErrorNotFound {
            return Err(Error::new(format!(
                "Could not get printers from print server at {}: {}",
                server.host,
                status_text(&response)
            )));
        }
        Ok(printers_of(&response))
    }

    fn default_printer(&self, server: &PrintServer) -> Result<Option<PrinterInfo>, Error> {
        let response = send(server, "/", request(Operation::CupsGetDefault, None)?)?;
        if !is_success(&response) {
            return Ok(None);
        }
        Ok(printers_of(&response).into_iter().next())
    }

    fn print(
        &self,
        server: &PrintServer,
        printer: &PrinterInfo,
        document: &PrintDocument,
    ) -> Result<PrintResult, Error> {
        let printer_uri = if printer.uri.is_empty() {
            ipp_uri(server, &format!("/printers/{}", printer.name))
        } else {
            printer.uri.clone()
        };
        let mut message = request(Operation::PrintJob, Some(&printer_uri))?;
        add_operation(
            &mut message,
            "requesting-user-name",
            name_value(&user_name(document.user_name.as_deref()))?,
        )?;
        if let Some(job_name) = document.job_name.as_deref().filter(|name| !name.is_empty()) {
            add_operation(&mut message, "job-name", name_value(job_name)?)?;
        }
        // The job's type, which CUPS4J does not send (see the module doc); with none it is octet-stream, which the
        // server types.
        let format = if document.content_type.is_empty() {
            "application/octet-stream"
        } else {
            document.content_type.split(';').next().unwrap_or("").trim()
        };
        add_operation(
            &mut message,
            "document-format",
            IppValue::MimeMediaType(format.try_into().map_err(parse_error)?),
        )?;
        if let Some(copies) = document.copies {
            add_job(&mut message, "copies", IppValue::Integer(copies))?;
        }
        if let Some(duplex) = document.duplex {
            let sides = if duplex { "two-sided-long-edge" } else { "one-sided" };
            add_job(&mut message, "sides", keyword(sides)?)?;
        }
        if let Some(ranges) = document.page_ranges.as_deref().map(page_ranges).transpose()?.flatten() {
            add_job(&mut message, "page-ranges", ranges)?;
        }
        *message.payload_mut() = IppPayload::new(Cursor::new(document.bytes.clone()));
        let response = send(server, &uri_path(&printer_uri), message)?;
        let job_id = response
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .find_map(|group| int_of(group, "job-id"));
        let error = (!is_success(&response)).then(|| status_text(&response));
        Ok(PrintResult { job_id, error })
    }

    fn job_attributes(&self, server: &PrintServer, job_id: i32) -> Result<JobAttributes, Error> {
        let mut message = request(Operation::GetJobAttributes, None)?;
        add_operation(
            &mut message,
            "job-uri",
            IppValue::Uri(
                ipp_uri(server, &format!("/jobs/{job_id}"))
                    .try_into()
                    .map_err(parse_error)?,
            ),
        )?;
        add_operation(&mut message, "requesting-user-name", name_value(&user_name(None))?)?;
        let response = send(server, "/jobs/", message)?;
        if !is_success(&response) {
            return Err(Error::new(format!(
                "Could not get job [{job_id}] from print server at {}: {}",
                server.host,
                status_text(&response)
            )));
        }
        let Some(group) = response.attributes().groups_of(DelimiterTag::JobAttributes).next() else {
            return Ok(JobAttributes::default());
        };
        Ok(JobAttributes {
            state: int_of(group, "job-state")
                .map(job_state)
                .unwrap_or_default()
                .to_string(),
            complete_time: int_of(group, "time-at-completed")
                .filter(|time| *time > 0)
                .map(i64::from),
            create_time: int_of(group, "time-at-creation")
                .filter(|time| *time > 0)
                .map(i64::from),
            job_url: text_of(group, "job-uri").map(http_url),
            pages_printed: int_of(group, "job-media-sheets-completed"),
            printer_url: text_of(group, "job-printer-uri").map(http_url),
            size: int_of(group, "job-k-octets"),
        })
    }

    fn job_action(
        &self,
        server: &PrintServer,
        printer: &PrinterInfo,
        job_id: i32,
        action: JobAction,
    ) -> Result<bool, Error> {
        let operation = match action {
            JobAction::Hold => Operation::HoldJob,
            JobAction::Release => Operation::ReleaseJob,
            JobAction::Cancel => Operation::CancelJob,
        };
        let printer_uri = if printer.uri.is_empty() {
            ipp_uri(server, &format!("/printers/{}", printer.name))
        } else {
            printer.uri.clone()
        };
        let mut message = request(operation, Some(&printer_uri))?;
        add_operation(&mut message, "job-id", IppValue::Integer(job_id))?;
        add_operation(&mut message, "requesting-user-name", name_value(&user_name(None))?)?;
        let response = send(server, &uri_path(&printer_uri), message)?;
        Ok(is_success(&response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_paths() {
        let server = PrintServer {
            host: "cups.local".to_string(),
            port: 631,
        };
        assert_eq!(base_url(&server), "http://cups.local:631");
        assert_eq!(ipp_uri(&server, "/jobs/7"), "ipp://cups.local:631/jobs/7");
        assert_eq!(uri_path("ipp://cups.local:631/printers/gate"), "/printers/gate");
        assert_eq!(uri_path("ipp://cups.local"), "/");
        let tls = PrintServer {
            host: "cups.example".to_string(),
            port: 443,
        };
        assert_eq!(base_url(&tls), "https://cups.example:443");
    }

    #[test]
    fn a_host_that_names_its_port_keeps_it_once() {
        let named = |host: &str| PrintServer {
            host: host.to_string(),
            port: 631,
        };
        let server = named("http://cups:632/");
        assert_eq!(base_url(&server), "http://cups:632");
        assert_eq!(ipp_uri(&server, "/printers/x"), "ipp://cups:632/printers/x");
        let server = named("https://cups.example");
        assert_eq!(base_url(&server), "https://cups.example:631");
        assert_eq!(ipp_uri(&server, "/printers/x"), "ipp://cups.example:631/printers/x");
        let server = named("cups:632");
        assert_eq!(base_url(&server), "http://cups:632");
        assert_eq!(ipp_uri(&server, "/jobs/1"), "ipp://cups:632/jobs/1");
        let server = named("::1");
        assert_eq!(base_url(&server), "http://[::1]:631");
        let server = named("http://[::1]");
        assert_eq!(base_url(&server), "http://[::1]:631");
        assert_eq!(ipp_uri(&server, "/"), "ipp://[::1]:631/");
        assert!(request(
            Operation::PrintJob,
            Some(&ipp_uri(&named("http://cups:632"), "/printers/x"))
        )
        .is_ok());
    }

    #[test]
    fn page_ranges_are_ranges() {
        assert!(matches!(
            page_ranges("2"),
            Ok(Some(IppValue::RangeOfInteger { min: 2, max: 2 }))
        ));
        assert!(matches!(page_ranges("1-3, 5"), Ok(Some(IppValue::Array(values))) if values.len() == 2));
        // Every page, as CUPS4J: no attribute.
        assert!(matches!(page_ranges(" 1- "), Ok(None)));
        assert!(matches!(page_ranges(""), Ok(None)));
        // Not page numbers: the send fails, as CUPS4J's parse does, rather than printing every page.
        for text in ["x", "3-", "1-z"] {
            let err = page_ranges(text).unwrap_err();
            assert!(err.message().contains(&format!("[{text}]")), "{}", err.message());
        }
    }

    #[test]
    fn job_and_printer_urls_are_http_as_cups4j_gives_them() {
        assert_eq!(
            http_url("ipp://cups.local:631/jobs/7".to_string()),
            "http://cups.local:631/jobs/7"
        );
        assert_eq!(
            http_url("https://cups.example/jobs/7".to_string()),
            "https://cups.example/jobs/7"
        );
    }

    #[test]
    fn a_print_job_request_encodes_and_parses_back() {
        let mut message = request(Operation::PrintJob, Some("ipp://localhost:631/printers/gate")).unwrap();
        add_operation(&mut message, "job-name", name_value("Gate").unwrap()).unwrap();
        add_job(&mut message, "copies", IppValue::Integer(2)).unwrap();
        *message.payload_mut() = IppPayload::new(Cursor::new(b"hello".to_vec()));
        let mut body = Vec::new();
        message.into_read().read_to_end(&mut body).unwrap();
        let parsed = IppParser::new(ipp::reader::IppReader::new(Cursor::new(body)))
            .parse()
            .unwrap();
        assert_eq!(parsed.header().operation_or_status, Operation::PrintJob as i16);
        let job = parsed
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .next()
            .unwrap();
        assert_eq!(int_of(job, "copies"), Some(2));
        let mut payload = Vec::new();
        parsed.into_payload().read_to_end(&mut payload).unwrap();
        assert_eq!(payload, b"hello");
    }
}
