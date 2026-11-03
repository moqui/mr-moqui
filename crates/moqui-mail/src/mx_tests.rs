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

//! The MX delivery's pieces: the MTA-STS policy and its MX patterns (and enforce in a session), the retry schedule, a
//! null MX, A and AAAA answers that fail, and one session with a fake MX on 127.0.0.1 (through `mail_mx_override`):
//! each recipient's answer, sent, refused for good or tried again. With the entity facade: a try's answers committed
//! when its report cannot be queued (and not delivered again), and the reports, partial and final.

use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use super::*;

#[test]
fn an_mta_sts_policy_names_the_mx_hosts_it_allows() {
    let text = "version: STSv1\r\nmode: enforce\r\nmx: mail.example.com\r\nmx: *.example.net\r\nmax_age: 86400\r\n";
    let policy = sts::Policy::parse(text, "20261101").unwrap();
    assert!(policy.enforce);
    assert_eq!(policy.max_age, Duration::from_secs(86_400));
    assert!(policy.allows("mail.example.com."));
    assert!(policy.allows("MX1.example.net"));
    assert!(!policy.allows("a.b.example.net"), "a wildcard is one label");
    assert!(!policy.allows("example.net"));
    assert!(!policy.allows("evil.example.com"));
    let testing = sts::Policy::parse("version: STSv1\nmode: testing\nmx: mail.example.com\n", "1").unwrap();
    assert!(!testing.enforce);
    assert!(sts::Policy::parse("mode: enforce\n", "1").is_none(), "no version");
    assert!(sts::Policy::parse("version: STSv1\n", "1").is_none(), "no mode");
}

#[test]
fn the_retry_wait_doubles_to_four_hours() {
    assert_eq!(retry_after(1), Duration::from_secs(300));
    assert_eq!(retry_after(2), Duration::from_secs(600));
    assert_eq!(retry_after(5), Duration::from_secs(4800));
    assert_eq!(retry_after(7), Duration::from_secs(4 * 3600));
    assert_eq!(retry_after(40), Duration::from_secs(4 * 3600));
    assert_eq!(domain_of(" Ann@Example.COM. "), "example.com");
}

/// A fake MX: refuses for good each recipient whose address starts with `bounce`, for now each starting with `later`,
/// takes the rest; keeps each message with its MAIL FROM line first.
fn fake_mx() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let taken = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&taken);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let keep = Arc::clone(&keep);
            std::thread::spawn(move || mx_session(stream, &keep));
        }
    });
    (port, taken)
}

fn mx_session(stream: TcpStream, taken: &Mutex<Vec<String>>) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut say = |line: &str| writer.write_all(format!("{line}\r\n").as_bytes());
    if say("220 mx.test ESMTP").is_err() {
        return;
    }
    let (mut sender, mut accepted) = (String::new(), 0);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let upper = line.trim_end().to_ascii_uppercase();
        let answer = if upper.starts_with("EHLO") {
            "250-mx.test\r\n250 8BITMIME".to_string()
        } else if upper.starts_with("MAIL FROM") {
            sender = line.trim_end().to_string();
            "250 2.1.0 Ok".to_string()
        } else if upper.starts_with("RCPT TO:<BOUNCE") {
            "550 5.1.1 User unknown".to_string()
        } else if upper.starts_with("RCPT TO:<LATER") {
            "451 4.2.0 Mailbox busy".to_string()
        } else if upper.starts_with("RCPT TO") {
            accepted += 1;
            "250 2.1.5 Ok".to_string()
        } else if upper == "DATA" && accepted > 0 {
            if say("354 Go ahead").is_err() {
                return;
            }
            let mut message = String::new();
            loop {
                let mut data = String::new();
                if reader.read_line(&mut data).unwrap_or(0) == 0 {
                    return;
                }
                if data == ".\r\n" {
                    break;
                }
                message.push_str(&data);
            }
            taken.lock().unwrap().push(format!("{sender}\r\n{message}"));
            "250 2.0.0 Ok: queued as MX1".to_string()
        } else if upper == "QUIT" {
            let _ = say("221 2.0.0 Bye");
            return;
        } else {
            "250 2.0.0 Ok".to_string()
        };
        if say(&answer).is_err() {
            return;
        }
    }
}

