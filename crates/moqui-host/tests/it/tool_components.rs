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

//! The ported tool components on the seeded embedded Postgres, gate 4:
//!
//! - `moqui-poi`: a framework screen requested as `.xlsx` is a workbook with a sheet for each form-list and typed
//!   cells, and `read#Spreadsheet` reads it back;
//! - `moqui-image`: `convert#Image` writes the original and the four widths of a png, a gif and a tiff as jpg;
//! - `moqui-cups`: `get#ServerPrinters`, `print#Document`, `get#PrintJobDetailsFromServer` and `cancel#PrintJob` against
//!   an IPP server in the test, and a send to a server that does not answer.

#![cfg(all(unix, feature = "embedded-postgres", feature = "postgres", feature = "axum"))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use crate::common::{basic, exchange_bytes, header, http_request, start_with_data};
use calamine::{Data, Reader};
use ipp::attribute::IppAttribute;
use ipp::model::{DelimiterTag, IppVersion, Operation, StatusCode};
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use moqui_lib::object::{to_plain_string, Value};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.basic.print.NetworkPrinter networkPrinterId="SILENT_PRINTER" serverHost="127.0.0.1" serverPort="9"
        printerName="silent"/>
</entity-facade-xml>"#;

const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

fn params(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect()
}

fn text(value: &str) -> Value {
    Value::string(value)
}

/// What the stub IPP server saw: the operation, the job attributes (name and value) and the document of each request.
type Seen = Arc<Mutex<Vec<(i16, Vec<(String, String)>, Vec<u8>)>>>;

/// An IPP server with one printer, "stub", that takes every job as job 7 and reports it completed.
fn ipp_stub() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request = ipp::parser::IppParser::new(ipp::reader::IppReader::new(Cursor::new(body)))
                .parse()
                .unwrap();
            let operation = request.header().operation_or_status;
            let id = request.header().request_id;
            let job_attributes = request
                .attributes()
                .groups_of(DelimiterTag::JobAttributes)
                .flat_map(|group| group.attributes().iter())
                .map(|attr| (attr.name().as_str().to_string(), attr.value().to_string()))
                .collect();
            let mut payload = Vec::new();
            request.into_payload().read_to_end(&mut payload).unwrap();
            record.lock().unwrap().push((operation, job_attributes, payload));
            let mut response =
                IppRequestResponse::new_response(IppVersion::v1_1(), StatusCode::SuccessfulOk, id).unwrap();
            let mut add = |tag: DelimiterTag, name: &str, value: IppValue| {
                response
                    .attributes_mut()
                    .add(tag, IppAttribute::new(name.try_into().unwrap(), value));
            };
            let uri = |text: String| IppValue::Uri(text.as_str().try_into().unwrap());
            let name = |text: &str| IppValue::NameWithoutLanguage(text.try_into().unwrap());
            if operation == Operation::CupsGetPrinters as i16 || operation == Operation::CupsGetDefault as i16 {
                add(DelimiterTag::PrinterAttributes, "printer-name", name("stub"));
                add(
                    DelimiterTag::PrinterAttributes,
                    "printer-info",
                    IppValue::TextWithoutLanguage("Stub printer".try_into().unwrap()),
                );
                add(
                    DelimiterTag::PrinterAttributes,
                    "printer-uri-supported",
                    uri(format!("ipp://127.0.0.1:{port}/printers/stub")),
                );
            } else if operation == Operation::PrintJob as i16 {
                add(DelimiterTag::JobAttributes, "job-id", IppValue::Integer(7));
                add(DelimiterTag::JobAttributes, "job-state", IppValue::Enum(3));
            } else if operation == Operation::GetJobAttributes as i16 {
                add(DelimiterTag::JobAttributes, "job-id", IppValue::Integer(7));
                add(DelimiterTag::JobAttributes, "job-state", IppValue::Enum(9));
                add(
                    DelimiterTag::JobAttributes,
                    "job-media-sheets-completed",
                    IppValue::Integer(2),
                );
                add(
                    DelimiterTag::JobAttributes,
                    "time-at-creation",
                    IppValue::Integer(1_700_000_000),
                );
                add(
                    DelimiterTag::JobAttributes,
                    "job-uri",
                    uri(format!("ipp://127.0.0.1:{port}/jobs/7")),
                );
            }
            let mut bytes = Vec::new();
            response.into_read().read_to_end(&mut bytes).unwrap();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&bytes);
        }
    });
    (port, seen)
}

