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

use std::collections::BTreeMap;

use indexmap::IndexMap;

use super::{field_text, host_in, stored_headers, SinkGuard, LIST_HEADERS, MESSAGE};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::screen::PDF_UNAVAILABLE;
#[cfg(feature = "imap")]
use crate::service::eca_rules;
#[cfg(feature = "imap")]
use crate::service::mail::inbound::{header_one, parse_rfc822, read_line_bytes, FakeGuard, Inbound, FAKE_DELETED};
use moqui_lib::object::Value;

/// The tests set `email-client` `allowed-hosts`, a process property, so they share the property lock.
fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    crate::user::session::cookie_property_lock()
}

fn fixture(name: &str) -> String {
    format!("{}/src/service/testdata/mail/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn smtp_server(facade: &EntityFacade, context: &crate::context::ExecutionContext, id: &str) {
    seed(
        facade,
        context,
        "moqui.basic.email.EmailServer",
        &[
            ("emailServerId", id),
            ("smtpHost", "smtp.example.com"),
            ("smtpPort", "25"),
        ],
    );
}

/// `send#ScreenRenderEmail` requires a user, as in Java.
fn log_in(facade: &EntityFacade, context: &crate::context::ExecutionContext) {
    seed(
        facade,
        context,
        "moqui.security.UserAccount",
        &[
            ("userId", "EMAILER"),
            ("username", "emailer"),
            ("emailAddress", "emailer@example.com"),
        ],
    );
    crate::user::internal_login_user("emailer").expect("login");
}

fn map(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<IndexMap<_, _>>(),
    )
}

fn entered() -> (EntityFacade, crate::context::ExecutionContext) {
    entered_with("")
}

/// [`entered`] with the conf text `conf`.
fn entered_with(conf: &str) -> (EntityFacade, crate::context::ExecutionContext) {
    let facade = crate::testing::open();
    let context = ExecutionContextFactory::test_context_in(
        conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    context.factory().set_entity(facade.clone());
    (facade, context)
}

fn seed(facade: &EntityFacade, context: &crate::context::ExecutionContext, entity: &str, fields: &[(&str, &str)]) {
    let _authz = context.disable_authz();
    let mut row = facade.make_value(entity).expect("make");
    for (name, value) in fields {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).expect("create");
}

#[test]
fn host_suffix_matches_on_a_dot_boundary() {
    assert!(host_in("", "smtp.example.com"));
    assert!(host_in("example.com", "smtp.example.com"));
    assert!(host_in("example.com", "example.com"));
    assert!(!host_in("example.com", "notexample.com"));
}

#[test]
fn sink_receives_a_template_send_and_expands_the_subject() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[
            ("emailServerId", "TEST_SMTP"),
            ("smtpHost", "smtp.example.com"),
            ("smtpPort", "25"),
        ],
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "HELLO"),
            ("emailServerId", "TEST_SMTP"),
            ("fromAddress", "from@example.com"),
            ("subject", "Hello ${title}"),
        ],
    );
    let sink = SinkGuard::install();
    let mut body_parameters = indexmap::IndexMap::new();
    body_parameters.insert("title".into(), Value::string("there"));
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("HELLO"));
    params.insert("toAddresses".into(), Value::string("to@example.com"));
    params.insert("body".into(), Value::string("Hi"));
    params.insert("bodyText".into(), Value::string("Hi"));
    params.insert("bodyParameters".into(), Value::Map(body_parameters));
    let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.messages();
    assert_eq!(sent.len(), 1, "{errors}");
    assert_eq!(sent[0].subject, "Hello there");
    assert!(out.contains_key("emailMessageId"));
}

