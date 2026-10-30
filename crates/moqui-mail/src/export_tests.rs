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

use moqui_lib::object::{basic_convert, to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;

fn text(value: &str) -> Value {
    Value::string(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    moqui_core::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(&format!("{}.{name}", super::SERVICES), &parameters).unwrap();
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    (out, errors)
}

/// A sent message with an attachment and an inline image exports as one RFC 5322 file: the headers of the message, the
/// text and HTML as alternatives, the image by its Content-ID beside the HTML, and the file's bytes as they are.
#[test]
fn a_sent_message_exports_with_its_files() {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let framework = moqui_core::context::framework_dir().join("service/org/moqui");
    let failures = moqui_core::service::lint(&[
        framework.join("EmailServices.xml"),
        framework.join("impl/EmailServices.xml"),
        framework.join("impl/EmailClientServices.xml"),
        framework.join("impl/ShellServices.xml"),
    ])
    .unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    for id in ["alice", "eve"] {
        row(
            &facade,
            "moqui.security.UserAccount",
            &[
                ("userId", text(id)),
                ("username", text(id)),
                ("emailAddress", text(&format!("{id}@example.com"))),
            ],
        );
    }
    login("alice");
    let (created, errors) = call(
        "create#EmailAccount",
        &[
            ("accountKindEnumId", text("EakShared")),
            ("emailAddress", text("team@example.com")),
            ("accountName", text("Team")),
            ("smtpHost", text("smtp.example.com")),
            ("smtpStartTls", text("N")),
            ("storeHost", text("imap.example.com")),
            ("mailPassword", text("secret")),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    let account = to_plain_string(created.get("emailAccountId").unwrap());
    let sent = basic_convert(&Value::I64(1_760_000_000_000), "java.sql.Timestamp").value;
    row(
        &facade,
        super::MESSAGE,
        &[
            ("emailMessageId", text("EXP1")),
            ("emailAccountId", text(&account)),
            ("statusId", text("ES_SENT")),
            ("subject", text("Plan für Q3")),
            ("body", text("<p>See the plan <img src=\"cid:logo1\"></p>")),
            ("bodyText", text("See the plan")),
            ("fromAddress", text("team@example.com")),
            ("fromName", text("Team")),
            ("toAddresses", text("bob@example.com")),
            ("ccAddresses", text("\"Doe, Jane\" <jane@example.com>")),
            ("messageId", text("<exp1@example.com>")),
            ("inReplyTo", text("<earlier@example.com>")),
            ("sentDate", sent),
        ],
    );
    let pdf: Vec<u8> = (0..=255u8).chain([0xFF, 0x00, 0x0D, 0x0A]).collect();
    let resource = ctx.factory().resource();
    resource
        .write_bytes("dbresource://test/export/report.pdf", &pdf)
        .unwrap();
    resource
        .write_bytes("dbresource://test/export/logo.png", b"\x89PNG\r\n\x1a\nlogo")
        .unwrap();
    for (id, name, kind, location, inline) in [
        (
            "EXPA1",
            "report.pdf",
            "application/pdf",
            "dbresource://test/export/report.pdf",
            "",
        ),
        (
            "EXPA2",
            "logo.png",
            "image/png",
            "dbresource://test/export/logo.png",
            "logo1",
        ),
    ] {
        let mut fields = vec![
            ("emailMessageAttachmentId", text(id)),
            ("emailMessageId", text("EXP1")),
            ("fileName", text(name)),
            ("contentType", text(kind)),
            ("attachmentLocation", text(location)),
        ];
        if !inline.is_empty() {
            fields.push(("isInline", text("Y")));
            fields.push(("contentId", text(&format!("<{inline}>"))));
        }
        row(&facade, "moqui.basic.email.EmailMessageAttachment", &fields);
    }

    let (out, errors) = call("export#EmailMessage", &[("emailMessageId", text("EXP1"))]);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(out.get("contentType").unwrap()), "message/rfc822");
    assert_eq!(to_plain_string(out.get("fileName").unwrap()), "Plan für Q3.eml");
    let Some(Value::Bytes(bytes)) = out.get("mimeBytes") else {
        panic!("no mimeBytes: {out:?}")
    };
    let mime = to_plain_string(out.get("mimeText").unwrap());
    assert_eq!(mime.as_bytes(), bytes.as_slice());
    assert!(bytes.is_ascii(), "{mime}");
    for header in [
        "Message-ID: <exp1@example.com>",
        "In-Reply-To: <earlier@example.com>",
        "multipart/mixed",
        "multipart/related",
    ] {
        assert!(mime.contains(header), "{header} in {mime}");
    }
    let parsed = mail_parser::MessageParser::default().parse(bytes.as_slice()).unwrap();
    assert_eq!(parsed.subject(), Some("Plan für Q3"));
    assert_eq!(
        parsed
            .from()
            .and_then(|from| from.first())
            .and_then(|from| from.address()),
        Some("team@example.com")
    );
    assert_eq!(
        parsed.cc().and_then(|cc| cc.first()).and_then(|cc| cc.name()),
        Some("Doe, Jane")
    );
    assert_eq!(parsed.date().map(|date| date.to_timestamp()), Some(1_760_000_000));
    assert_eq!(parsed.body_text(0).as_deref().map(str::trim), Some("See the plan"));
    assert!(parsed.body_html(0).is_some_and(|html| html.contains("cid:logo1")));
    let files: Vec<(String, Vec<u8>, Option<String>)> = parsed
        .attachments()
        .map(|part| {
            (
                mail_parser::MimeHeaders::attachment_name(part)
                    .unwrap_or_default()
                    .to_string(),
                part.contents().to_vec(),
                mail_parser::MimeHeaders::content_id(part).map(str::to_string),
            )
        })
        .collect();
    assert!(
        files.iter().any(|(name, body, _)| name == "report.pdf" && body == &pdf),
        "{files:?}"
    );
    assert!(
        files
            .iter()
            .any(|(_, body, cid)| body.as_slice() == b"\x89PNG\r\n\x1a\nlogo" && cid.as_deref() == Some("logo1")),
        "{files:?}"
    );

    // a received message: its stored headers are in the file
    row(
        &facade,
        super::MESSAGE,
        &[
            ("emailMessageId", text("EXP2")),
            ("emailAccountId", text(&account)),
            ("statusId", text("ES_RECEIVED")),
            ("subject", text("News")),
            ("bodyText", text("Hello")),
            ("fromAddress", text("news@example.org")),
            ("toAddresses", text("team@example.com")),
            ("messageId", text("<news2@example.org>")),
            (
                "headersString",
                text("Received: from mail.example.org\n\tby mx.example.com\nFrom: news@example.org\nTo: team@example.com\nSubject: News\nMessage-ID: <news2@example.org>\nList-Unsubscribe: <mailto:leave@example.org>\nContent-Type: text/plain"),
            ),
        ],
    );
    let (out, errors) = call("export#EmailMessage", &[("emailMessageId", text("EXP2"))]);
    assert!(errors.is_empty(), "{errors}");
    let mime = to_plain_string(out.get("mimeText").unwrap());
    assert!(
        mime.starts_with("Received: from mail.example.org\r\n\tby mx.example.com\r\n"),
        "{mime}"
    );
    assert!(
        mime.contains("List-Unsubscribe: <mailto:leave@example.org>\r\n"),
        "{mime}"
    );

    // a user who does not reach the account gets nothing
    login("eve");
    let (out, errors) = call("export#EmailMessage", &[("emailMessageId", text("EXP1"))]);
    assert!(errors.contains("Email message EXP1 not found"), "{errors}");
    assert!(!out.contains_key("mimeBytes"));
}

/// A received message keeps the header block it came with (folded fields too), with the MIME structure of the body
/// made again and not the original's; a stray line is left out, and a draft's own headers are the made ones.
#[test]
fn a_received_message_keeps_its_stored_headers() {
    let stored = concat!(
        "From MAILER-DAEMON Mon Oct 19 10:00:00 2026\n",
        "Received: from mail.example.org (mail.example.org [192.0.2.1])\n",
        "\tby mx.example.com with ESMTPS id abc123\n",
        "\tfor <team@example.com>; Mon, 19 Oct 2026 10:00:00 +0000\n",
        "Authentication-Results: mx.example.com; dkim=pass header.d=example.org; spf=pass\n",
        "DKIM-Signature: v=1; a=rsa-sha256; d=example.org; s=s1;\n",
        " h=from:to:subject; bh=AAAA; b=BBBB\n",
        "From: \"News\" <news@example.org>\n",
        "To: team@example.com\n",
        "Subject: =?utf-8?q?Caf=C3=A9_news?=\n",
        "Date: Mon, 19 Oct 2026 10:00:00 +0000\n",
        "Message-ID: <news1@example.org>\n",
        "List-Unsubscribe: <mailto:leave@example.org>, <https://example.org/leave?u=1>\n",
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click\n",
        "MIME-Version: 1.0\n",
        "Content-Type: multipart/alternative; boundary=\"old-boundary\"",
    );
    let letter = super::Letter {
        subject: "Café news".to_string(),
        from_address: "news@example.org".to_string(),
        from_name: "News".to_string(),
        to: vec!["team@example.com".to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: vec!["identity@example.com".to_string()],
        bounce: String::new(),
        body: "<p>Hello</p>".to_string(),
        body_text: "Hello".to_string(),
        attachments: Vec::new(),
        inline: Vec::new(),
        message_id: "<news1@example.org>".to_string(),
        in_reply_to: String::new(),
        references: String::new(),
        date: Some(1_760_000_000_000),
        extra_headers: Vec::new(),
    };
    let built = super::build_lettre(&letter).unwrap().formatted();
    let mime = String::from_utf8(super::with_received_headers(built.clone(), stored)).unwrap();
    let (head, body) = mime.split_once("\r\n\r\n").expect("a header block");
    for expected in [
        "Received: from mail.example.org (mail.example.org [192.0.2.1])\r\n\tby mx.example.com with ESMTPS id abc123\r\n\tfor <team@example.com>; Mon, 19 Oct 2026 10:00:00 +0000\r\n",
        "Authentication-Results: mx.example.com; dkim=pass",
        "DKIM-Signature: v=1; a=rsa-sha256; d=example.org; s=s1;\r\n h=from:to:subject; bh=AAAA; b=BBBB\r\n",
        "List-Unsubscribe: <mailto:leave@example.org>, <https://example.org/leave?u=1>\r\n",
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        "Subject: =?utf-8?q?Caf=C3=A9_news?=\r\n",
    ] {
        assert!(format!("{head}\r\n").contains(expected), "{expected} in {head}");
    }
    assert!(head.starts_with("Received: "), "{head}");
    assert!(
        !head.contains("MAILER-DAEMON") && !head.contains("old-boundary"),
        "{head}"
    );
    // the made structure, once; not the identity's Reply-To the builder would add
    assert_eq!(head.matches("MIME-Version:").count(), 1, "{head}");
    assert_eq!(head.to_ascii_lowercase().matches("content-type:").count(), 1, "{head}");
    assert!(!head.contains("identity@example.com"), "{head}");
    let made = String::from_utf8_lossy(&built).into_owned();
    assert!(made.ends_with(body), "the made body");
    let parsed = mail_parser::MessageParser::default().parse(mime.as_bytes()).unwrap();
    assert_eq!(parsed.subject(), Some("Café news"));
    assert_eq!(parsed.body_text(0).as_deref().map(str::trim), Some("Hello"));
    assert!(parsed.body_html(0).is_some_and(|html| html.contains("<p>Hello</p>")));
    assert_eq!(parsed.message_id(), Some("news1@example.org"));
    assert!(parsed.header("List-Unsubscribe").is_some());

    // only the client's own headers (an automatic reply's): the message as made
    let own = super::with_received_headers(
        built.clone(),
        "Auto-Submitted: auto-replied\nX-Auto-Response-Suppress: All",
    );
    assert_eq!(own, built);
    assert_eq!(super::with_received_headers(built.clone(), ""), built);
    // a block cut at the most the store keeps loses its last field
    let long = format!("Received: from a\nX-Long: {}", "x".repeat(super::HEADERS_MAX));
    let cut = String::from_utf8(super::with_received_headers(built, &long)).unwrap();
    assert!(
        cut.starts_with("Received: from a\r\n") && !cut.contains("X-Long"),
        "{}",
        &cut[..80]
    );
}

#[test]
fn a_subject_is_a_safe_file_name() {
    assert_eq!(super::file_name("Re: Q3 plan / budget?"), "Re_ Q3 plan _ budget_.eml");
    assert_eq!(super::file_name(""), "message.eml");
    assert_eq!(super::file_name("../.."), "_.eml");
    assert_eq!(super::file_name("Café menu"), "Café menu.eml");
    assert_eq!(super::file_name(&"x".repeat(200)).len(), 84);
}
