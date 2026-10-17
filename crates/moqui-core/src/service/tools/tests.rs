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

//! The tools services on the in-memory database: print jobs and the services that are not available yet. The clean
//! services compare stamps, which the in-memory database does not; they and the print jobs on Postgres, with the
//! screens, are `moqui-host/tests/tools_services.rs`.

use std::collections::BTreeMap;

use moqui_lib::object::{to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::service::ServicePools;

const PRINT_JOB: &str = "moqui.basic.print.PrintJob";

fn setup() -> (EntityFacade, ExecutionContext) {
    let facade = crate::testing::open();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), crate::context::framework_dir());
    context.factory().set_entity(facade.clone());
    crate::testing::seed_status_flows(&facade);
    (facade, context)
}

fn install(files: &[&str]) {
    let dir = crate::context::framework_dir().join("service/org/moqui/impl");
    let paths: Vec<_> = files.iter().map(|file| dir.join(file)).collect();
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::string(*value)))
        .collect()
}

fn errors(context: &ExecutionContext) -> String {
    context.message().errors().join("\n")
}

fn job_field(facade: &EntityFacade, id: &str, field: &str) -> String {
    facade
        .find(PRINT_JOB)
        .unwrap()
        .eq("printJobId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get(field).map(to_plain_string))
        .unwrap_or_default()
}

/// A print server that takes every job but one named "refused", and knows one printer, "gate".
struct FakePrinter {
    printed: std::sync::Mutex<Vec<crate::print::PrintDocument>>,
}

impl crate::print::PrintClient for FakePrinter {
    fn printers(
        &self,
        _server: &crate::print::PrintServer,
    ) -> Result<Vec<crate::print::PrinterInfo>, moqui_lib::error::Error> {
        Ok(vec![crate::print::PrinterInfo {
            name: "gate".to_string(),
            description: "Gate printer".to_string(),
            location: "Dock".to_string(),
            uri: "ipp://localhost:631/printers/gate".to_string(),
        }])
    }

    fn default_printer(
        &self,
        server: &crate::print::PrintServer,
    ) -> Result<Option<crate::print::PrinterInfo>, moqui_lib::error::Error> {
        Ok(self.printers(server)?.into_iter().next())
    }

    fn print(
        &self,
        _server: &crate::print::PrintServer,
        printer: &crate::print::PrinterInfo,
        document: &crate::print::PrintDocument,
    ) -> Result<crate::print::PrintResult, moqui_lib::error::Error> {
        assert_eq!(printer.name, "gate");
        self.printed.lock().unwrap().push(document.clone());
        Ok(if document.job_name.as_deref() == Some("refused") {
            crate::print::PrintResult {
                job_id: None,
                error: Some("client-error-document-format-not-supported".to_string()),
            }
        } else {
            crate::print::PrintResult {
                job_id: Some(42),
                error: None,
            }
        })
    }

    fn job_attributes(
        &self,
        _server: &crate::print::PrintServer,
        job_id: i32,
    ) -> Result<crate::print::JobAttributes, moqui_lib::error::Error> {
        assert_eq!(job_id, 42);
        Ok(crate::print::JobAttributes {
            state: "completed".to_string(),
            pages_printed: Some(1),
            create_time: Some(1_700_000_000),
            ..Default::default()
        })
    }

    fn job_action(
        &self,
        _server: &crate::print::PrintServer,
        _printer: &crate::print::PrinterInfo,
        _job_id: i32,
        action: crate::print::JobAction,
    ) -> Result<bool, moqui_lib::error::Error> {
        Ok(action != crate::print::JobAction::Release)
    }
}

#[test]
fn a_print_job_is_sent_through_the_print_client_and_its_status_follows_the_server() {
    let _print = crate::print::print_test_lock();
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["PrintServices.xml"]);

    // No client installed: the job is created and the send fails with the not-installed message.
    crate::print::clear_print_client();
    crate::service::sync(
        "create#moqui.basic.print.NetworkPrinter",
        &params(&[
            ("networkPrinterId", "GATE_PRINTER"),
            ("serverHost", "localhost"),
            ("printerName", "gate"),
        ]),
    )
    .unwrap();
    let mut call = params(&[
        ("networkPrinterId", "GATE_PRINTER"),
        ("contentType", "text/plain"),
        ("jobName", "Test job"),
        ("copies", "2"),
        ("duplex", "Y"),
    ]);
    call.insert("inputStream".to_string(), Value::Bytes(b"hello printer".to_vec()));
    let out = crate::service::sync("org.moqui.impl.PrintServices.print#Document", &call).unwrap();
    let id = to_plain_string(out.get("printJobId").unwrap());
    assert!(!id.is_empty(), "{out:?}");
    assert_eq!(errors(&context), crate::print::PRINTING_UNAVAILABLE);
    // The create and the failed send each ran in their own transaction, so the record stays with the moved status.
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjSendFailed");
    assert_eq!(job_field(&facade, &id, "jobName"), "Test job");
    assert_eq!(
        job_field(&facade, &id, "errorMessage"),
        crate::print::PRINTING_UNAVAILABLE
    );
    let stored = facade
        .find(PRINT_JOB)
        .unwrap()
        .eq("printJobId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    assert!(matches!(stored.get("document"), Some(Value::Bytes(bytes)) if bytes.as_slice() == b"hello printer"));

    // With a client: the server takes the job, which is pending with the server's job id.
    let printer = std::sync::Arc::new(FakePrinter {
        printed: std::sync::Mutex::new(Vec::new()),
    });
    crate::print::install_print_client(printer.clone());
    context.message().clear();
    let out = crate::service::sync("org.moqui.impl.PrintServices.print#Document", &call).unwrap();
    let id = to_plain_string(out.get("printJobId").unwrap());
    assert_eq!(errors(&context), "");
    assert_eq!(out.get("jobId"), Some(&Value::I32(42)));
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjPending");
    assert_eq!(job_field(&facade, &id, "jobId"), "42");
    {
        let printed = printer.printed.lock().unwrap();
        let sent = printed.last().unwrap();
        assert_eq!(sent.bytes, b"hello printer");
        assert_eq!(sent.content_type, "text/plain");
        assert_eq!((sent.copies, sent.duplex), (Some(2), Some(true)));
    }

    // The details come from the server and move the status.
    let details = crate::service::sync(
        "org.moqui.impl.PrintServices.get#PrintJobDetailsFromServer",
        &params(&[("printJobId", &id)]),
    )
    .unwrap();
    assert_eq!(details.get("pagesPrinted"), Some(&Value::I32(1)));
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjCompleted");

    // Hold sets the held status, which the status flow allows only from pending: put the job back first.
    let mut job = facade
        .find(PRINT_JOB)
        .unwrap()
        .eq("printJobId", Value::string(id.clone()))
        .one()
        .unwrap()
        .unwrap();
    job.set("statusId", Value::string("PtjPending"));
    job.update(&facade).unwrap();
    // A release the server does not do is a message and no change.
    crate::service::sync(
        "org.moqui.impl.PrintServices.hold#PrintJob",
        &params(&[("printJobId", &id)]),
    )
    .unwrap();
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjPendingHeld");
    crate::service::sync(
        "org.moqui.impl.PrintServices.release#PrintJob",
        &params(&[("printJobId", &id)]),
    )
    .unwrap();
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjPendingHeld");
    assert!(context
        .message()
        .messages()
        .iter()
        .any(|text| text.starts_with("Could not release job [42]")));

    // A job the server refuses is a failed send with the server's reason.
    context.message().clear();
    let mut refused = call.clone();
    refused.insert("jobName".to_string(), Value::string("refused"));
    let out = crate::service::sync("org.moqui.impl.PrintServices.print#Document", &refused).unwrap();
    let id = to_plain_string(out.get("printJobId").unwrap());
    assert_eq!(errors(&context), "client-error-document-format-not-supported");
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjSendFailed");

    // get#ServerPrinters makes a record for a printer the server has and no record names; "gate" has one.
    context.message().clear();
    let printers = crate::service::sync(
        "org.moqui.impl.PrintServices.get#ServerPrinters",
        &params(&[("serverHost", "otherhost")]),
    )
    .unwrap();
    assert!(
        matches!(printers.get("networkPrinterIdList"), Some(Value::List(ids)) if ids.len() == 1),
        "{printers:?}"
    );
    let again = crate::service::sync(
        "org.moqui.impl.PrintServices.get#ServerPrinters",
        &params(&[("serverHost", "otherhost")]),
    )
    .unwrap();
    assert_eq!(printers.get("networkPrinterIdList"), again.get("networkPrinterIdList"));
    crate::print::clear_print_client();

    // No document at all is the Java message.
    context.message().clear();
    crate::service::sync(
        "org.moqui.impl.PrintServices.print#Document",
        &params(&[("networkPrinterId", "P")]),
    )
    .unwrap();
    assert!(
        errors(&context).contains("Both inputStream and serialBlob are null"),
        "{}",
        errors(&context)
    );
}

/// As the Groovy on cups4j: `send#PrintJob` sends the record it finds, and one with no copies or duplex as one copy,
/// one-sided; a job with no document is a failed send; and a hold of a job the server never took fails the call.
#[test]
fn a_send_takes_cups4j_defaults_and_fails_with_no_document_and_a_hold_needs_a_sent_job() {
    let _print = crate::print::print_test_lock();
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["PrintServices.xml"]);
    let printer = std::sync::Arc::new(FakePrinter {
        printed: std::sync::Mutex::new(Vec::new()),
    });
    crate::print::install_print_client(printer.clone());
    crate::service::sync(
        "create#moqui.basic.print.NetworkPrinter",
        &params(&[
            ("networkPrinterId", "GATE_PRINTER"),
            ("serverHost", "localhost"),
            ("printerName", "gate"),
        ]),
    )
    .unwrap();
    let mut job = facade.make_value(PRINT_JOB).unwrap();
    job.set("printJobId", Value::string("BARE_JOB"));
    job.set("networkPrinterId", Value::string("GATE_PRINTER"));
    job.set("statusId", Value::string("PtjNotSent"));
    job.set("document", Value::Bytes(b"bare".to_vec()));
    job.create(&facade).unwrap();
    let mut empty = facade.make_value(PRINT_JOB).unwrap();
    empty.set("printJobId", Value::string("EMPTY_JOB"));
    empty.set("networkPrinterId", Value::string("GATE_PRINTER"));
    empty.set("statusId", Value::string("PtjNotSent"));
    empty.create(&facade).unwrap();

    // A hold before the send fails: the job has no server id.
    crate::service::sync(
        "org.moqui.impl.PrintServices.hold#PrintJob",
        &params(&[("printJobId", "BARE_JOB")]),
    )
    .unwrap();
    assert_eq!(
        errors(&context),
        "PrintJob [BARE_JOB] has not been sent to the print server"
    );
    assert_eq!(job_field(&facade, "BARE_JOB", "statusId"), "PtjNotSent");

    context.message().clear();
    crate::service::sync(
        "org.moqui.impl.PrintServices.send#PrintJob",
        &params(&[("printJobId", "BARE_JOB")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(job_field(&facade, "BARE_JOB", "statusId"), "PtjPending");
    {
        let printed = printer.printed.lock().unwrap();
        let sent = printed.last().unwrap();
        assert_eq!(sent.bytes, b"bare");
        assert_eq!((sent.copies, sent.duplex), (Some(1), Some(false)));
    }

    crate::service::sync(
        "org.moqui.impl.PrintServices.send#PrintJob",
        &params(&[("printJobId", "EMPTY_JOB")]),
    )
    .unwrap();
    assert_eq!(errors(&context), "PrintJob [EMPTY_JOB] has no document");
    assert_eq!(job_field(&facade, "EMPTY_JOB", "statusId"), "PtjSendFailed");
    assert_eq!(
        job_field(&facade, "EMPTY_JOB", "errorMessage"),
        "PrintJob [EMPTY_JOB] has no document"
    );
    assert_eq!(
        printer.printed.lock().unwrap().len(),
        1,
        "nothing is printed for the empty job"
    );
    crate::print::clear_print_client();
}

#[test]
fn a_spreadsheet_loads_into_an_entity_through_the_auto_service() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["SpreadsheetServices.xml"]);
    let mut book = crate::screen::xlsx::XlsxBook::default();
    book.start_sheet("Units");
    for title in ["uomId", "Name", "uomTypeEnumId", "ignored column"] {
        book.header(title);
    }
    for (id, name) in [("SHEET_A", "Sheet A"), ("SHEET_B", "Sheet B")] {
        for value in [id, name, "UT_LENGTH_MEASURE", "x"] {
            book.cell(&[(Value::string(value), String::new())], Default::default())
                .unwrap();
        }
        book.end_row();
    }
    let mut call = BTreeMap::new();
    call.insert("entityName".to_string(), Value::string("moqui.basic.Uom"));
    call.insert("contentBytes".to_string(), Value::Bytes(book.to_bytes().unwrap()));
    let mut field_map = indexmap::IndexMap::new();
    field_map.insert("Name".to_string(), Value::string("description"));
    call.insert("fieldMap".to_string(), Value::Map(field_map));
    let out = crate::service::sync("org.moqui.impl.SpreadsheetServices.load#SpreadsheetEntity", &call).unwrap();
    assert_eq!(errors(&context), "");
    assert_eq!(out.get("recordCount"), Some(&Value::I64(2)));
    let row = facade
        .find("moqui.basic.Uom")
        .unwrap()
        .eq("uomId", Value::string("SHEET_B"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(row.get("description").map(to_plain_string).as_deref(), Some("Sheet B"));
}

struct FakePdf;

impl crate::screen::PdfEngine for FakePdf {
    fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
        assert!(html.contains("@page"), "the screen renders as html-print: {html}");
        Ok(b"%PDF-1.7 fake".to_vec())
    }
}

#[test]
fn a_screen_document_prints_its_pdf_or_reports_that_pdf_is_not_available() {
    let _pdf = crate::screen::pdf_test_lock();
    crate::screen::clear_pdf_engine();
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["PrintServices.xml"]);
    let location = format!(
        "{}/src/screen/testdata/Render/Render/Print.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    let call = params(&[
        ("networkPrinterId", "P"),
        ("screenLocation", &location),
        ("jobName", "Screen job"),
    ]);
    crate::service::sync("org.moqui.impl.PrintServices.print#ScreenDocument", &call).unwrap();
    assert_eq!(errors(&context), crate::screen::PDF_UNAVAILABLE);

    // With an engine the PDF is stored on the job, and the send fails as any other.
    context.message().clear();
    crate::screen::install_pdf_engine(std::sync::Arc::new(FakePdf));
    let out = crate::service::sync("org.moqui.impl.PrintServices.print#ScreenDocument", &call).unwrap();
    // With no jobName it is the screen name and the screenParameters values in the order given, as Java's `[:]`.
    let mut unnamed = params(&[("networkPrinterId", "P"), ("screenLocation", &location)]);
    let mut screen_parameters = indexmap::IndexMap::new();
    screen_parameters.insert("zeta".to_string(), Value::string("first"));
    screen_parameters.insert("alpha".to_string(), Value::string("second"));
    unnamed.insert("screenParameters".to_string(), Value::Map(screen_parameters));
    context.message().clear();
    let named = crate::service::sync("org.moqui.impl.PrintServices.print#ScreenDocument", &unnamed).unwrap();
    crate::screen::clear_pdf_engine();
    let named_id = to_plain_string(named.get("printJobId").unwrap());
    assert_eq!(job_field(&facade, &named_id, "jobName"), "Print first second");
    let id = to_plain_string(out.get("printJobId").unwrap());
    assert!(!id.is_empty(), "{out:?} {}", errors(&context));
    assert_eq!(job_field(&facade, &id, "contentType"), "application/pdf");
    assert_eq!(job_field(&facade, &id, "statusId"), "PtjSendFailed");
    let stored = facade
        .find(PRINT_JOB)
        .unwrap()
        .eq("printJobId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    assert!(matches!(stored.get("document"), Some(Value::Bytes(bytes)) if bytes.starts_with(b"%PDF-")));

    context.message().clear();
    crate::service::sync(
        "org.moqui.impl.PrintServices.print#ScreenDocument",
        &params(&[
            ("networkPrinterId", "P"),
            ("screenLocation", "file:/no/such/NotThere.xml"),
        ]),
    )
    .unwrap();
    assert!(
        errors(&context).contains("Screen not found at [file:/no/such/NotThere.xml]"),
        "{}",
        errors(&context)
    );
}

fn crypt(pass: &str) -> crate::entity::crypto::CryptConfig {
    crate::entity::crypto::CryptConfig {
        pass: pass.to_string(),
        salt: "20201202".to_string(),
        iterations: 10,
        algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
    }
}

/// Pages of three over seven records with a two field key: each record is rewritten once, under the new key, and a
/// record that does not decrypt is skipped.
#[test]
fn encrypted_fields_are_rewritten_in_key_pages() {
    const ENV: &str = "moqui.server.instance.InstanceImageTypeEnv";
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    facade.set_disable_fk_create(true);
    let old = crypt("OldPassword:CHANGEME");
    let new = crypt("NewPassword:CHANGEME");
    facade.set_crypt(old.clone(), Vec::new());
    let mut keys = Vec::new();
    for image in ["A", "B"] {
        for env in ["E1", "E2", "E3", "E4"] {
            keys.push((image, env));
        }
    }
    keys.pop();
    for (image, env) in &keys {
        let mut value = facade.make_value(ENV).unwrap();
        value.set("imageTypeId", Value::string(*image));
        value.set("envName", Value::string(*env));
        value.set("envValue", Value::string(format!("{image}-{env}")));
        value.create(&facade).unwrap();
    }
    // Sealed under a key the rewrite does not have.
    facade.set_crypt(crypt("Unknown:CHANGEME"), Vec::new());
    let mut lost = facade.make_value(ENV).unwrap();
    lost.set("imageTypeId", Value::string("A"));
    lost.set("envName", Value::string("E9"));
    lost.set("envValue", Value::string("lost"));
    lost.create(&facade).unwrap();

    facade.set_crypt(new.clone(), vec![old]);
    let out = super::rewrite_encrypted_in_pages(ENV, &facade, 3).unwrap();
    assert_eq!(out.get("recordsUpdated"), Some(&Value::I64(7)));

    facade.set_crypt(new, Vec::new());
    for (image, env) in &keys {
        let row = facade
            .find(ENV)
            .unwrap()
            .eq("imageTypeId", Value::string(*image))
            .eq("envName", Value::string(*env))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(
            row.get("envValue").map(to_plain_string).as_deref(),
            Some(format!("{image}-{env}").as_str())
        );
    }
}

#[test]
fn key_after_orders_by_each_key_field_in_turn() {
    use crate::entity::Condition;
    let names: Vec<String> = ["a", "b", "c"].iter().map(|name| name.to_string()).collect();
    let mut last = indexmap::IndexMap::new();
    for name in &names {
        last.insert(name.clone(), Value::string(format!("{name}1")));
    }
    let one = super::key_after(&names[..1], &last);
    assert_eq!(one, Condition::greater("a", Value::string("a1")));
    let three = super::key_after(&names, &last);
    assert_eq!(
        three,
        Condition::or(vec![
            Condition::greater("a", Value::string("a1")),
            Condition::and(vec![
                Condition::equals("a", Value::string("a1")),
                Condition::greater("b", Value::string("b1")),
            ]),
            Condition::and(vec![
                Condition::equals("a", Value::string("a1")),
                Condition::equals("b", Value::string("b1")),
                Condition::greater("c", Value::string("c1")),
            ]),
        ])
    );
}

/// As Java's `findAllFieldsNestedMap`: the values in the document, its nested maps and the maps in its lists, not in a
/// list in a list; an empty value names no user.
#[test]
fn document_user_ids_are_found_in_nested_maps_and_lists_of_maps() {
    let map = |pairs: Vec<(&str, Value)>| {
        Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
    };
    let document = map(vec![
        ("userId", Value::string("U1")),
        ("party", map(vec![("userId", Value::string("U2"))])),
        (
            "notes",
            Value::List(vec![map(vec![("userId", Value::string("U3"))]), Value::string("text")]),
        ),
        (
            "pages",
            Value::List(vec![Value::List(vec![map(vec![("userId", Value::string("U4"))])])]),
        ),
        ("blank", map(vec![("userId", Value::string(""))])),
    ]);
    let mut found = std::collections::BTreeSet::new();
    super::find_nested("userId", &document, &mut found);
    assert_eq!(found.into_iter().collect::<Vec<_>>(), ["U1", "U2", "U3"]);
}