/// A list email's extra headers go out and stay on the message for a send again; one the letter writes itself, or a
/// value with a line break (an injected header), is left out.
#[test]
fn a_template_send_adds_the_list_headers_and_keeps_them() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[("emailServerId", "TEST_LIST"), ("smtpHost", "smtp.example.com")],
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "LIST"),
            ("emailServerId", "TEST_LIST"),
            ("fromAddress", "news@example.com"),
            ("subject", "News"),
        ],
    );
    let sink = SinkGuard::install();
    let mut extra = indexmap::IndexMap::new();
    extra.insert(
        "List-Unsubscribe".into(),
        Value::string("<https://example.com/u/TOKEN/oneClick>, <mailto:leave@example.com?subject=unsubscribe>"),
    );
    extra.insert(
        "List-Unsubscribe-Post".into(),
        Value::string("List-Unsubscribe=One-Click"),
    );
    extra.insert("Subject".into(), Value::string("Not this one"));
    extra.insert("X-Note".into(), Value::string("a\r\nBcc: someone@example.net"));
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("LIST"));
    params.insert("toAddresses".into(), Value::string("member@example.com"));
    params.insert("body".into(), Value::string("<p>News</p>"));
    params.insert("extraHeaders".into(), Value::Map(extra));
    let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.messages();
    assert_eq!(sent.len(), 1);
    let names: Vec<&str> = sent[0].extra_headers.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["List-Unsubscribe", "List-Unsubscribe-Post"]);
    assert_eq!(sent[0].subject, "News");
    assert!(sent[0].bcc.is_empty());
    // kept on the message, and what a send again of it takes
    let id = moqui_lib::object::to_plain_string(out.get("emailMessageId").expect("id"));
    let row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("emailMessageId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    let stored = field_text(&row, "headersString");
    let again = stored_headers(&stored, &LIST_HEADERS);
    assert_eq!(again, sent[0].extra_headers);
}

#[test]
fn a_template_send_threads_and_replies_as_the_caller_says() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[("emailServerId", "TEST_THREAD"), ("smtpHost", "smtp.example.com")],
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "THREAD"),
            ("emailServerId", "TEST_THREAD"),
            ("fromAddress", "forum@example.com"),
            ("replyToAddresses", "template@example.com"),
            ("subject", "Reply"),
        ],
    );
    let sink = SinkGuard::install();
    let send = |in_reply_to: &str, references: &str, reply_to: &str| {
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("THREAD"));
        params.insert("toAddresses".into(), Value::string("member@example.com"));
        params.insert("body".into(), Value::string("<p>Reply</p>"));
        params.insert("inReplyTo".into(), Value::string(in_reply_to));
        params.insert("references".into(), Value::string(references));
        params.insert("replyToAddresses".into(), Value::string(reply_to));
        crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send")
    };
    let out = send(
        "<b@forum.example.com>",
        "<a@forum.example.com> <b@forum.example.com>",
        "reply+t1@forum.example.com",
    );
    send("not an id", "<a@x> bad", "");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.messages();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].in_reply_to, "<b@forum.example.com>");
    assert_eq!(sent[0].references, "<a@forum.example.com> <b@forum.example.com>");
    assert_eq!(sent[0].reply_to, ["reply+t1@forum.example.com"]);
    assert!(sent[1].in_reply_to.is_empty() && sent[1].references.is_empty());
    assert_eq!(sent[1].reply_to, ["template@example.com"]);
    let id = moqui_lib::object::to_plain_string(out.get("emailMessageId").expect("id"));
    let row = facade
        .find(MESSAGE)
        .unwrap()
        .eq("emailMessageId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(field_text(&row, "inReplyTo"), "<b@forum.example.com>");
}

#[test]
fn host_rejected_by_allowed_hosts_does_not_touch_the_sink() {
    let _lock = lock_tests();
    let (facade, context) = entered_with(r#"<moqui-conf><email-client allowed-hosts="example.com"/></moqui-conf>"#);
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[("emailServerId", "TEST_BAD"), ("smtpHost", "notexample.com")],
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "BADHOST"),
            ("emailServerId", "TEST_BAD"),
            ("fromAddress", "from@example.com"),
            ("subject", "No"),
        ],
    );
    let sink = SinkGuard::install();
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("BADHOST"));
    params.insert("toAddresses".into(), Value::string("to@example.com"));
    params.insert("body".into(), Value::string("Hi"));
    params.insert("bodyText".into(), Value::string("Hi"));
    let _ = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params);
    let errors = context.message().errors().join("\n");
    assert!(errors.contains("notexample.com"), "{errors}");
    assert!(sink.messages().is_empty(), "{errors}");
}

