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

//! The inbound SMTP listener (`inbound-smtp`) on the seeded embedded Postgres, driven by a scripted SMTP client on
//! localhost as the edge gateway would: a connection from an address outside `smtp_allowed_addresses` is refused at
//! connect; RCPT TO takes a hosted account's address with or without a plus tag, a shared account's address and
//! identity, and refuses any other with 550; one message to both accounts is stored once in each Inbox with its file as
//! a DbResource, the account rule and the email ECAs run once on each copy (a failing ECA logged, the other running);
//! the gateway's spam verdict files in Junk; a message over `smtp-server` `max-message-size` is refused with 552; a store that
//! fails answers 451 and the gateway's retry does not double the account that took it. In LMTP mode (with DSN) each
//! recipient gets its own reply: a store that fails for one account is 451 for its recipient only.

#![cfg(all(
    unix,
    feature = "embedded-postgres",
    feature = "postgres",
    feature = "axum",
    feature = "inbound-smtp"
))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use moqui_core::entity::EntityValue;
use moqui_lib::object::{to_plain_string, Value};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.basic.EnumerationType enumTypeId="GateSmtpEca" description="Email ECA runs of the SMTP gate"/>
</entity-facade-xml>"#;

/// A component of the test's own with two email ECA rules: one that always fails, and one that records each message
/// it runs on as an Enumeration row.
const COMPONENT: &str = r#"<component name="gate-smtp"/>"#;
const EMECAS: &str = r#"<emecas>
    <emeca rule-name="GateSmtpFails"><condition><expression>true</expression></condition>
        <actions><script>throw new Error('gate ECA boom')</script></actions></emeca>
    <emeca rule-name="GateSmtpSeen">
        <actions><script>
            const row = ec.entity.makeValue('moqui.basic.Enumeration')
            row.setSequencedIdPrimary()
            row.setFields({enumTypeId: 'GateSmtpEca', description: emailMessageId, enumCode: emailAccountId}, true, null, null)
            row.create()
        </script></actions></emeca>
</emecas>"#;

const FILES: &str = "Authentication-Results: gw.example.com; spf=pass smtp.mailfrom=example.org; dkim=pass\r
Authentication-Results: evil.example; dmarc=pass\r
Message-ID: <files@example.org>\r
From: Carol <carol@example.org>\r
To: sales@example.com, help@example.com\r
Subject: Files\r
Date: Mon, 19 Oct 2026 11:00:00 +0000\r
MIME-Version: 1.0\r
Content-Type: multipart/mixed; boundary=\"mix\"\r
\r
--mix\r
Content-Type: text/plain; charset=utf-8\r
\r
The report is attached.\r
.A line that starts with a dot.\r
--mix\r
Content-Type: application/pdf; name=\"report.pdf\"\r
Content-Disposition: attachment; filename=\"report.pdf\"\r
Content-Transfer-Encoding: base64\r
\r
JVBERi0xLjQK\r
--mix--\r
";

fn sync(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let result = moqui_core::service::sync(name, &parameters).unwrap();
    let errors = moqui_core::context::ExecutionContext::with_current(|ctx| {
        let text = ctx.message().errors().join("\n");
        ctx.message().clear();
        text
    })
    .unwrap_or_default();
    assert_eq!(errors, "", "{name}");
    result
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn out(result: &BTreeMap<String, Value>, name: &str) -> String {
    result.get(name).map(to_plain_string).unwrap_or_default()
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

/// A scripted SMTP client: one reply is its lines joined with `|`, empty once the server closed.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn new(stream: TcpStream) -> Self {
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    fn connect(address: SocketAddr) -> Self {
        Self::new(TcpStream::connect(address).unwrap())
    }

    fn reply(&mut self) -> String {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end().to_string();
            let last = line.as_bytes().get(3) != Some(&b'-');
            lines.push(line);
            if last {
                break;
            }
        }
        lines.join("|")
    }

    fn command(&mut self, line: &str) -> String {
        self.writer.write_all(format!("{line}\r\n").as_bytes()).unwrap();
        self.reply()
    }

    /// DATA, then the message dot-stuffed and its end.
    fn data(&mut self, message: &str) -> String {
        assert_eq!(self.command("DATA"), "354 End data with <CR><LF>.<CR><LF>");
        let mut stuffed = String::new();
        for line in message.split_inclusive("\r\n") {
            if line.starts_with('.') {
                stuffed.push('.');
            }
            stuffed.push_str(line);
        }
        stuffed.push_str(".\r\n");
        self.writer.write_all(stuffed.as_bytes()).unwrap();
        self.reply()
    }
}

