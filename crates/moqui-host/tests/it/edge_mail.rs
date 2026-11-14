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

//! Mail through the edge node with Postfix beside it (plans/archive/Edge-Node.md §5.4, E5a). Both ends are real servers in
//! the test's own process, as in `edge_work.rs`, with the edge conf of MoquiDefaultConf.xml (the mail kinds and the
//! replicated `mailRecipients`). The edge node's LMTP listener stands where Postfix hands it mail, a scripted client
//! in Postfix's place; a fake SMTP server stands for Postfix's relay port on the outbound side.
//!
//! - In: RCPT TO is answered from the replicated accounts: a hosted account's address is taken, an unknown one refused
//!   with 550, and so while the link is down too; what is taken while it is down waits in the edge node's queue and
//!   is stored once in the account when the link is back; a message over the 1 MiB payload limit crosses as the
//!   item's content.
//! - Out: a message sent through an `EmailServer` with `smtpHost="edge:"` is queued on the local node, relayed by the
//!   edge node to the fake relay, and a relay's 5xx comes back as a bounce: the mail client's message waits in the
//!   Outbox with the reply as its sendError.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres", feature = "inbound-smtp"))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_core::entity::EntityFacade;
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const NODE: &str = "local-mail";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";

/// Both nodes: quick retries. The kinds and the set are MoquiDefaultConf.xml's.
const LOCAL_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>"#;
const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>
    <smtp-server protocol="lmtp"/>"#;

fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// A test authority's root, and a `localhost` certificate and key (PEM) it signed.
fn certs() -> (CertificateDer<'static>, String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let root = ca.self_signed(&ca_key).unwrap().der().clone();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    (
        root,
        pem("CERTIFICATE", cert.der()),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
}

fn facade(gate: &Gate) -> EntityFacade {
    gate.server.factory().entity().unwrap()
}

/// A service on `gate`'s server as john.doe, with authorization off; it must add no errors.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

/// The rows of `entity` on `gate` with these field values.
fn rows(gate: &Gate, entity: &str, conditions: &[(&str, &str)]) -> Vec<BTreeMap<String, String>> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut find = facade(gate).find(entity).unwrap().use_cache(false);
    for (name, value) in conditions {
        find = find.eq(name, Value::string(*value));
    }
    find.list()
        .unwrap()
        .values()
        .iter()
        .map(|row| {
            row.fields()
                .iter()
                .map(|(name, value)| (name.to_string(), to_plain_string(value)))
                .collect()
        })
        .collect()
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Postfix's relay port, or (E5b) a recipient domain's MX: takes each message (kept with its `MAIL FROM` line first),
/// refusing with 550 any recipient whose address starts with `bounce`, and with 451 the first time one whose address
/// starts with `later`.
struct Relay {
    address: SocketAddr,
    taken: Arc<Mutex<Vec<String>>>,
}

fn relay() -> Relay {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let taken = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&taken);
    let later = Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let keep = Arc::clone(&keep);
            let later = Arc::clone(&later);
            std::thread::spawn(move || relay_session(stream, &keep, &later));
        }
    });
    Relay { address, taken }
}