#[test]
#[ignore = "slow: cargo test-slow"]
fn xlsx_spreadsheets_images_and_ipp_printing() {
    let gate = start_with_data("tool-components", DATA);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    // ---- moqui-poi: the screen as xlsx, with the download name the XLS button sends ----
    let path =
        "/System/Security/ArtifactGroup/ArtifactGroupDetail.xlsx?artifactGroupId=SYSTEM_APP&saveFilename=Group.xlsx";
    let (status, headers, body) = exchange_bytes(address, &http_request("GET", path, &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(header(&headers, "content-type").as_deref(), Some(XLSX));
    assert!(header(&headers, "content-disposition").unwrap().contains("Group.xlsx"));
    let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(body.clone())).unwrap();
    assert_eq!(
        book.sheet_names(),
        vec!["ArtifactGroupMemberList", "ArtifactAuthzList", "ArtifactTarpitList"]
    );
    let authz = book.worksheet_range("ArtifactAuthzList").unwrap();
    assert_eq!(authz.get_value((0, 0)), Some(&Data::String("ID".to_string())));
    assert_eq!(
        authz.get_value((1, 0)),
        Some(&Data::String("SYSTEM_APP_ADMIN".to_string()))
    );
    // the field's two widgets (the group's description and its id) are one cell, a line each, as FormListExcelRender
    assert_eq!(
        authz.get_value((1, 1)),
        Some(&Data::String("Administrators (full access) - \nADMIN".to_string()))
    );
    // a number field is a number cell
    let tarpit = book.worksheet_range("ArtifactTarpitList").unwrap();
    assert_eq!(tarpit.get_value((1, 1)), Some(&Data::Float(100.0)));

    // renderMode=xlsx on a list screen, as the form-list button links it
    let path = "/System/Visit/VisitList?renderMode=xlsx&pageNoLimit=true&lastStandalone=true";
    let (status, headers, body) = exchange_bytes(address, &http_request("GET", path, &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(header(&headers, "content-type").as_deref(), Some(XLSX));
    assert!(body.starts_with(b"PK"));

    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();

    let read = moqui_core::service::sync(
        "org.moqui.impl.SpreadsheetServices.read#Spreadsheet",
        &params(&[
            ("contentBytes", Value::Bytes(exchange_xlsx(address, &john))),
            ("sheetName", text("ArtifactTarpitList")),
        ]),
    )
    .unwrap();
    let Some(Value::List(rows)) = read.get("rowList") else {
        panic!("{read:?}")
    };
    let Some(Value::Map(first)) = rows.first() else {
        panic!("{rows:?}")
    };
    assert_eq!(first.get("Max Hits Count"), Some(&Value::I64(100)));
    assert_eq!(
        read.get("sheetNameList"),
        Some(&Value::List(vec![
            text("ArtifactGroupMemberList"),
            text("ArtifactAuthzList"),
            text("ArtifactTarpitList")
        ]))
    );

    // ---- moqui-image: the original and four widths of each fixture ----
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images");
    for file in ["barnstar.png", "wolf.gif", "autumn.tif"] {
        let stem = file.split('.').next().unwrap();
        let bytes = std::fs::read(fixtures.join(file)).unwrap();
        let out = |size: &str, ext: &str| text(&format!("runtime://image-test/{stem}-{size}.{ext}"));
        let converted = moqui_core::service::sync(
            "org.moqui.impl.ImageServices.convert#Image",
            &params(&[
                ("inputImageStream", Value::Bytes(bytes)),
                ("outputImageFormat", text("jpg")),
                ("outputResourceReferenceOriginal", out("original", "jpg")),
                ("outputResourceReferenceLarge", out("large", "jpg")),
                ("outputResourceReferenceMedium", out("medium", "jpg")),
                ("outputResourceReferenceSmall", out("small", "jpg")),
                ("outputResourceReferenceThumbnail", out("thumbnail", "jpg")),
            ]),
        )
        .unwrap_or_else(|err| panic!("{file}: {err}"));
        // the original is written as outputImageFormat too, as its caller names the file
        for (size, width) in [
            ("Original", None),
            ("Large", Some(2160)),
            ("Medium", Some(1080)),
            ("Small", Some(360)),
            ("Thumbnail", Some(165)),
        ] {
            let location = to_plain_string(converted.get(&format!("contentLocation{size}")).unwrap());
            assert!(location.ends_with(".jpg"), "{location}");
            let written = factory.resource().get(&location).unwrap().read_bytes().unwrap();
            let image = image::load_from_memory(&written).unwrap_or_else(|err| panic!("{location}: {err}"));
            if let Some(width) = width {
                assert_eq!(image.width(), width, "{location}");
            }
            assert_eq!(
                image::guess_format(&written).unwrap(),
                image::ImageFormat::Jpeg,
                "{location}"
            );
        }
    }

    // ---- moqui-cups: a server that does not answer, then the stub ----
    let print = |printer: &str, job_name: &str| {
        context.message().clear();
        let out = moqui_core::service::sync(
            "org.moqui.impl.PrintServices.print#Document",
            &params(&[
                ("networkPrinterId", text(printer)),
                ("inputStream", Value::Bytes(b"%PDF-1.7 gate".to_vec())),
                ("contentType", text("application/pdf")),
                ("jobName", text(job_name)),
                ("copies", text("2")),
            ]),
        )
        .unwrap();
        to_plain_string(out.get("printJobId").unwrap())
    };
    let job_field = |id: &str, field: &str| {
        factory
            .entity()
            .unwrap()
            .find("moqui.basic.print.PrintJob")
            .unwrap()
            .eq("printJobId", text(id))
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(to_plain_string))
            .unwrap_or_default()
    };
    let silent = print("SILENT_PRINTER", "No answer");
    assert_eq!(job_field(&silent, "statusId"), "PtjSendFailed");
    assert!(
        job_field(&silent, "errorMessage").starts_with("Could not reach print server at http://127.0.0.1:9"),
        "{}",
        job_field(&silent, "errorMessage")
    );

    let (port, seen) = ipp_stub();
    context.message().clear();
    let printers = moqui_core::service::sync(
        "org.moqui.impl.PrintServices.get#ServerPrinters",
        &params(&[
            ("serverHost", text("127.0.0.1")),
            ("serverPort", Value::I32(i32::from(port))),
        ]),
    )
    .unwrap();
    let Some(Value::List(ids)) = printers.get("networkPrinterIdList") else {
        panic!("{printers:?}")
    };
    assert_eq!(ids.len(), 1, "{printers:?}");
    let printer_id = to_plain_string(&ids[0]);
    let job = print(&printer_id, "Gate stub");
    assert!(
        context.message().errors().is_empty(),
        "{:?}",
        context.message().errors()
    );
    assert_eq!(job_field(&job, "statusId"), "PtjPending");
    assert_eq!(job_field(&job, "jobId"), "7");
    {
        let seen = seen.lock().unwrap();
        let (_, job_attributes, _) = seen
            .iter()
            .find(|(operation, _, payload)| *operation == Operation::PrintJob as i16 && payload == b"%PDF-1.7 gate")
            .expect("the Print-Job with the document");
        // As cups4j's builder: the copies given, and `duplex` (`N` by default) one-sided.
        let attribute = |name: &str| {
            job_attributes
                .iter()
                .find(|(found, _)| found == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(attribute("copies"), Some("2"), "{job_attributes:?}");
        assert_eq!(attribute("sides"), Some("one-sided"), "{job_attributes:?}");
    }

    let details = moqui_core::service::sync(
        "org.moqui.impl.PrintServices.get#PrintJobDetailsFromServer",
        &params(&[("printJobId", text(&job))]),
    )
    .unwrap();
    assert_eq!(details.get("pagesPrinted"), Some(&Value::I32(2)));
    // The job URL as cups4j gives it, `http://` for `ipp://`.
    assert_eq!(
        details.get("jobUrl").map(to_plain_string),
        Some(format!("http://127.0.0.1:{port}/jobs/7"))
    );
    assert_eq!(job_field(&job, "statusId"), "PtjCompleted");

    // The status flow has no Completed to Canceled transition, so the cancel is of a second, pending job.
    let pending = print(&printer_id, "Gate stub cancel");
    assert_eq!(job_field(&pending, "statusId"), "PtjPending");
    moqui_core::service::sync(
        "org.moqui.impl.PrintServices.cancel#PrintJob",
        &params(&[("printJobId", text(&pending))]),
    )
    .unwrap();
    assert_eq!(job_field(&pending, "statusId"), "PtjCanceled");
}

fn exchange_xlsx(address: std::net::SocketAddr, auth: &str) -> Vec<u8> {
    let path = "/System/Security/ArtifactGroup/ArtifactGroupDetail.xlsx?artifactGroupId=SYSTEM_APP";
    exchange_bytes(address, &http_request("GET", path, &[("Authorization", auth)], "")).2
}