#[test]
fn the_smtp_listener_stores_gateway_mail_for_hosted_and_shared_accounts() {
    let gate = crate::common::start_with_conf_files(
        "smtp",
        Some(DATA),
        r#"<smtp-server spam-headers="X-Spam-Flag,X-Spam-Status" plus-separator="+" max-message-size="200000"/>
            <email-client idle-enabled="false"/>"#,
        &[
            ("smtp_listen_port", "0"),
            ("smtp_allowed_addresses", "127.0.0.1"),
            ("smtp_hostname", "mx.gate.test"),
            ("smtp_authserv_id", "gw.example.com"),
        ],
        &[
            ("component/gate-smtp/component.xml", COMPONENT),
            ("component/gate-smtp/service/GateSmtp.emecas.xml", EMECAS),
        ],
    );
    let address = gate.server.smtp_address().expect("the SMTP listener listens");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let entity = factory.entity().unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    // set up and checked as internal work: john.doe owns the accounts and the rule
    let _authz = context.disable_authz();
    let account = |kind: &str, email: &str| {
        out(
            &sync(
                "org.moqui.impl.EmailClientServices.create#EmailAccount",
                &[("accountKindEnumId", text(kind)), ("emailAddress", text(email))],
            ),
            "emailAccountId",
        )
    };
    let sales = account("EakHosted", "sales@example.com");
    let support = account("EakShared", "support@example.com");
    let broken = account("EakHosted", "broken@example.com");
    sync(
        "org.moqui.impl.EmailClientServices.create#EmailIdentity",
        &[
            ("emailAccountId", text(&support)),
            ("emailAddress", text("help@example.com")),
        ],
    );
    let rule = out(
        &sync(
            "org.moqui.impl.EmailClientServices.create#EmailRule",
            &[
                ("ruleName", text("Files")),
                (
                    "conditionList",
                    Value::List(vec![Value::Map(
                        [("field", "subject"), ("operator", "contains"), ("value", "Files")]
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), text(value)))
                            .collect(),
                    )]),
                ),
                (
                    "actionList",
                    Value::List(vec![Value::Map(
                        [("action".to_string(), text("flag"))].into_iter().collect(),
                    )]),
                ),
            ],
        ),
        "emailRuleId",
    );
    let rows = |entity_name: &str, conditions: &[(&str, &str)]| -> Vec<EntityValue> {
        let mut find = entity.find(entity_name).unwrap().disable_authz();
        for (name, value) in conditions {
            find = find.eq(name, text(value));
        }
        find.list().unwrap().values().to_vec()
    };
    let folder = |account: &str, kind: &str| -> String {
        field(
            &rows(
                "moqui.basic.email.EmailFolder",
                &[("emailAccountId", account), ("folderKindEnumId", kind)],
            )[0],
            "emailFolderId",
        )
    };
    // the broken account has no Inbox, so a store for it fails
    let mut inbox = entity.make_value("moqui.basic.email.EmailFolder").unwrap();
    inbox.set("emailFolderId", text(&folder(&broken, "EfkInbox")));
    inbox.delete(&entity).unwrap();

    // an address outside the allow-list is refused at connect; macOS routes only 127.0.0.1 unless lo0 has an alias
    let other = moqui_host::edge::block_on(async move {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        match socket.bind("127.0.0.2:0".parse().unwrap()) {
            Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => {
                eprintln!("skipping the 127.0.0.2 connect: this host does not route 127.0.0.2 ({err})");
                None
            }
            bound => {
                bound.unwrap();
                Some(socket.connect(address).await.unwrap().into_std().unwrap())
            }
        }
    })
    .unwrap();
    if let Some(other) = other {
        other.set_nonblocking(false).unwrap();
        let mut refused = Client::new(other);
        assert_eq!(
            refused.reply(),
            "554 5.7.1 mx.gate.test does not take mail from this address"
        );
        assert_eq!(refused.reply(), "");
    }

    let mut gateway = Client::connect(address);
    assert_eq!(gateway.reply(), "220 mx.gate.test ESMTP mr-moqui");
    assert!(gateway.command("EHLO gw.example.com").contains("250-SIZE 200000"));
    assert_eq!(
        gateway.command("MAIL FROM:<carol@example.org> SIZE=900"),
        "250 2.1.0 Ok"
    );
    assert_eq!(
        gateway.command("RCPT TO:<nobody@example.com>"),
        "550 5.1.1 <nobody@example.com>: Recipient address rejected: User unknown"
    );
    assert_eq!(gateway.command("RCPT TO:<sales@example.com>"), "250 2.1.5 Ok");
    assert_eq!(gateway.command("RCPT TO:<Sales+reports@Example.com>"), "250 2.1.5 Ok");
    assert_eq!(gateway.command("RCPT TO:<help@example.com>"), "250 2.1.5 Ok");
    assert_eq!(gateway.command("RCPT TO:<SUPPORT@example.com>"), "250 2.1.5 Ok");
    assert!(gateway.data(FILES).starts_with("250 2.0.0 Ok: stored as "));

    let ecas = || rows("moqui.basic.Enumeration", &[("enumTypeId", "GateSmtpEca")]);
    for account in [&sales, &support] {
        let stored = rows(
            "moqui.basic.email.EmailMessage",
            &[("emailAccountId", account), ("messageId", "<files@example.org>")],
        );
        assert_eq!(stored.len(), 1, "once in account {account}");
        let message = &stored[0];
        let id = field(message, "emailMessageId");
        assert_eq!(field(message, "emailFolderId"), folder(account, "EfkInbox"));
        assert_eq!(field(message, "statusId"), "ES_RECEIVED");
        assert_eq!(field(message, "isFlagged"), "Y", "the rule ran");
        assert!(field(message, "bodyText").contains("\n.A line that starts with a dot."));
        let headers = field(message, "headersString");
        assert!(headers.starts_with("Return-Path: <carol@example.org>"), "{headers}");
        assert!(
            headers.contains("by mx.gate.test (mr-moqui) with ESMTP id "),
            "{headers}"
        );
        assert!(headers.contains("Authentication-Results: gw.example.com;"), "{headers}");
        assert!(!headers.contains("evil.example"), "{headers}");
        let files = rows("moqui.basic.email.EmailMessageAttachment", &[("emailMessageId", &id)]);
        assert_eq!(files.len(), 1);
        let location = field(&files[0], "attachmentLocation");
        let stat = factory
            .resource()
            .stat(&location)
            .unwrap()
            .expect("the file is a DbResource");
        assert_eq!(stat.size, 9);
        assert_eq!(
            ecas().iter().filter(|row| field(row, "description") == id).count(),
            1,
            "the email ECA ran once on {id}"
        );
    }
    let sales_message = rows(
        "moqui.basic.email.EmailMessage",
        &[("emailAccountId", &sales), ("messageId", "<files@example.org>")],
    );
    let headers = field(&sales_message[0], "headersString");
    assert!(
        headers.contains("Delivered-To: sales@example.com\r\nDelivered-To: Sales+reports@Example.com"),
        "{headers}"
    );
    assert_eq!(ecas().len(), 2);
    assert_eq!(
        field(
            &rows("moqui.basic.email.EmailRule", &[("emailRuleId", &rule)])[0],
            "matchedCount"
        ),
        "2"
    );

    // the gateway's spam verdict: Junk, and no ECA or rule runs on it
    assert_eq!(gateway.command("MAIL FROM:<promo@example.net>"), "250 2.1.0 Ok");
    assert_eq!(gateway.command("RCPT TO:<sales@example.com>"), "250 2.1.5 Ok");
    let spam = "Message-ID: <spam@example.net>\r\nFrom: promo@example.net\r\nTo: sales@example.com\r\n\
                Subject: Files for free\r\nX-Spam-Flag: YES\r\n\r\nBuy now\r\n";
    assert!(gateway.data(spam).starts_with("250 2.0.0"));
    let junk = rows(
        "moqui.basic.email.EmailMessage",
        &[("emailAccountId", &sales), ("messageId", "<spam@example.net>")],
    );
    assert_eq!(field(&junk[0], "emailFolderId"), folder(&sales, "EfkJunk"));
    assert_eq!(field(&junk[0], "isFlagged"), "N");
    assert_eq!(ecas().len(), 2);

    // over smtp-server max-message-size: refused when declared, and when sent
    assert_eq!(
        gateway.command("MAIL FROM:<big@example.org> SIZE=300000"),
        "552 5.3.4 Message size exceeds fixed maximum message size"
    );
    assert_eq!(gateway.command("MAIL FROM:<big@example.org>"), "250 2.1.0 Ok");
    assert_eq!(gateway.command("RCPT TO:<sales@example.com>"), "250 2.1.5 Ok");
    let big = format!(
        "Message-ID: <big@example.org>\r\nFrom: big@example.org\r\nSubject: Big\r\n\r\n{}",
        format!("{}\r\n", "x".repeat(998)).repeat(300)
    );
    assert_eq!(
        gateway.data(&big),
        "552 5.3.4 Message size exceeds fixed maximum message size"
    );
    assert!(rows("moqui.basic.email.EmailMessage", &[("messageId", "<big@example.org>")]).is_empty());

    // a store that fails answers 451; the retry does not double the account that took it
    let question =
        "Message-ID: <ask@example.org>\r\nFrom: dan@example.org\r\nTo: broken@example.com, sales@example.com\r\n\
                    Subject: Question\r\n\r\nHello\r\n";
    for _ in 0..2 {
        assert_eq!(gateway.command("MAIL FROM:<dan@example.org>"), "250 2.1.0 Ok");
        assert_eq!(gateway.command("RCPT TO:<broken@example.com>"), "250 2.1.5 Ok");
        assert_eq!(gateway.command("RCPT TO:<sales@example.com>"), "250 2.1.5 Ok");
        assert_eq!(gateway.data(question), "451 4.3.0 Message not stored, try again later");
    }
    assert_eq!(
        rows(
            "moqui.basic.email.EmailMessage",
            &[("emailAccountId", &sales), ("messageId", "<ask@example.org>")]
        )
        .len(),
        1
    );
    assert!(rows("moqui.basic.email.EmailMessage", &[("emailAccountId", &broken)]).is_empty());
    assert_eq!(gateway.command("QUIT"), "221 2.0.0 Bye");
    assert_eq!(gateway.reply(), "");
}