/// Gate 4: a screen body renders in html and text and goes out as multipart/alternative, with
/// screen attachments (forEachIn, attachmentCondition, csv/text/html), a plain file, and an xsl-fo
/// attachment that is skipped because no PDF engine is installed.
#[test]
fn screen_body_renders_html_and_text_and_sends_with_attachments() {
    let _lock = lock_tests();
    let _pdf = crate::screen::pdf_test_lock();
    crate::screen::clear_pdf_engine();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    smtp_server(&facade, &context, "GATE_SMTP");
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "GATE_BODY"),
            ("emailServerId", "GATE_SMTP"),
            ("fromAddress", "from@example.com"),
            ("subject", "Report for ${name}"),
            ("bodyScreenLocation", &fixture("MailBody.xml")),
        ],
    );
    let report = fixture("MailReport.xml");
    let attachments: [&[(&str, &str)]; 5] = [
        &[
            ("fileName", "report-${rowName}.csv"),
            ("attachmentLocation", &report),
            ("screenRenderMode", "csv"),
            ("forEachIn", "reportRows"),
        ],
        &[
            ("fileName", "report.txt"),
            ("attachmentLocation", &report),
            ("screenRenderMode", "text"),
            ("attachmentCondition", "includeText"),
        ],
        &[
            ("fileName", "report.xml"),
            ("attachmentLocation", &report),
            ("screenRenderMode", "xml"),
            ("attachmentCondition", "!includeText"),
        ],
        &[
            ("fileName", "report.pdf"),
            ("attachmentLocation", &report),
            ("screenRenderMode", "xsl-fo"),
        ],
        &[("fileName", "plain.txt"), ("attachmentLocation", &fixture("plain.txt"))],
    ];
    for fields in attachments {
        let mut row = vec![("emailTemplateId", "GATE_BODY")];
        row.extend_from_slice(fields);
        seed(&facade, &context, "moqui.basic.email.EmailTemplateAttachment", &row);
    }
    let sink = SinkGuard::install();
    let rows = Value::List(vec![
        map(vec![("rowName", Value::string("one"))]),
        map(vec![("rowName", Value::string("two"))]),
    ]);
    let mut params = BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("GATE_BODY"));
    params.insert("toAddresses".into(), Value::string("to@example.com"));
    params.insert(
        "bodyParameters".into(),
        map(vec![
            ("name", Value::string("Ada")),
            ("includeText", Value::Bool(true)),
            ("reportRows", rows),
        ]),
    );
    params.insert(
        "attachments".into(),
        Value::List(vec![map(vec![
            ("fileName", Value::string("inline.html")),
            ("attachmentLocation", Value::string(&report)),
            ("screenRenderMode", Value::string("html")),
        ])]),
    );
    params.insert(
        "inlineAttachments".into(),
        Value::List(vec![map(vec![
            ("contentId", Value::string("report-image@moqui")),
            ("fileName", Value::string("report.png")),
            ("contentType", Value::string("image/png")),
            ("contentBytes", Value::Bytes(vec![137, 80, 78, 71])),
        ])]),
    );
    let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
    let errors = context.message().errors().join("\n");
    let messages = context.message().messages().join("\n");
    assert!(errors.is_empty(), "{errors}");
    assert!(!messages.contains("Error sending email"), "{messages}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{messages}");
    let mail = &sent[0];
    assert_eq!(mail.subject, "Report for Ada");
    assert!(mail.mime.contains("Content-ID: <report-image@moqui>"), "{}", mail.mime);
    assert!(mail.mime.contains("Content-Type: image/png"), "{}", mail.mime);
    assert!(mail.mime.contains("iVBORw=="), "{}", mail.mime);
    let email_message_id = out
        .get("emailMessageId")
        .and_then(Value::as_str)
        .expect("emailMessageId");
    assert!(
        mail.body.contains(r#"<p class="greeting">Hello Ada</p>"#),
        "{}",
        mail.body
    );
    assert!(mail.body.contains("Report for Ada via webroot"), "{}", mail.body);
    assert!(
        mail.body.contains(&format!("message {email_message_id}")),
        "{}",
        mail.body
    );
    assert!(
        mail.body_text.contains("Hello Ada\nSubject: Report for Ada"),
        "{}",
        mail.body_text
    );
    assert!(!mail.body_text.contains('<'), "{}", mail.body_text);

    let names: Vec<&str> = mail.attachments.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "inline.html",
            "report-one.csv",
            "report-two.csv",
            "report.txt",
            "plain.txt"
        ]
    );
    let part = |name: &str| mail.attachments.iter().find(|(file, _, _)| file == name).unwrap();
    assert_eq!(part("inline.html").1, "text/html");
    assert!(String::from_utf8_lossy(&part("inline.html").2).contains("<p>Report all</p>"));
    assert_eq!(part("report-one.csv").1, "text/csv");
    assert_eq!(
        String::from_utf8_lossy(&part("report-one.csv").2).trim(),
        "name,greeting\none,Ada"
    );
    assert!(String::from_utf8_lossy(&part("report-two.csv").2).contains("two,Ada"));
    assert_eq!(part("report.txt").1, "text/plain");
    assert_eq!(String::from_utf8_lossy(&part("report.txt").2).trim(), "Report all");
    assert_eq!(String::from_utf8_lossy(&part("plain.txt").2), "plain attachment text\n");

    assert!(mail.mime.contains("multipart/mixed"), "{}", mail.mime);
    assert!(mail.mime.contains("multipart/alternative"), "{}", mail.mime);
    assert!(mail.mime.contains("Content-Type: text/plain"), "{}", mail.mime);
    assert!(mail.mime.contains("Content-Type: text/html"), "{}", mail.mime);
    assert!(mail.mime.contains("report-one.csv"), "{}", mail.mime);
    assert!(!mail.mime.contains("report.pdf"), "{}", mail.mime);

    let _authz = context.disable_authz();
    let row = facade
        .find("moqui.basic.email.EmailMessage")
        .unwrap()
        .eq("emailMessageId", Value::string(email_message_id))
        .one()
        .unwrap()
        .expect("EmailMessage row");
    assert_eq!(row.get("statusId").and_then(Value::as_str), Some("ES_SENT"));
    assert!(row
        .get("bodyText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .contains("Hello Ada"));
    assert!(row
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .contains("Hello Ada"));
}

#[test]
fn screen_render_email_sends_the_rendered_screen_as_an_attachment() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    smtp_server(&facade, &context, "SYSTEM");
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "SCREEN_RENDER"),
            ("emailServerId", "SYSTEM"),
            ("emailTypeEnumId", "EMT_SCREEN_RENDER"),
            (
                "bodyScreenLocation",
                "component://framework/screen/ScreenRenderEmail.xml",
            ),
            ("webappName", "webroot"),
            ("fromAddress", "reports@example.com"),
            ("subject", "${title}"),
        ],
    );
    log_in(&facade, &context);
    let sink = SinkGuard::install();
    let mut params = BTreeMap::new();
    params.insert("screenLocation".into(), Value::string(fixture("MailReport.xml")));
    params.insert("screenRenderMode".into(), Value::string("csv"));
    params.insert("toAddresses".into(), Value::string("boss@example.com"));
    params.insert("title".into(), Value::string("Monthly Report"));
    params.insert("bodyParameters".into(), map(vec![("rowName", Value::string("March"))]));
    crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
    let errors = context.message().errors().join("\n");
    let messages = context.message().messages().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1, "{messages}");
    let mail = &sent[0];
    assert_eq!(mail.subject, "Monthly Report");
    assert!(mail.body.contains("<h2>Monthly Report</h2>"), "{}", mail.body);
    assert!(mail.body.contains("<strong>rowName</strong>: March"), "{}", mail.body);
    assert!(!mail.body.contains("lastStandalone"), "{}", mail.body);
    assert!(mail.body_text.contains("Monthly Report"), "{}", mail.body_text);
    assert!(mail.body_text.contains("rowName: March"), "{}", mail.body_text);
    assert_eq!(mail.attachments.len(), 1);
    let (name, content_type, bytes) = &mail.attachments[0];
    assert_eq!(name, "MailReport.csv");
    assert_eq!(content_type, "text/csv");
    assert!(String::from_utf8_lossy(bytes).contains("March"));
    assert!(mail.mime.contains("multipart/alternative"), "{}", mail.mime);
}

