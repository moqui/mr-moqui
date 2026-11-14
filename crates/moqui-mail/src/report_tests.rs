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

use super::*;
use moqui_core::context::ExecutionContextFactory;

const STATUS: &str = "Reporting-MTA: dns; mx.example.net\r
Original-Envelope-Id: D100\r
Arrival-Date: Fri, 13 Nov 2026 10:00:00 +0000\r
\r
Final-Recipient: rfc822; <Nobody@Example.net>\r
Original-Recipient: rfc822; nobody@example.net\r
Action: failed\r
Status: 5.1.1 (user unknown)\r
Diagnostic-Code: smtp; 550 5.1.1 <nobody@example.net>:\r
 Recipient address rejected: User unknown\r
\r
Final-Recipient: rfc822; later@example.net\r
Action: delayed\r
Status: 4.4.1\r
";

const FEEDBACK: &str = "Feedback-Type: Abuse\r
User-Agent: SomeGenerator/1.0\r
Version: 1\r
Original-Mail-From: <bounces+D200@bounce.retail.example>\r
Original-Rcpt-To: <olive@example.org>\r
Arrival-Date: Thu, 12 Nov 2026 08:00:00 EDT\r
Reported-Domain: retail.example\r
";

const ORIGINAL: &str = "Return-Path: <bounces+D200@bounce.retail.example>\r
Message-ID: <abc.123@retail.example>\r
From: Retail news <news@retail.example>\r
To: olive@example.org\r
Subject: Spring\r
\r
Hello\r
";

/// A delivery status part: the message's group, then each recipient with its address, action, status (its code alone)
/// and folded diagnostic; a part with no message group starts with a recipient.
#[test]
fn a_delivery_status_part_names_each_recipient_and_its_status() {
    let status = delivery_status(STATUS);
    assert_eq!(status.original_envelope_id, "D100");
    assert_eq!(status.recipients.len(), 2);
    let first = &status.recipients[0];
    assert_eq!(first.final_recipient, "Nobody@Example.net");
    assert_eq!(first.original_recipient, "nobody@example.net");
    assert_eq!((first.action.as_str(), first.status.as_str()), ("failed", "5.1.1"));
    assert!(first.diagnostic.contains("550 5.1.1") && first.diagnostic.contains("User unknown"));
    assert_eq!(status.recipients[1].action, "delayed");
    let bare = delivery_status("Final-Recipient: rfc822; a@b.example\nAction: failed\nStatus: 5.0.0\n");
    assert_eq!(bare.recipients.len(), 1);
    assert!(bare.original_envelope_id.is_empty());
    assert_eq!(delivery_status(""), DeliveryStatus::default());
}

/// An abuse report's fields, and a whole message's feedback part; a message with none is no report.
#[test]
fn an_abuse_report_names_its_type_and_the_original_envelope() {
    let report = feedback_report(FEEDBACK);
    assert_eq!(report.feedback_type, "abuse");
    assert_eq!(report.original_mail_from, "bounces+D200@bounce.retail.example");
    assert_eq!(report.original_rcpt_to, ["olive@example.org"]);
    assert_eq!(report.reported_domain, "retail.example");
    let raw = format!(
        "From: fbl@isp.example\r\nContent-Type: multipart/report; report-type=feedback-report; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain\r\n\r\nThis is an abuse report\r\n\
         --b1\r\nContent-Type: message/feedback-report\r\n\r\n{FEEDBACK}\r\n\
         --b1\r\nContent-Type: message/rfc822\r\n\r\n{ORIGINAL}\r\n--b1--\r\n"
    );
    assert_eq!(parse_arf(raw.as_bytes()), Some(report));
    assert_eq!(parse_arf(b"Subject: Out of office\r\n\r\nAway\r\n"), None);
}

/// A stored report from its parts: the kind, the returned message's Message-ID, and the addresses it came to (its
/// Delivered-To, else its To); an ordinary message is no report.
#[test]
fn a_stored_report_reads_its_parts_and_the_address_it_came_to() {
    let headers = "Return-Path: <>\r\nDelivered-To: bounces+D100@bounce.retail.example\r\nSubject: Undelivered\r\n";
    let dsn = stored_report(
        headers,
        "bounces+D100@bounce.retail.example",
        &[
            ("message/delivery-status".to_string(), STATUS.as_bytes().to_vec()),
            ("text/rfc822-headers".to_string(), ORIGINAL.as_bytes().to_vec()),
        ],
    );
    assert_eq!(dsn.report_type, "dsn");
    assert_eq!(dsn.status.recipients[0].status, "5.1.1");
    assert_eq!(dsn.original_message_id, "<abc.123@retail.example>");
    assert_eq!(dsn.delivered_to, ["bounces+D100@bounce.retail.example"]);
    let arf = stored_report(
        "Subject: Abuse report\r\n",
        "Retail abuse <bounces+D200@bounce.retail.example>",
        &[
            ("message/feedback-report".to_string(), FEEDBACK.as_bytes().to_vec()),
            ("message/rfc822".to_string(), ORIGINAL.as_bytes().to_vec()),
        ],
    );
    assert_eq!(arf.report_type, "arf");
    assert_eq!(arf.feedback.feedback_type, "abuse");
    assert_eq!(arf.delivered_to, ["bounces+D200@bounce.retail.example"]);
    let out = report_out(&arf);
    assert_eq!(out["reportType"], Value::string("arf"));
    assert_eq!(out["originalMessageId"], Value::string("<abc.123@retail.example>"));
    let plain = stored_report(
        "Subject: Hello\r\n",
        "ann@example.com",
        &[("text/plain".to_string(), b"hi".to_vec())],
    );
    assert!(plain.report_type.is_empty());
}