#[test]
fn a_session_answers_each_recipient_and_a_host_not_reached_waits() {
    let (port, taken) = fake_mx();
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    moqui_lib::env::set_property(
        "mail_mx_override",
        format!("mx-unit.test=127.0.0.1:{port},mx-closed.test=127.0.0.1:{closed}"),
    );
    let raw = b"From: a@example.com\r\nTo: ok@mx-unit.test\r\nSubject: Hi\r\n\r\nHello\r\n";
    let recipients: Vec<String> = ["ok@mx-unit.test", "bounce@mx-unit.test", "later@mx-unit.test"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    let answers = deliver_domain("mx-unit.test", "bounce+item1@bounce.test", &recipients, raw, deadline);
    assert!(
        matches!(&answers[0].1, Answer::Sent(reply) if reply.starts_with("250")),
        "{answers:?}"
    );
    assert!(
        matches!(&answers[1].1, Answer::Refused(reply) if reply.contains("550")),
        "{answers:?}"
    );
    assert!(
        matches!(&answers[2].1, Answer::Later(reply) if reply.contains("451")),
        "{answers:?}"
    );
    let taken = taken.lock().unwrap().clone();
    assert_eq!(taken.len(), 1);
    assert!(
        taken[0].starts_with("MAIL FROM:<bounce+item1@bounce.test>"),
        "{}",
        taken[0]
    );
    assert!(taken[0].contains("\r\nSubject: Hi\r\n"));
    // a null sender stays null; nobody accepted: no data
    let answers = deliver_domain("mx-unit.test", "", &["bounce@mx-unit.test".to_string()], raw, deadline);
    assert!(matches!(&answers[0].1, Answer::Refused(_)), "{answers:?}");
    // the MX not reached: every recipient waits
    let answers = deliver_domain(
        "mx-closed.test",
        "a@example.com",
        &["x@mx-closed.test".to_string()],
        raw,
        deadline,
    );
    assert!(
        matches!(&answers[0].1, Answer::Later(reply) if reply.contains("no MX of mx-closed.test took the message")),
        "{answers:?}"
    );
}

#[test]
fn a_null_mx_refuses_and_enforce_needs_the_policys_hosts_and_starttls() {
    let records = crate::dns::records();
    records.fix_mx("nullmx.test", &[(0, ".")]);
    assert!(
        matches!(hosts("nullmx.test", None), Err(Answer::Refused(reply)) if reply.starts_with("556 5.1.10")),
        "a null MX"
    );
    records.fix_mx("sts.test", &[(10, "mx1.sts.test")]);
    let policy = sts::Policy::parse("version: STSv1\nmode: enforce\nmx: mx9.sts.test\n", "1").unwrap();
    assert!(
        matches!(hosts("sts.test", Some(&policy)), Err(Answer::Later(reply)) if reply.contains("MTA-STS")),
        "no MX host in the policy"
    );
    // a host without STARTTLS, under enforce: no session
    let (port, taken) = fake_mx();
    let target = Target {
        host: "mx.test",
        address: SocketAddr::from(([127, 0, 0, 1], port)),
        secure: true,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    let refused = session(
        &target,
        "a@example.com",
        &["ok@sts.test".to_string()],
        b"Subject: x\r\n\r\nx\r\n",
    );
    assert!(
        matches!(&refused, Err(reason) if reason.contains("no STARTTLS")),
        "{refused:?}"
    );
    assert!(taken.lock().unwrap().is_empty());
    // the try's time already out: nothing is sent
    let late = Target {
        deadline: Instant::now(),
        ..target
    };
    assert!(session(&late, "a@example.com", &["ok@sts.test".to_string()], b"x").is_err());
}

#[test]
fn an_address_lookup_that_failed_is_not_a_domain_with_no_address() {
    let not_found = || {
        mail_auth::Error::Dns(mail_auth::DnsError::RecordNotFound(
            hickory_resolver::proto::op::ResponseCode::NXDomain,
        ))
    };
    let failed = || mail_auth::Error::Dns(mail_auth::DnsError::Resolver("timed out".to_string()));
    let v4: IpAddr = "192.0.2.1".parse().unwrap();
    assert!(crate::dns::combine_ips("a.test", Err(failed()), Err(not_found())).is_err());
    assert!(crate::dns::combine_ips("a.test", Err(not_found()), Err(failed())).is_err());
    assert_eq!(
        crate::dns::combine_ips("a.test", Err(not_found()), Err(not_found())),
        Ok(Vec::new())
    );
    assert_eq!(
        crate::dns::combine_ips("a.test", Ok(vec![v4]), Err(failed())),
        Ok(vec![v4])
    );
}

/// The reports of a message to `a.test` (two recipients) and `b.test`, with the status kind listed or not.
fn two_domains(conf: &str) -> (moqui_core::context::ExecutionContext, EntityFacade, Vec<EntityValue>) {
    let (ctx, facade) = crate::sync_tests::setup_with(conf);
    let to: Vec<String> = ["ok@a.test", "no@a.test", "x@b.test"]
        .iter()
        .map(ToString::to_string)
        .collect();
    record(
        ctx.factory(),
        &facade,
        "node1",
        "item1",
        "bounce+item1@bounce.test",
        &to,
        &[],
        &[],
        b"Subject: x\r\n\r\nx\r\n",
    )
    .unwrap();
    let rows = facade
        .find(DELIVERY)
        .unwrap()
        .order_by("domain", false)
        .disable_authz()
        .list()
        .unwrap()
        .values()
        .to_vec();
    assert_eq!(rows.len(), 2);
    (ctx, facade, rows)
}

fn row(facade: &EntityFacade, domain: &str) -> EntityValue {
    facade
        .find(DELIVERY)
        .unwrap()
        .eq("domain", Value::string(domain))
        .disable_authz()
        .one()
        .unwrap()
        .unwrap()
}

fn statuses(facade: &EntityFacade) -> Vec<(String, String)> {
    facade
        .find(queue::QUEUE_ITEM)
        .unwrap()
        .eq("kind", Value::string("mailStatus"))
        .order_by("sequenceNum", false)
        .disable_authz()
        .list()
        .unwrap()
        .values()
        .iter()
        .map(|row| {
            let payload: serde_json::Value = serde_json::from_str(&field_text(row, "payload")).unwrap();
            (
                payload["status"].as_str().unwrap_or_default().to_string(),
                payload["reply"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_try_is_committed_when_its_report_cannot_be_queued_and_is_not_tried_again() {
    // no mailStatus kind listed: every report fails to queue
    let (ctx, facade, rows) = two_domains("");
    let _entered = ctx.enter();
    let answers = vec![
        ("ok@a.test".to_string(), Answer::Sent("250 ok".to_string())),
        ("no@a.test".to_string(), Answer::Refused("550 no".to_string())),
    ];
    finish(ctx.factory(), &facade, &rows[0], 0, &answers).unwrap();
    let a = row(&facade, "a.test");
    assert_eq!(
        field_text(&a, "statusEnumId"),
        BOUNCED,
        "done, not to be delivered again"
    );
    assert_eq!(
        field_text(&a, "reportPending"),
        "Y",
        "its report waits for the next round"
    );
    assert_eq!((number(&a, "sentCount"), number(&a, "bouncedCount")), (1, 1));
    assert!(field_text(&a, "refusedText").starts_with("no@a.test: 550 no"));
    assert!(due_rows(&facade, now_millis() + 1000)
        .unwrap()
        .iter()
        .all(|row| field_text(row, "domain") != "a.test"));
    assert!(report(ctx.factory(), &facade, &field_text(&a, "mailDeliveryId")).is_err());
    assert!(report_rows(&facade)
        .unwrap()
        .contains(&field_text(&a, "mailDeliveryId")));
}

const STATUS_KIND: &str = r#"<moqui-conf><edge><queue-kind name="mailStatus" service="org.moqui.impl.EmailServices.note#EdgeMailStatus"/></edge></moqui-conf>"#;

#[test]
fn a_partial_delivery_reports_the_refused_and_a_message_none_got_bounces() {
    let (ctx, facade, rows) = two_domains(STATUS_KIND);
    let _entered = ctx.enter();
    let sent = |address: &str| (address.to_string(), Answer::Sent("250 ok".to_string()));
    let refused = |address: &str| (address.to_string(), Answer::Refused("550 5.1.1 unknown".to_string()));
    finish(
        ctx.factory(),
        &facade,
        &rows[0],
        0,
        &[sent("ok@a.test"), refused("no@a.test")],
    )
    .unwrap();
    // b.test still waits: the refused go back now, as partial
    assert_eq!(
        statuses(&facade),
        [("partial".to_string(), "no@a.test: 550 5.1.1 unknown".to_string())]
    );
    assert_eq!(field_text(&row(&facade, "a.test"), "reportPending"), "N");
    finish(ctx.factory(), &facade, &rows[1], 0, &[sent("x@b.test")]).unwrap();
    let all = statuses(&facade);
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[1],
        ("partial".to_string(), "no@a.test: 550 5.1.1 unknown".to_string()),
        "the last report"
    );

    // none delivered: bounced
    let (ctx, facade, rows) = two_domains(STATUS_KIND);
    let _entered = ctx.enter();
    finish(
        ctx.factory(),
        &facade,
        &rows[0],
        0,
        &[refused("ok@a.test"), refused("no@a.test")],
    )
    .unwrap();
    finish(ctx.factory(), &facade, &rows[1], 0, &[refused("x@b.test")]).unwrap();
    let all = statuses(&facade);
    let last = all.last().unwrap();
    assert_eq!(last.0, "bounced");
    assert!(last.1.contains("ok@a.test") && last.1.contains("x@b.test"), "{last:?}");
}

#[test]
fn the_servers_own_accounts_refused_are_reported_with_the_rest() {
    let (ctx, facade) = crate::sync_tests::setup_with(STATUS_KIND);
    let _entered = ctx.enter();
    record(
        ctx.factory(),
        &facade,
        "node1",
        "item2",
        "bounce+item2@bounce.test",
        &[],
        &["ann@desk.test".to_string(), "sales@server.test".to_string()],
        &["help@server.test: the server's account 101 is gone".to_string()],
        b"Subject: x\r\n\r\nx\r\n",
    )
    .unwrap();
    let local = row(&facade, LOCAL_DOMAIN);
    assert_eq!(field_text(&local, "statusEnumId"), BOUNCED);
    assert_eq!((number(&local, "sentCount"), number(&local, "bouncedCount")), (2, 1));
    assert_eq!(field_text(&local, "reportPending"), "Y");
    report(ctx.factory(), &facade, &field_text(&local, "mailDeliveryId")).unwrap();
    assert_eq!(
        statuses(&facade),
        [(
            "partial".to_string(),
            "help@server.test: the server's account 101 is gone".to_string()
        )]
    );
}