#[cfg(feature = "xlsx")]
/// The Email Export dialog's default: the screen as xlsx, attached as the workbook it renders.
#[test]
fn screen_render_email_as_xlsx_attaches_the_workbook() {
    use calamine::{Data, Reader};
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    smtp_server(&facade, &context, "SYSTEM");
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "SCREEN_RENDER"),
            ("emailServerId", "SYSTEM"),
            (
                "bodyScreenLocation",
                "component://framework/screen/ScreenRenderEmail.xml",
            ),
            ("fromAddress", "reports@example.com"),
            ("subject", "${title}"),
        ],
    );
    log_in(&facade, &context);
    let sink = SinkGuard::install();
    let mut params = BTreeMap::new();
    params.insert("screenLocation".into(), Value::string(fixture("MailList.xml")));
    params.insert("screenRenderMode".into(), Value::string("xlsx"));
    params.insert("toAddresses".into(), Value::string("boss@example.com"));
    params.insert("title".into(), Value::string("Monthly Report"));
    params.insert("bodyParameters".into(), map(vec![("rowName", Value::string("March"))]));
    crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1);
    let (name, content_type, bytes) = &sent[0].attachments[0];
    assert_eq!(name, "MailList.xlsx");
    assert_eq!(
        content_type,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes.clone())).unwrap();
    let range = book.worksheet_range("ReportList").unwrap();
    assert_eq!(range.get_value((1, 0)), Some(&Data::String("March".to_string())));
    assert_eq!(range.get_value((1, 1)), Some(&Data::Float(12.5)));
}