#[test]
fn the_lmtp_listener_answers_each_recipient_by_its_account() {
    let gate = crate::common::start_with_conf_files(
        "lmtp",
        None,
        r#"<smtp-server protocol="lmtp" dsn="true"/>
            <email-client idle-enabled="false"/>"#,
        &[
            ("smtp_listen_port", "0"),
            ("smtp_allowed_addresses", "127.0.0.1"),
            ("smtp_hostname", "mx.gate.test"),
        ],
        &[],
    );
    let address = gate.server.smtp_address().expect("the LMTP listener listens");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let entity = factory.entity().unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let account = |email: &str| {
        out(
            &sync(
                "org.moqui.impl.EmailClientServices.create#EmailAccount",
                &[("accountKindEnumId", text("EakHosted")), ("emailAddress", text(email))],
            ),
            "emailAccountId",
        )
    };
    let sales = account("sales@example.com");
    let broken = account("broken@example.com");
    let rows = |conditions: &[(&str, &str)]| -> Vec<EntityValue> {
        let mut find = entity.find("moqui.basic.email.EmailMessage").unwrap().disable_authz();
        for (name, value) in conditions {
            find = find.eq(name, text(value));
        }
        find.list().unwrap().values().to_vec()
    };
    // the broken account has no Inbox, so a store for it fails
    let inbox = entity
        .find("moqui.basic.email.EmailFolder")
        .unwrap()
        .disable_authz()
        .eq("emailAccountId", text(&broken))
        .eq("folderKindEnumId", text("EfkInbox"))
        .one()
        .unwrap()
        .unwrap();
    inbox.delete(&entity).unwrap();

    let mut gateway = Client::connect(address);
    assert_eq!(gateway.reply(), "220 mx.gate.test LMTP mr-moqui");
    assert_eq!(
        gateway.command("EHLO gw.example.com"),
        "500 5.5.1 This is an LMTP server: use LHLO"
    );
    let lhlo = gateway.command("LHLO gw.example.com");
    assert!(lhlo.contains("|250-CHUNKING|250-DSN|"), "{lhlo}");
    let question =
        "Message-ID: <lmtp@example.org>\r\nFrom: dan@example.org\r\nTo: sales@example.com, broken@example.com\r\n\
         Subject: Question\r\n\r\nHello\r\n";
    for _ in 0..2 {
        assert_eq!(gateway.command("MAIL FROM:<dan@example.org> ENVID=Q1"), "250 2.1.0 Ok");
        assert_eq!(
            gateway.command("RCPT TO:<sales@example.com> NOTIFY=SUCCESS ORCPT=rfc822;Sales+2Blist@example.com"),
            "250 2.1.5 Ok"
        );
        assert_eq!(gateway.command("RCPT TO:<broken@example.com>"), "250 2.1.5 Ok");
        let stored = gateway.data(question);
        assert!(
            stored.starts_with("250 2.1.5 <sales@example.com> Ok: stored as "),
            "{stored}"
        );
        assert_eq!(
            gateway.reply(),
            "451 4.3.0 <broken@example.com>: Message not stored, try again later"
        );
    }
    let kept = rows(&[("emailAccountId", &sales), ("messageId", "<lmtp@example.org>")]);
    assert_eq!(kept.len(), 1, "the resend is not stored twice");
    let headers = field(&kept[0], "headersString");
    assert!(
        headers.contains("Original-Recipient: rfc822;Sales+list@example.com"),
        "{headers}"
    );
    assert!(
        headers.contains("by mx.gate.test (mr-moqui) with LMTP id "),
        "{headers}"
    );
    assert!(rows(&[("emailAccountId", &broken)]).is_empty());
    assert_eq!(gateway.command("QUIT"), "221 2.0.0 Bye");
    assert_eq!(gateway.reply(), "");
}