/// read#DeliveryReport on a stored message: its report part and returned message as the store keeps them
/// (EmailMessageAttachment files); a message that is not there is an error.
#[test]
fn a_stored_message_is_read_as_a_report_by_its_files() {
    let facade = moqui_core::testing::open();
    let context = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut message = facade.make_value(MESSAGE).unwrap();
    for (name, value) in [
        ("emailMessageId", "REPORT1"),
        ("statusId", "ES_RECEIVED"),
        ("subject", "Undelivered Mail Returned to Sender"),
        ("toAddresses", "bounces+D100@bounce.retail.example"),
        (
            "headersString",
            "Delivered-To: bounces+D100@bounce.retail.example\r\nSubject: Undelivered",
        ),
    ] {
        message.set(name, Value::string(value));
    }
    message.create(&facade).unwrap();
    // the store keeps a part as a DbResource file; any location the resource facade reads serves here
    let dir = std::env::temp_dir().join(format!("moqui-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (id, kind, text) in [
        ("RA1", "message/delivery-status", STATUS),
        ("RA2", "text/rfc822-headers", ORIGINAL),
    ] {
        let path = dir.join(id);
        std::fs::write(&path, text).unwrap();
        let location = format!("file://{}", path.display());
        let mut row = facade.make_value(ATTACHMENT).unwrap();
        for (name, value) in [
            ("emailMessageAttachmentId", id),
            ("emailMessageId", "REPORT1"),
            ("attachmentLocation", location.as_str()),
            ("contentType", kind),
        ] {
            row.set(name, Value::string(value));
        }
        row.create(&facade).unwrap();
    }
    let params = BTreeMap::from([("emailMessageId".to_string(), Value::string("REPORT1"))]);
    let out = read_delivery_report(&params, &facade).unwrap();
    assert_eq!(out["reportType"], Value::string("dsn"));
    assert_eq!(out["originalMessageId"], Value::string("<abc.123@retail.example>"));
    let Value::List(recipients) = &out["recipientList"] else {
        panic!("recipientList is a list: {out:?}")
    };
    assert_eq!(recipients.len(), 2);
    let missing = BTreeMap::from([("emailMessageId".to_string(), Value::string("NOPE"))]);
    assert!(read_delivery_report(&missing, &facade).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hostile and broken reports answer what they can and never panic: no field separator, a folded line with no field
/// before it, bytes that are not UTF-8 (a multi-byte character cut at the part limit too), a feedback part with no body,
/// a marker inside a multi-byte text, and far more recipients than are read.
#[test]
fn a_hostile_or_broken_report_never_panics() {
    for body in [
        "",
        "\n\n\n",
        " folded first line\r\n\tmore\r\n",
        "no colon at all\r\nFinal-Recipient\r\n",
        "Final-Recipient: ;\r\nAction:\r\nStatus:\r\n",
        "Final-Recipient: rfc822; <>\r\nAction: failed\r\nStatus: 5.1.1\r\n",
        "\u{feff}Final-Recipient: rfc822; é@ü.example\r\nAction: FAILED\r\nStatus: \r\n",
    ] {
        let status = delivery_status(body);
        assert!(status
            .recipients
            .iter()
            .all(|recipient| !recipient.final_recipient.is_empty()));
        let _ = feedback_report(body);
    }
    let many: String = (0..1000)
        .map(|n| format!("\r\nFinal-Recipient: rfc822; r{n}@example.net\r\nAction: failed\r\nStatus: 5.1.1\r\n"))
        .collect();
    assert_eq!(
        delivery_status(&format!("Reporting-MTA: dns; x\r\n{many}"))
            .recipients
            .len(),
        RECIPIENTS_MAX
    );
    let rcpts: String = (0..1000)
        .map(|n| format!("Original-Rcpt-To: r{n}@example.net\r\n"))
        .collect();
    assert_eq!(feedback_report(&rcpts).original_rcpt_to.len(), RECIPIENTS_MAX);
    for raw in [
        &b"Content-Type: message/feedback-report"[..],
        b"Content-Type: message/feedback-report\r\nno empty line",
        b"\xff\xfe\xc3Content-Type: message/feedback-report\r\n\r\n\xc3\x28Feedback-Type: abuse\r\n",
        "Subject: ü\u{1F600}content-type: MESSAGE/FEEDBACK-REPORT\r\n\r\nFeedback-Type: fraud\r\n".as_bytes(),
    ] {
        let _ = parse_arf(raw);
    }
    // a part over the limit cut inside a multi-byte character, and broken Message-IDs
    let mut long = "Message-ID: <x@y>\r\n".as_bytes().to_vec();
    long.resize(PART_MAX - 1, b'a');
    long.extend_from_slice("é".repeat(4).as_bytes());
    let parts = [
        ("message/rfc822".to_string(), long),
        (
            "message/delivery-status".to_string(),
            vec![0xff, 0xfe, 0x00, b':', b'\n'],
        ),
    ];
    let report = stored_report(
        "Delivered-To:\r\n\tbounces+1@x.example\r\nDelivered-To: <>\r\n",
        "",
        &parts,
    );
    assert_eq!(report.report_type, "dsn");
    assert_eq!(report.original_message_id, "<x@y>");
    assert_eq!(report.delivered_to, ["bounces+1@x.example"]);
    assert_eq!(message_id_of("Message-ID: <>\r\n"), "");
    assert_eq!(message_id_of("Message-ID:\r\n"), "");
    assert_eq!(
        stored_report("", "<>, ,no-address,a@b.example", &[]).delivered_to,
        ["a@b.example"]
    );
}