#[test]
fn screen_render_email_as_xsl_fo_attaches_the_pdf_or_sends_without_it() {
    let _lock = lock_tests();
    let _pdf = crate::screen::pdf_test_lock();
    crate::screen::clear_pdf_engine();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    smtp_server(&facade, &context, "SYSTEM");
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "SCREEN_RENDER"),
            ("emailServerId", "SYSTEM"),
            (
                "bodyScreenLocation",
                "component://framework/screen/ScreenRenderEmail.xml",
            ),
            ("fromAddress", "reports@example.com"),
            ("subject", "${title}"),
        ],
    );
    log_in(&facade, &context);
    let sink = SinkGuard::install();
    let mut params = BTreeMap::new();
    params.insert("screenLocation".into(), Value::string(fixture("MailReport.xml")));
    params.insert("screenRenderMode".into(), Value::string("xsl-fo"));
    params.insert("toAddresses".into(), Value::string("boss@example.com"));
    params.insert("title".into(), Value::string("Printed"));
    crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    assert!(!errors.contains(PDF_UNAVAILABLE), "{errors}");
    let sent = sink.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].attachments.is_empty());
    assert!(sent[0].body.contains("Printed"), "{}", sent[0].body);

    crate::screen::install_pdf_engine(std::sync::Arc::new(FakePdf));
    crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
    crate::screen::clear_pdf_engine();
    let sent = sink.sent();
    assert_eq!(sent.len(), 2);
    let (filename, content_type, bytes) = &sent[1].attachments[0];
    assert_eq!(content_type, "application/pdf");
    assert!(filename.ends_with(".pdf"), "{filename}");
    assert!(bytes.starts_with(b"%PDF-fake"));
}

struct FakePdf;

impl crate::screen::PdfEngine for FakePdf {
    fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
        assert!(html.contains("@page"), "the attachment is the html-print document");
        Ok(b"%PDF-fake".to_vec())
    }
}

#[test]
fn a_body_screen_that_fails_to_render_does_not_send() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    smtp_server(&facade, &context, "BROKEN_SMTP");
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailTemplate",
        &[
            ("emailTemplateId", "BROKEN"),
            ("emailServerId", "BROKEN_SMTP"),
            ("fromAddress", "from@example.com"),
            ("subject", "Broken"),
            ("bodyScreenLocation", &fixture("NoSuchScreen.xml")),
        ],
    );
    let sink = SinkGuard::install();
    let mut params = BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("BROKEN"));
    params.insert("toAddresses".into(), Value::string("to@example.com"));
    let _ = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params);
    let messages = context.message().messages().join("\n");
    assert!(messages.contains("Error sending email"), "{messages}");
    assert!(messages.contains("NoSuchScreen.xml"), "{messages}");
    assert!(sink.sent().is_empty());
}

#[test]
fn sim_skip_returns_email_message_id_sim() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    facade.set_sim_active(true);
    let sink = SinkGuard::install();
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailTemplateId".into(), Value::string("MISSING"));
    params.insert("toAddresses".into(), Value::string("to@example.com"));
    let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("sim");
    assert_eq!(out.get("emailMessageId").and_then(Value::as_str), Some("SIM"));
    assert!(sink.messages().is_empty());
    assert!(context.message().errors().is_empty());
}