fn relay_session(stream: TcpStream, taken: &Mutex<Vec<String>>, later: &std::sync::atomic::AtomicBool) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut say = |line: &str| writer.write_all(format!("{line}\r\n").as_bytes());
    if say("220 relay.test ESMTP").is_err() {
        return;
    }
    let mut accepted = 0;
    let mut sender = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let upper = line.trim_end().to_ascii_uppercase();
        let answer = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            "250 relay.test".to_string()
        } else if upper.starts_with("MAIL FROM") {
            accepted = 0;
            sender = line.trim_end().to_string();
            "250 2.1.0 Ok".to_string()
        } else if upper.starts_with("RCPT TO:<BOUNCE") {
            "550 5.1.1 <bounce@example.net>: Recipient address rejected: User unknown".to_string()
        } else if upper.starts_with("RCPT TO:<LATER") && !later.swap(true, std::sync::atomic::Ordering::SeqCst) {
            "451 4.2.0 <later@example.net>: Mailbox busy, try again later".to_string()
        } else if upper.starts_with("RCPT TO") {
            accepted += 1;
            "250 2.1.5 Ok".to_string()
        } else if upper == "DATA" {
            if accepted == 0 {
                "554 5.5.1 No valid recipients".to_string()
            } else {
                if say("354 End data with <CR><LF>.<CR><LF>").is_err() {
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
                "250 2.0.0 Ok: queued as RELAY1".to_string()
            }
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

/// A scripted LMTP client in Postfix's place: one reply is its lines joined with `|`.
struct Lmtp {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Lmtp {
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
}

/// Postfix's side of LMTP: one message to `recipients`, each RCPT's reply and then the replies after the data.
fn lmtp(address: SocketAddr, recipients: &[&str], message: &str) -> (Vec<String>, Vec<String>) {
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut client = Lmtp {
        reader: BufReader::new(stream.try_clone().unwrap()),
        writer: stream,
    };
    assert!(client.reply().starts_with("220 "));
    assert!(client.command("LHLO edge.example.com").starts_with("250"));
    assert_eq!(client.command("MAIL FROM:<dan@example.org>"), "250 2.1.0 Ok");
    let rcpt: Vec<String> = recipients
        .iter()
        .map(|address| client.command(&format!("RCPT TO:<{address}>")))
        .collect();
    let taken = rcpt.iter().filter(|line| line.starts_with("250")).count();
    if taken == 0 {
        client.command("QUIT");
        return (rcpt, Vec::new());
    }
    assert!(client.command("DATA").starts_with("354"));
    let mut stuffed = String::new();
    for line in message.split_inclusive("\r\n") {
        if line.starts_with('.') {
            stuffed.push('.');
        }
        stuffed.push_str(line);
    }
    stuffed.push_str(".\r\n");
    client.writer.write_all(stuffed.as_bytes()).unwrap();
    let after = (0..taken).map(|_| client.reply()).collect();
    client.command("QUIT");
    (rcpt, after)
}

/// A sender on the internet, to the edge node's port 25 (the MTA in moqui): EHLO `helo`, one message from `from` to
/// `recipients`; each RCPT's reply, and the reply after the data (empty when no recipient was taken).
fn smtp(address: SocketAddr, helo: &str, from: &str, recipients: &[&str], message: &str) -> (Vec<String>, String) {
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut client = Lmtp {
        reader: BufReader::new(stream.try_clone().unwrap()),
        writer: stream,
    };
    assert!(client.reply().starts_with("220 "));
    assert!(client.command(&format!("EHLO {helo}")).starts_with("250"));
    assert_eq!(client.command(&format!("MAIL FROM:<{from}>")), "250 2.1.0 Ok");
    let rcpt: Vec<String> = recipients
        .iter()
        .map(|address| client.command(&format!("RCPT TO:<{address}>")))
        .collect();
    if !rcpt.iter().any(|line| line.starts_with("250")) {
        client.command("QUIT");
        return (rcpt, String::new());
    }
    assert!(client.command("DATA").starts_with("354"));
    let mut stuffed = String::new();
    for line in message.split_inclusive("\r\n") {
        if line.starts_with('.') {
            stuffed.push('.');
        }
        stuffed.push_str(line);
    }
    stuffed.push_str(".\r\n");
    client.writer.write_all(stuffed.as_bytes()).unwrap();
    let after = client.reply();
    client.command("QUIT");
    (rcpt, after)
}

/// Whether the edge node has the local node's down connection.
fn down(edge: &Gate) -> bool {
    let out = call(edge, "org.moqui.edge.EdgeServices.get#EdgeClients", &[]);
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter().any(|row| {
        let Value::Map(row) = row else { return false };
        to_plain_string(&row["pairedNodeId"]) == NODE && row["down"] == Value::Bool(true)
    })
}

fn message(message_id: &str, body: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nFrom: Dan <dan@example.org>\r\nTo: sales@example.com\r\nSubject: Through the \
         edge\r\nDate: Mon, 19 Oct 2026 12:00:00 +0000\r\n\r\n{body}\r\n"
    )
}

/// The copies of `message_id` in the local node's account `account`.
fn stored(local: &Gate, account: &str, message_id: &str) -> usize {
    rows(
        local,
        MESSAGE,
        &[("emailAccountId", account), ("messageId", message_id)],
    )
    .len()
}

#[test]
fn mail_crosses_the_edge_node_both_ways_and_waits_while_the_link_is_down() {
    if !crate::common::own_process() {
        return;
    }
    let (root, cert, key) = certs();
    let relay = relay();
    let local = crate::common::start_with_conf_files("edge-mail-local", None, LOCAL_CONF, &[], &[]);
    // the accounts, before the first link sends them whole
    let account = |kind: &str, address: &str| {
        to_plain_string(
            &call(
                &local,
                "org.moqui.mail.EmailClientServices.create#EmailAccount",
                &[
                    ("accountKindEnumId", Value::string(kind)),
                    ("emailAddress", Value::string(address)),
                ],
            )["emailAccountId"],
        )
    };
    let sales = account("EakHosted", "sales@example.com");
    account("EakExternal", "alice@example.com");

    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let relay_port = relay.address.port().to_string();
    let closed_port = crate::common::free_port().to_string();
    let edge = crate::common::start_second_with(
        "edge-mail-edge",
        EDGE_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("smtp_listen_port", "0"),
            ("smtp_allowed_addresses", "127.0.0.1"),
            ("smtp_hostname", "edge.example.com"),
            // not reached at first: a port nothing listens on
            ("edge_mail_relay_port", &closed_port),
            ("edge_mail_bounce_domain", "bounce.test"),
            ("edge_mail_mta", "postfix"),
        ],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    let lmtp_address = edge.server.smtp_address().expect("the edge node's LMTP listener");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let code = to_plain_string(&call(&edge, "org.moqui.edge.EdgeServices.create#PairingCode", &[])["code"]);
    let mut pairing = PairConfig::new(&url, &code, NODE);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let start = || {
        let dialer = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
        wait_until(Duration::from_secs(10), "the link up", || {
            dialer.status().state == LinkState::Linked
        });
        wait_until(Duration::from_secs(5), "the down connection", || down(&edge));
        dialer
    };

    // before the accounts are replicated, an address is not refused for good
    let (rcpt, _) = lmtp(
        lmtp_address,
        &["sales@example.com"],
        &message("<early@example.org>", "Early"),
    );
    assert_eq!(
        rcpt,
        ["451 4.3.0 <sales@example.com>: Temporary lookup failure, try again later"]
    );

    let dialer = start();
    wait_until(Duration::from_secs(10), "the accounts replicated", || {
        let facade = facade(&edge);
        moqui_core::edge::replica::cursors(&facade, Some(NODE))
            .unwrap()
            .iter()
            .any(|cursor| cursor.set_name == "mailRecipients" && cursor.snapshot_date.is_some())
    });

    // a hosted account's address is taken and the mail stored in it; any other is refused
    let (rcpt, after) = lmtp(
        lmtp_address,
        &["Sales+edge@example.com", "nobody@example.com", "alice@example.com"],
        &message("<up@example.org>", "While the link is up"),
    );
    assert_eq!(rcpt[0], "250 2.1.5 Ok");
    assert!(rcpt[1].starts_with("550 5.1.1 <nobody@example.com>"), "{rcpt:?}");
    assert!(
        rcpt[2].starts_with("550 5.1.1 <alice@example.com>"),
        "an external account: {rcpt:?}"
    );
    assert!(
        after[0].starts_with("250 2.1.5 <Sales+edge@example.com> Ok"),
        "{after:?}"
    );
    wait_until(Duration::from_secs(10), "the mail in the account", || {
        stored(&local, &sales, "<up@example.org>") == 1
    });
    // and its item delivered, the answer back, before the link is cut
    let pending = || {
        rows(
            &edge,
            "moqui.edge.EdgeQueueItem",
            &[("kind", "mail"), ("statusEnumId", "EqsPending")],
        )
    };
    wait_until(Duration::from_secs(5), "the item delivered", || pending().is_empty());

    // the link down: an unknown address is still refused, a known one taken and queued
    dialer.stop();
    drop(dialer);
    wait_until(Duration::from_secs(5), "the link down", || !down(&edge));
    let (rcpt, _) = lmtp(lmtp_address, &["nobody@example.com"], &message("<n@example.org>", "x"));
    assert!(rcpt[0].starts_with("550 5.1.1"), "{rcpt:?}");
    let (rcpt, after) = lmtp(
        lmtp_address,
        &["sales@example.com"],
        &message("<down@example.org>", "While the link is down"),
    );
    assert_eq!(rcpt, ["250 2.1.5 Ok"]);
    assert!(after[0].starts_with("250 2.1.5"), "{after:?}");
    // a message over the payload limit: its body crosses as the item's content
    let big_body = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\r\n".repeat(20_000);
    assert!(big_body.len() > 1024 * 1024);
    let (_, after) = lmtp(
        lmtp_address,
        &["sales@example.com"],
        &message("<big@example.org>", &big_body),
    );
    assert!(after[0].starts_with("250 2.1.5"), "{after:?}");
    // the gateway sending one again (its 250 lost) queues nothing more
    let (_, after) = lmtp(
        lmtp_address,
        &["sales@example.com"],
        &message("<down@example.org>", "While the link is down"),
    );
    assert!(after[0].starts_with("250 2.1.5"), "{after:?}");
    assert_eq!(pending().len(), 2, "two messages wait for the local node");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        stored(&local, &sales, "<down@example.org>"),
        0,
        "nothing arrives while the link is down"
    );

    // the link back: each stored once, the large one whole
    let dialer = start();
    wait_until(Duration::from_secs(10), "the mail taken while down", || {
        stored(&local, &sales, "<down@example.org>") == 1 && stored(&local, &sales, "<big@example.org>") == 1
    });
    let big = rows(&local, MESSAGE, &[("messageId", "<big@example.org>")]);
    let size: usize = big[0]
        .get("messageSize")
        .and_then(|size| size.parse().ok())
        .unwrap_or(0);
    assert!(size > 1024 * 1024, "the whole message: {size}");
    // the content each side kept for the deliveries is gone once they were answered
    wait_until(Duration::from_secs(5), "the received content removed", || {
        let facade = facade(&local);
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let node = moqui_core::edge::queue::local_node_id(factory);
        facade
            .read_db_resource(&format!("{}{node}", moqui_core::edge::queue::CONTENT_IN))
            .map(|info| info.children.is_empty())
            .unwrap_or(true)
    });
    assert_eq!(stored(&local, &sales, "<down@example.org>"), 1);

    // out: through an EmailServer with smtpHost edge:, relayed beside the edge node; a 5xx comes back as a bounce
    call(
        &local,
        "create#moqui.basic.email.EmailServer",
        &[
            ("emailServerId", Value::string("EDGE_OUT")),
            ("smtpHost", Value::string("edge:")),
        ],
    );
    let outbound = |id: &str, to: &str| {
        call(
            &local,
            &format!("create#{MESSAGE}"),
            &[
                ("emailMessageId", Value::string(id)),
                ("statusId", Value::string("ES_READY")),
                ("emailAccountId", Value::string(&sales)),
                ("emailServerId", Value::string("EDGE_OUT")),
                ("fromAddress", Value::string("sales@example.com")),
                ("toAddresses", Value::string(to)),
                ("subject", Value::string("Out through the edge")),
                ("bodyText", Value::string("Hello from the local node")),
            ],
        );
        let sent = call(
            &local,
            "org.moqui.mail.EmailServices.send#EmailMessage",
            &[("emailMessageId", Value::string(id))],
        );
        assert!(!to_plain_string(&sent["messageId"]).is_empty(), "{sent:?}");
        assert_eq!(sent.get("sendError"), None, "{sent:?}");
    };
    // the relay not reached: the item waits on the local node, its attempts not counted, and nothing comes back
    outbound("EDGE_OUT_1", "ok@example.net");
    let out_item = |id: &str| {
        rows(&local, "moqui.edge.EdgeQueueItem", &[("kind", "mailOut")])
            .into_iter()
            .find(|row| row.get("payload").is_some_and(|payload| payload.contains(id)))
            .unwrap_or_else(|| panic!("no mailOut item for {id}"))
    };
    wait_until(Duration::from_secs(10), "a try that did not reach the relay", || {
        out_item("EDGE_OUT_1")
            .get("lastErrorText")
            .is_some_and(|error| error.contains("the relay 127.0.0.1"))
    });
    // not counted: an attempt in flight holds the count up by one under its lease (the claim's fencing, given back
    // when the answer is unavailable), so the count is read between attempts, with no lease
    let started = Instant::now();
    let mut between = 0;
    while started.elapsed() < Duration::from_millis(500) {
        let waiting = out_item("EDGE_OUT_1");
        assert_eq!(waiting.get("statusEnumId").map(String::as_str), Some("EqsPending"));
        if waiting.get("leaseOwner").is_none_or(String::is_empty) {
            between += 1;
            assert_eq!(
                waiting.get("attemptCount").map(String::as_str),
                Some("0"),
                "not counted"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(between > 0, "read between attempts");
    assert!(rows(&edge, "moqui.edge.EdgeQueueItem", &[("kind", "mailStatus")]).is_empty());
    // the relay back: it takes it, from the VERP sender of the item
    moqui_lib::env::set_property("edge_mail_relay_port", relay_port.clone());
    let item_1 = out_item("EDGE_OUT_1")["queueItemId"].clone();
    wait_until(Duration::from_secs(10), "the relay took the first", || {
        relay
            .taken
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("Hello from the local node"))
    });
    let taken = relay.taken.lock().unwrap()[0].clone();
    assert!(
        taken.starts_with(&format!("MAIL FROM:<bounce+{item_1}@bounce.test>")),
        "{}",
        taken.lines().next().unwrap_or_default()
    );

    // refused at the relay with 5xx: back in the Outbox
    outbound("EDGE_OUT_2", "bounce@example.net");
    wait_until(Duration::from_secs(10), "the bounce in the Outbox", || {
        let row = &rows(&local, MESSAGE, &[("emailMessageId", "EDGE_OUT_2")])[0];
        row.get("statusId").map(String::as_str) == Some("ES_READY")
            && row
                .get("sendError")
                .is_some_and(|error| error.starts_with("Bounced: ") && error.contains("5.1.1 <bounce@example.net>"))
    });
    let first = &rows(&local, MESSAGE, &[("emailMessageId", "EDGE_OUT_1")])[0];
    assert_eq!(first.get("statusId").map(String::as_str), Some("ES_SENT"));
    assert_eq!(relay.taken.lock().unwrap().len(), 1, "the bounce was not relayed");

    // a remote MX's bounce later, a delivery status notification to the VERP address: the message back in the Outbox;
    // the bounce address of an item the edge node never relayed is refused
    let (rcpt, _) = lmtp(lmtp_address, &["bounce+k0000@bounce.test"], "Subject: x\r\n\r\nx\r\n");
    assert!(rcpt[0].starts_with("550 5.1.1"), "{rcpt:?}");
    let dsn = format!(
        "From: MAILER-DAEMON@mx.example.net\r\nTo: bounce+{item_1}@bounce.test\r\nSubject: Undelivered Mail\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain\r\n\r\nNot delivered.\r\n--b1\r\nContent-Type: message/delivery-status\r\n\r\n\
         Reporting-MTA: dns; mx.example.net\r\n\r\nFinal-Recipient: rfc822; ok@example.net\r\nAction: failed\r\n\
         Status: 5.1.1\r\nDiagnostic-Code: smtp; 550 5.1.1 User unknown\r\n\r\n--b1--\r\n"
    );
    let (rcpt, after) = lmtp(lmtp_address, &[&format!("bounce+{item_1}@bounce.test")], &dsn);
    assert_eq!(rcpt, ["250 2.1.5 Ok"]);
    assert!(after[0].starts_with("250 2.1.5"), "{after:?}");
    wait_until(Duration::from_secs(10), "the later bounce in the Outbox", || {
        let row = &rows(&local, MESSAGE, &[("emailMessageId", "EDGE_OUT_1")])[0];
        row.get("statusId").map(String::as_str) == Some("ES_READY")
            && row
                .get("sendError")
                .is_some_and(|error| error.contains("ok@example.net; 5.1.1"))
    });
    dialer.stop();
}

/// The MTA in moqui (E5b): the edge node takes port 25 itself, in internet mode, with no Postfix.
const EDGE_MTA_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>
    <smtp-server internet="true" starttls="false"/>"#;

/// E5a's cases with the MTA in moqui, no Postfix: mail from the internet to port 25 checked (SPF, DKIM, DMARC) with the
/// edge node's own Authentication-Results, a forged one dropped, a DMARC reject policy refused with 550, and the rest
/// queued for the local node; mail out DKIM-signed and delivered to the recipient domain's MX (a fake one through
/// `mail_mx_override`), a 4xx tried again, a 5xx back in the Outbox. DNS is offline: the records are fixed.
#[test]
fn the_moqui_mta_takes_port_25_and_delivers_to_the_mx() {
    if !crate::common::own_process() {
        return;
    }
    let (root, cert, key) = certs();
    let mx = relay();
    let local =
        crate::common::start_with_conf_files("edge-mta-local", None, LOCAL_CONF, &[("mail_dns_offline", "true")], &[]);
    let sales = to_plain_string(
        &call(
            &local,
            "org.moqui.mail.EmailClientServices.create#EmailAccount",
            &[
                ("accountKindEnumId", Value::string("EakHosted")),
                ("emailAddress", Value::string("sales@example.com")),
            ],
        )["emailAccountId"],
    );
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let mx_override = format!("example.net=127.0.0.1:{}", mx.address.port());
    let edge = crate::common::start_second_with(
        "edge-mta-edge",
        EDGE_MTA_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("smtp_listen_port", "0"),
            ("smtp_hostname", "edge.example.com"),
            ("edge_mail_mta", "moqui"),
            ("edge_mail_bounce_domain", "bounce.test"),
            ("mail_mx_override", &mx_override),
            ("mail_delivery_retry_seconds", "0.2"),
            ("mail_dns_offline", "true"),
        ],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    let port_25 = edge.server.smtp_address().expect("the edge node's SMTP listener");
    // the senders' records: example.org sends from this host, reject.test from nowhere and asks for rejection
    let records = moqui_mail::dns::records();
    records.fix_txt("example.org", "v=spf1 ip4:127.0.0.1 -all").unwrap();
    records.fix_txt("mx.example.org", "v=spf1 ip4:127.0.0.1 -all").unwrap();
    records.fix_txt("reject.test", "v=spf1 -all").unwrap();
    records.fix_txt("_dmarc.reject.test", "v=DMARC1; p=reject").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let code = to_plain_string(&call(&edge, "org.moqui.edge.EdgeServices.create#PairingCode", &[])["code"]);
    let mut pairing = PairConfig::new(&url, &code, NODE);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_until(Duration::from_secs(10), "the link up", || {
        dialer.status().state == LinkState::Linked
    });
    wait_until(Duration::from_secs(10), "the accounts replicated", || {
        let facade = facade(&edge);
        moqui_core::edge::replica::cursors(&facade, Some(NODE))
            .unwrap()
            .iter()
            .any(|cursor| cursor.set_name == "mailRecipients" && cursor.snapshot_date.is_some())
    });

    // in, from the internet: checked, the forged result dropped, stored once in the account
    let forged = format!(
        "Authentication-Results: edge.example.com; dkim=pass header.d=example.org\r\n{}",
        message("<in@example.org>", "From the internet")
    );
    let (rcpt, after) = smtp(
        port_25,
        "mx.example.org",
        "dan@example.org",
        &["sales@example.com", "nobody@example.com"],
        &forged,
    );
    assert_eq!(rcpt[0], "250 2.1.5 Ok");
    assert!(rcpt[1].starts_with("550 5.1.1 <nobody@example.com>"), "{rcpt:?}");
    assert!(after.starts_with("250 2.0.0"), "{after}");
    wait_until(Duration::from_secs(10), "the mail in the account", || {
        stored(&local, &sales, "<in@example.org>") == 1
    });
    let headers = rows(&local, MESSAGE, &[("messageId", "<in@example.org>")])[0]
        .get("headersString")
        .cloned()
        .unwrap_or_default();
    assert!(
        headers.contains("Authentication-Results: edge.example.com;"),
        "{headers}"
    );
    assert!(headers.contains("spf=pass"), "{headers}");
    assert!(headers.contains("dmarc=none"), "{headers}");
    assert!(!headers.contains("dkim=pass"), "the forged result is gone: {headers}");
    // a DMARC reject policy that fails: refused after the data, nothing queued
    let (rcpt, after) = smtp(
        port_25,
        "mx.example.org",
        "x@reject.test",
        &["sales@example.com"],
        &message("<dmarc@reject.test>", "Spoofed").replace("dan@example.org", "x@reject.test"),
    );
    assert_eq!(rcpt, ["250 2.1.5 Ok"]);
    assert!(
        after.starts_with("550 5.7.1 Rejected by the DMARC policy of reject.test"),
        "{after}"
    );

    // out: signed here and delivered to the MX; a 4xx tried again, a 5xx back in the Outbox
    call(
        &local,
        "create#moqui.basic.email.EmailServer",
        &[
            ("emailServerId", Value::string("EDGE_OUT")),
            ("smtpHost", Value::string("edge:")),
        ],
    );
    let outbound = |id: &str, to: &str| {
        call(
            &local,
            &format!("create#{MESSAGE}"),
            &[
                ("emailMessageId", Value::string(id)),
                ("statusId", Value::string("ES_READY")),
                ("emailAccountId", Value::string(&sales)),
                ("emailServerId", Value::string("EDGE_OUT")),
                ("fromAddress", Value::string("sales@example.com")),
                ("toAddresses", Value::string(to)),
                ("subject", Value::string("Out through the MTA in moqui")),
                ("bodyText", Value::string("Hello from the local node")),
            ],
        );
        let sent = call(
            &local,
            "org.moqui.mail.EmailServices.send#EmailMessage",
            &[("emailMessageId", Value::string(id))],
        );
        assert_eq!(sent.get("sendError"), None, "{sent:?}");
    };
    outbound("MTA_OUT_1", "ok@example.net, later@example.net");
    wait_until(
        Duration::from_secs(10),
        "both recipients delivered, one after a 4xx",
        || mx.taken.lock().unwrap().len() == 2,
    );
    let taken = mx.taken.lock().unwrap().clone();
    let item = rows(&edge, "moqui.edge.EdgeMailDelivery", &[("domain", "example.net")])[0]["queueItemId"].clone();
    assert!(
        taken[0].starts_with(&format!("MAIL FROM:<bounce+{item}@bounce.test>")),
        "{}",
        taken[0].lines().next().unwrap_or_default()
    );
    // what the MX got is signed for example.com with the edge node's key, and verifies against its published record
    let signed = taken[0]
        .split_once("\r\n")
        .map(|(_, message)| message)
        .unwrap_or_default()
        .to_string();
    assert!(signed.starts_with("DKIM-Signature: v=1; a=rsa-sha256;"), "{signed}");
    let record = to_plain_string(&call(&edge, "org.moqui.mail.EmailServices.get#DkimRecord", &[])["record"]);
    assert!(record.starts_with("v=DKIM1; k=rsa; p="), "{record}");
    let fixed = moqui_mail::dns::Records::offline();
    fixed.fix_txt("moqui._domainkey.example.com", &record).unwrap();
    let verdict = moqui_mail::dns::run(moqui_mail::auth::verify(
        signed.into_bytes(),
        moqui_mail::auth::Session {
            client_ip: Some("192.0.2.25".parse().unwrap()),
            helo: "edge.example.com".to_string(),
            mail_from: format!("bounce+{item}@bounce.test"),
            hostname: "mx.example.net".to_string(),
        },
        "mx.example.net".to_string(),
        Some(Arc::new(fixed)),
    ))
    .unwrap();
    assert!(verdict.summary.contains("dkim=pass"), "{verdict:?}");
    wait_until(Duration::from_secs(10), "the delivery done", || {
        rows(&edge, "moqui.edge.EdgeMailDelivery", &[("queueItemId", &item)])[0]
            .get("statusEnumId")
            .map(String::as_str)
            == Some("EmdSent")
    });
    let row = &rows(&edge, "moqui.edge.EdgeMailDelivery", &[("queueItemId", &item)])[0];
    assert_eq!(row.get("attemptCount").map(String::as_str), Some("2"), "{row:?}");

    outbound("MTA_OUT_2", "bounce@example.net");
    wait_until(Duration::from_secs(10), "the bounce in the Outbox", || {
        let row = &rows(&local, MESSAGE, &[("emailMessageId", "MTA_OUT_2")])[0];
        row.get("statusId").map(String::as_str) == Some("ES_READY")
            && row
                .get("sendError")
                .is_some_and(|error| error.starts_with("Bounced: bounce@example.net: ") && error.contains("550"))
    });
    let first = &rows(&local, MESSAGE, &[("emailMessageId", "MTA_OUT_1")])[0];
    assert_eq!(first.get("statusId").map(String::as_str), Some("ES_SENT"));
    assert_eq!(mx.taken.lock().unwrap().len(), 2, "the bounce took no data");

    // some delivered and some refused: it stays Sent, with the refused named, so a resend reaches nobody twice
    outbound("MTA_OUT_3", "ok3@example.net, bounce@example.net");
    wait_until(Duration::from_secs(10), "the partial delivery noted", || {
        let row = &rows(&local, MESSAGE, &[("emailMessageId", "MTA_OUT_3")])[0];
        row.get("statusId").map(String::as_str) == Some("ES_SENT")
            && row
                .get("sendError")
                .is_some_and(|error| error.starts_with("Not delivered to: bounce@example.net: "))
    });
    assert_eq!(mx.taken.lock().unwrap().len(), 3);

    // internal mail through the edge node: queued back to the local node, not round through port 25, signed and
    // verified with the edge node's key
    outbound("MTA_OUT_4", "sales@example.com");
    let message_id = rows(&local, MESSAGE, &[("emailMessageId", "MTA_OUT_4")])[0]["messageId"].clone();
    wait_until(Duration::from_secs(10), "the internal message in the account", || {
        rows(
            &local,
            MESSAGE,
            &[("emailAccountId", &sales), ("messageId", &message_id)],
        )
        .iter()
        .any(|row| row.get("statusId").map(String::as_str) == Some("ES_RECEIVED"))
    });
    let headers = rows(
        &local,
        MESSAGE,
        &[("emailAccountId", &sales), ("messageId", &message_id)],
    )
    .into_iter()
    .find(|row| row.get("statusId").map(String::as_str) == Some("ES_RECEIVED"))
    .and_then(|row| row.get("headersString").cloned())
    .unwrap_or_default();
    assert!(
        headers.contains("Authentication-Results: edge.example.com;"),
        "{headers}"
    );
    assert!(
        headers.contains("dkim=pass header.d=example.com header.s=moqui"),
        "{headers}"
    );
    assert_eq!(mx.taken.lock().unwrap().len(), 3, "nothing to the MX");
    dialer.stop();
}