#[cfg(feature = "imap")]
#[test]
fn fake_store_poll_runs_one_eca_and_skips_seen() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            "",
            r#"<emecas>
                <emeca rule-name="Notice">
                    <condition><expression>true</expression></condition>
                    <actions><script>ec.message.addMessage(fields.subject);</script></actions>
                </emeca>
            </emecas>"#,
        ),
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[
            ("emailServerId", "TEST_POLL"),
            ("storeHost", "imap.example.com"),
            ("mailUsername", "inbox"),
            ("mailPassword", "secret"),
            ("storeProtocol", "imap"),
            ("storeSkipSeen", "Y"),
        ],
    );
    let mut fresh = Inbound::blank();
    fresh.id = "1".to_string();
    fresh.subject = "new mail".to_string();
    fresh.seen = false;
    let mut seen = Inbound::blank();
    seen.id = "2".to_string();
    seen.subject = "old mail".to_string();
    seen.seen = true;
    let _store = FakeGuard::install(vec![fresh, seen]);
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailServerId".into(), Value::string("TEST_POLL"));
    crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params).expect("poll");
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let messages = context.message().messages().to_vec();
    assert!(messages.iter().any(|text| text == "new mail"), "{messages:?}");
    assert!(!messages.iter().any(|text| text == "old mail"), "{messages:?}");
}

#[cfg(feature = "imap")]
#[test]
fn an_inbound_message_reads_like_java_mail() {
    let raw = b"From: =?UTF-8?B?SsO8cmdlbg==?= <j@example.com>\r\n\
To: a@example.com, \"Two, B\" <b@example.com>\r\n\
Subject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe?= from\r\n the team\r\n\
Message-ID: <m1@example.com>\r\n\
Date: Tue, 5 Mar 2024 10:20:30 -0800\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=\"inner\"\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain; charset=ISO-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=E9 au lait\r\n\
--inner\r\n\
Content-Type: text/html; charset=UTF-8\r\n\
\r\n\
<p>Caf\xc3\xa9</p>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: application/octet-stream; name=\"data.bin\"\r\n\
Content-Disposition: attachment; filename=\"data.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AAEC/w==\r\n\
--outer--\r\n";
    let message = parse_rfc822(raw);
    assert_eq!(message.subject, "Gr\u{fc}\u{df}e from the team");
    assert_eq!(message.from, "J\u{fc}rgen <j@example.com>");
    assert_eq!(
        message.to,
        vec!["a@example.com".to_string(), "\"Two, B\" <b@example.com>".to_string()]
    );
    assert_eq!(header_one(&message.headers, "message-id"), "<m1@example.com>");
    assert!(!message.sent.is_null());
    // The leaf parts of both multiparts, in order: text in its charset, the attachment as bytes.
    assert_eq!(message.parts.len(), 3, "{:?}", message.parts);
    let part = |index: usize, key: &str| match &message.parts[index] {
        Value::Map(map) => map.get(key).cloned().unwrap_or(Value::Null),
        other => panic!("{other:?}"),
    };
    assert_eq!(part(0, "contentText").as_str(), Some("Caf\u{e9} au lait"));
    assert!(part(1, "contentType").as_str().unwrap().starts_with("text/html"));
    assert_eq!(part(2, "filename").as_str(), Some("data.bin"));
    assert_eq!(part(2, "disposition").as_str(), Some("attachment"));
    assert!(matches!(part(2, "contentBytes"), Value::Bytes(bytes) if bytes == vec![0, 1, 2, 255]));
}

#[cfg(feature = "imap")]
#[test]
fn a_pop3_line_with_8_bit_bytes_is_read() {
    let mut reader = std::io::Cursor::new(b"Caf\xe9\r\n.\r\n".to_vec());
    assert_eq!(read_line_bytes(&mut reader).unwrap(), b"Caf\xe9".to_vec());
    assert_eq!(read_line_bytes(&mut reader).unwrap(), b".".to_vec());
}

#[cfg(feature = "imap")]
#[test]
fn a_message_whose_rule_fails_is_not_marked_or_deleted() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            "",
            r#"<emecas>
                <emeca rule-name="Breaks"><condition><expression>fields.subject == 'bad'</expression></condition>
                    <actions><script>throw new Error('rule broke')</script></actions></emeca>
                <emeca rule-name="Notes"><actions><script>ec.message.addMessage('saw ' + fields.subject);</script></actions></emeca>
            </emecas>"#,
        ),
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[
            ("emailServerId", "TEST_KEEP"),
            ("storeHost", "imap.example.com"),
            ("mailUsername", "inbox"),
            ("mailPassword", "secret"),
            ("storeProtocol", "imap"),
            ("storeDelete", "Y"),
        ],
    );
    let mut bad = Inbound::blank();
    bad.id = "1".to_string();
    bad.subject = "bad".to_string();
    let mut good = Inbound::blank();
    good.id = "2".to_string();
    good.subject = "good".to_string();
    let _store = FakeGuard::install(vec![bad, good]);
    FAKE_DELETED.with(|deleted| deleted.borrow_mut().clear());
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailServerId".into(), Value::string("TEST_KEEP"));
    let _ = crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params);
    let messages = context.message().messages().to_vec();
    // The second rule still ran for the failing message, and the next message was processed.
    assert!(messages.iter().any(|text| text == "saw bad"), "{messages:?}");
    assert!(messages.iter().any(|text| text == "saw good"), "{messages:?}");
    let left = FAKE_DELETED.with(|deleted| deleted.borrow().clone());
    assert_eq!(left, vec!["2".to_string()]);
}

#[cfg(feature = "imap")]
#[test]
fn a_failed_message_keeps_the_ones_before_it_committed_and_deleted() {
    let _lock = lock_tests();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            "",
            r#"<emecas>
                <emeca rule-name="Save"><actions>
                    <service-call name="org.moqui.impl.EmailServices.save#EcaEmailMessage" in-map="context"/></actions></emeca>
                <emeca rule-name="Breaks"><condition><expression>fields.subject == 'bad'</expression></condition>
                    <actions><script>throw new Error('rule broke')</script></actions></emeca>
            </emecas>"#,
        ),
    );
    seed(
        &facade,
        &context,
        "moqui.basic.email.EmailServer",
        &[
            ("emailServerId", "TEST_EACH"),
            ("storeHost", "imap.example.com"),
            ("mailUsername", "inbox"),
            ("mailPassword", "secret"),
            ("storeProtocol", "imap"),
            ("storeDelete", "Y"),
        ],
    );
    let inbound = |id: &str, subject: &str| {
        let mut message = Inbound::blank();
        message.id = id.to_string();
        message.subject = subject.to_string();
        message
            .headers
            .insert("message-id".into(), Value::string(format!("<each-{id}@example.com>")));
        message
    };
    let _store = FakeGuard::install(vec![inbound("1", "first"), inbound("2", "bad"), inbound("3", "third")]);
    FAKE_DELETED.with(|deleted| deleted.borrow_mut().clear());
    let mut params = std::collections::BTreeMap::new();
    params.insert("emailServerId".into(), Value::string("TEST_EACH"));
    let _ = crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params);
    let errors = context.message().errors().join("\n");
    assert!(errors.contains("rule broke"), "{errors}");
    // The poll's own transaction rolled back on that error; the saved messages were committed before it.
    let saved = |id: &str| {
        let _authz = context.disable_authz();
        facade
            .find("moqui.basic.email.EmailMessage")
            .unwrap()
            .eq("messageId", Value::string(format!("<each-{id}@example.com>")))
            .one()
            .unwrap()
            .is_some()
    };
    assert!(saved("1"));
    assert!(!saved("2"), "the failed message's row rolled back");
    assert!(saved("3"));
    let deleted = FAKE_DELETED.with(|deleted| deleted.borrow().clone());
    assert_eq!(deleted, vec!["1".to_string(), "3".to_string()]);
}

#[test]
fn the_message_id_host_is_the_domain_of_the_from_address() {
    let id = super::new_message_id("Ann Smith <ann@example.com>").unwrap();
    assert!(id.starts_with('<') && id.ends_with("@example.com>"), "{id}");
    assert_eq!(id.matches('>').count(), 1, "{id}");
    assert!(super::new_message_id("not an address")
        .unwrap()
        .ends_with("@localhost>"));
}

#[test]
fn allowed_domains_match_the_domain_or_below_and_a_display_name_is_an_address() {
    let domains = vec!["example.com".to_string()];
    assert!(super::domain_allowed("a@example.com", &domains));
    assert!(super::domain_allowed("Ann <a@Mail.Example.com>", &domains));
    assert!(!super::domain_allowed("a@evilexample.com", &domains));
    assert!(!super::domain_allowed("a@example.com.evil.org", &domains));
    let mailbox = super::mailbox("Ann Smith <ann@example.com>", "").unwrap();
    assert_eq!(mailbox.email.to_string(), "ann@example.com");
    assert_eq!(mailbox.name.as_deref(), Some("Ann Smith"));
    assert_eq!(
        super::mailbox("ann@example.com", "Named").unwrap().name.as_deref(),
        Some("Named")
    );
}
