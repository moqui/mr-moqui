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

//! Several local nodes on one edge node (plans/Edge-Node2.md §4.1, E16). Two local nodes on separate databases pair with
//! one dedicated edge node, all three real servers in the test's own process; each local node's link is a dialer with
//! an explicit `LocalConfig` serving its own app, as in `edge_mail.rs`. Each node has a host name, a mail domain, a chat
//! bridge (the same id on both, as two databases' sequences give) and a PASS site, and a test component whose services
//! answer with the node's name. Each one's work reaches only it: a request at its host, mail to its addresses, a webhook
//! at its host, a PASS call for its site, a delegated call in its owner scope. A name both claim is refused, naming
//! only that another node has it; an address both hold stays with the first, the later copy kept as a conflict.
//!
//! The acceptance scenario (§5 E16, all areas at once with one node away and one overloaded) is
//! [`one_node_away_or_overloaded_leaves_the_other_working`]; SIP and XMPP are left to their own parts of E16.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "embedded-postgres",
    feature = "inbound-smtp",
    feature = "channels"
))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine as _;
use moqui_core::edge::owner::{self, Owner};
use moqui_core::edge::setup::EdgeSetup;
use moqui_core::edge::{EdgeHostInfo, LinkState, HOST_TERMINATE};
use moqui_core::entity::EntityFacade;
use moqui_core::pass_authority::Call;
use moqui_host::edge_link::{local_web, Backoff, Dialer, LocalConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use serde_json::json;
use tokio::sync::watch;

use super::edge_web::{authority, down, issue, pair_node, wait_until};
use crate::common::Gate;

const A: &str = "dom-a";
const B: &str = "dom-b";
const HOST_A: &str = "www.a-shop.example";
const HOST_B: &str = "www.b-shop.example";
/// The bridge id both local nodes have.
const BRIDGE: &str = "BR1";
const WHO: &str = "edgetest.DomainServices.get#Who";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";

/// The local nodes and the edge node: quick retries; the test service delegated.
const CONF: &str = r#"<edge queue-retry-seconds="0.1">
        <delegate service="edgetest.DomainServices.get#Who"/>
    </edge>
    <email-client idle-enabled="false"/>"#;
const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1">
        <delegate service="edgetest.DomainServices.get#Who"/>
    </edge>
    <email-client idle-enabled="false"/>
    <smtp-server protocol="lmtp"/>"#;

/// The test component of a node named `name`: its name by a service, by REST and as a PASS authority.
fn component(name: &str) -> Vec<(String, String)> {
    let services = format!(
        r#"<services>
    <service verb="get" noun="Who" authenticate="false" allow-remote="true">
        <out-parameters><parameter name="node"/></out-parameters>
        <actions><set field="node" value="{name}"/></actions>
    </service>
    <service verb="handle" noun="Call" authenticate="anonymous-all" transaction="ignore">
        <in-parameters>
            <parameter name="operation" required="true"/>
            <parameter name="siteId"/>
            <parameter name="input" type="Map"/>
            <parameter name="delegation"/>
            <parameter name="clientId"/>
            <parameter name="clientVerified" type="Boolean"/>
            <parameter name="visitorAddress"/>
            <parameter name="operationId"/>
        </in-parameters>
        <out-parameters><parameter name="result" type="Map"/></out-parameters>
        <actions><script>result = {{node: '{name}', siteId}}</script></actions>
    </service>
</services>"#
    );
    vec![
        (
            "components/addon/edgetest/component.xml".to_string(),
            r#"<component name="edgetest"/>"#.to_string(),
        ),
        (
            "components/addon/edgetest/service/edgetest/DomainServices.xml".to_string(),
            services,
        ),
        (
            "components/addon/edgetest/service/edgetest.rest.xml".to_string(),
            r#"<resource name="edgetest" require-authentication="false"><resource name="who"><method type="get">
                <service name="edgetest.DomainServices.get#Who"/></method></resource></resource>"#
                .to_string(),
        ),
    ]
}

fn facade(gate: &Gate) -> EntityFacade {
    gate.server.factory().entity().unwrap()
}

/// `body` in a context of `gate`'s server, logged in as john.doe, with authorization off.
fn in_context<T>(gate: &Gate, body: impl FnOnce() -> T) -> T {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    body()
}

/// A service on `gate`'s server: its result and the errors it added.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, Vec<String>) {
    in_context(gate, || {
        let parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let called = moqui_core::service::sync(name, &parameters);
        let mut errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec())
            .unwrap_or_default();
        let out = called.unwrap_or_else(|err| {
            errors.push(err.to_string());
            BTreeMap::new()
        });
        (out, errors)
    })
}

/// The rows of `entity` on `gate` with these field values.
fn rows(gate: &Gate, entity: &str, conditions: &[(&str, &str)]) -> Vec<BTreeMap<String, String>> {
    in_context(gate, || {
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
    })
}

fn create(gate: &Gate, entity: &str, fields: &[(&str, &str)]) {
    in_context(gate, || {
        let facade = facade(gate);
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in fields {
            row.set(*name, Value::string(*value));
        }
        row.create(&facade).unwrap();
    });
}

/// A hosted account on `gate` at `address`: its id.
fn account(gate: &Gate, address: &str) -> String {
    let (out, errors) = call(
        gate,
        "org.moqui.mail.EmailClientServices.create#EmailAccount",
        &[
            ("accountKindEnumId", Value::string("EakHosted")),
            ("emailAddress", Value::string(address)),
        ],
    );
    assert!(errors.is_empty(), "{address}: {errors:?}");
    to_plain_string(&out["emailAccountId"])
}

/// The copies of `message_id` in account `account` on `gate`.
fn stored(gate: &Gate, account: &str, message_id: &str) -> usize {
    rows(gate, MESSAGE, &[("emailAccountId", account), ("messageId", message_id)]).len()
}

fn message(message_id: &str, to: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nFrom: Dan <dan@example.org>\r\nTo: {to}\r\nSubject: Two pairings\r\nDate: Mon, 19 \
         Oct 2026 12:00:00 +0000\r\n\r\nHello\r\n"
    )
}

/// One LMTP connection, as Postfix holds it.
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
    assert!(client.command("MAIL FROM:<dan@example.org>").starts_with("250"));
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
    let mut data = String::new();
    for line in message.split_inclusive("\r\n") {
        if line.starts_with('.') {
            data.push('.');
        }
        data.push_str(line);
    }
    data.push_str(".\r\n");
    client.writer.write_all(data.as_bytes()).unwrap();
    let after = (0..taken).map(|_| client.reply()).collect();
    client.command("QUIT");
    (rcpt, after)
}

/// A Telnyx webhook of event `event`, signed now with `key`, posted to `host` on the edge node's port: its status.
fn webhook(
    client: &reqwest::blocking::Client,
    host: &str,
    port: u16,
    key: &Ed25519KeyPair,
    event: &str,
) -> (u16, String) {
    let body = json!({"data": {"event_type": "message.received", "id": event, "record_type": "event",
        "occurred_at": "2026-11-13T12:00:00.000+00:00",
        "payload": {"id": format!("in-{event}"), "direction": "inbound", "type": "SMS",
            "from": {"phone_number": "+15557778888"}, "to": [{"phone_number": "+15550001000"}],
            "text": "hello", "received_at": "2026-11-13T12:00:00.000+00:00"}}})
    .to_string();
    let stamp = jiff::Timestamp::now().as_second().to_string();
    let mut signed = stamp.as_bytes().to_vec();
    signed.push(b'|');
    signed.extend_from_slice(body.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&signed).as_ref());
    let response = client
        .post(format!("https://{host}:{port}/channels/{BRIDGE}/webhook"))
        .header("content-type", "application/json")
        .header("telnyx-signature-ed25519", signature)
        .header("telnyx-timestamp", stamp)
        .body(body)
        .send()
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().unwrap_or_default())
}

/// The node the edge node queued webhook `event` for, once it has.
fn webhook_node(edge: &Gate, event: &str) -> Option<String> {
    rows(edge, "moqui.edge.EdgeQueueItem", &[("kind", "chatWebhook")])
        .into_iter()
        .find(|row| row.get("idempotencyKey").is_some_and(|key| key.ends_with(event)))
        .map(|row| row["pairedNodeId"].clone())
}

/// A PASS authority call for `site` from the edge node (`edge:`): the node that answered, or the error.
fn pass_node(edge: &Gate, site: &str) -> Result<String, String> {
    in_context(edge, || {
        let sender = moqui_core::pass_authority::sender().map_err(|err| err.message().to_string())?;
        let call = Call {
            operation: "who".to_string(),
            site_id: site.to_string(),
            input: json!({}),
            ..Call::default()
        };
        let reply = sender.call("edge:", call).map_err(|err| err.message().to_string())?;
        let result = reply.result.map_err(|err| format!("{}: {}", err.code, err.message))?;
        Ok(result["node"].as_str().unwrap_or_default().to_string())
    })
}

/// The delegated `get#Who` on the edge node, in `scope` when given: the node that answered, or the errors.
fn delegated(edge: &Gate, scope: Option<&str>) -> Result<String, String> {
    let run = || call(edge, WHO, &[]);
    let (out, errors) = match scope {
        Some(node) => owner::with(Owner::paired(node), run),
        None => run(),
    };
    if errors.is_empty() {
        Ok(to_plain_string(out.get("node").unwrap_or(&Value::Null)))
    } else {
        Err(errors.join("; "))
    }
}

/// The two local nodes, the edge node, their links and what each has.
/// Its fields drop in order: the links, then the edge node and node B, then node A, whose gate holds the test lock.
struct Domains {
    dialers: BTreeMap<&'static str, Dialer>,
    _stop: watch::Sender<bool>,
    shutdown: watch::Receiver<bool>,
    configs: BTreeMap<&'static str, LocalConfig>,
    edge: Gate,
    local_b: Gate,
    local_a: Gate,
    rt: tokio::runtime::Runtime,
    root: CertificateDer<'static>,
    edge_port: u16,
    /// Each node's account ids: its own address's, and the address both have.
    own: BTreeMap<&'static str, String>,
    shared: BTreeMap<&'static str, String>,
    keys: BTreeMap<&'static str, Ed25519KeyPair>,
    lmtp: SocketAddr,
}

impl Domains {
    /// Start the three servers, pair both nodes and link them one after the other (so node A's copies come first),
    /// with each node's accounts and bridge in place before its link sends them whole.
    fn start(label: &str) -> Self {
        let ca = authority();
        let (edge_cert, edge_key) = issue(&ca, &["localhost", HOST_A, HOST_B]);
        let owned = |name: &str| component(name);
        let files_a = owned("A");
        let files_a: Vec<(&str, &str)> = files_a.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        let pass_properties = [
            ("pass_authority_service", "edgetest.DomainServices.handle#Call"),
            ("channels_webhooks_enabled", "true"),
        ];
        let local_a =
            crate::common::start_with_conf_files(&format!("{label}-a"), None, CONF, &pass_properties, &files_a);
        let files_b = owned("B");
        let files_b: Vec<(&str, &str)> = files_b.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        let local_b = crate::common::start_second_with(&format!("{label}-b"), CONF, &[], &files_b);
        let mut keys = BTreeMap::new();
        let mut own = BTreeMap::new();
        let mut shared = BTreeMap::new();
        for (node, gate, seed, address) in [(A, &local_a, 7_u8, "ann@a.test"), (B, &local_b, 8, "bob@b.test")] {
            own.insert(node, account(gate, address));
            shared.insert(node, account(gate, "info@both.test"));
            let key = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
            let public = base64::engine::general_purpose::STANDARD.encode(key.public_key().as_ref());
            create(
                gate,
                "moqui.chat.ChatBridge",
                &[
                    ("chatBridgeId", BRIDGE),
                    ("name", "Telnyx"),
                    ("typeEnumId", "CbtSms"),
                    ("adapterName", "sms"),
                    ("webhookPublicKey", &public),
                    ("configJson", &json!({"apiKey": "KEY", "publicKey": public}).to_string()),
                ],
            );
            keys.insert(node, key);
        }

        let edge_port = crate::common::free_port();
        let edge_port_text = edge_port.to_string();
        let files_edge = owned("EDGE");
        let mut files_edge: Vec<(&str, &str)> = files_edge.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        files_edge.push(("cert/default.crt", &edge_cert));
        files_edge.push(("cert/default.key", &edge_key));
        let edge = crate::common::start_second_with(
            &format!("{label}-edge"),
            EDGE_CONF,
            &[
                ("node_role", "edge"),
                ("webapp_https_listen_port", &edge_port_text),
                ("smtp_listen_port", "0"),
                ("smtp_allowed_addresses", "127.0.0.1"),
                ("smtp_hostname", "edge.example.com"),
                ("edge_mail_mta", "postfix"),
            ],
            &files_edge,
        );
        let lmtp = edge.server.smtp_address().expect("the edge node's LMTP listener");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let url = format!("https://localhost:{edge_port}");
        let (stop, shutdown) = watch::channel(false);
        let mut configs = BTreeMap::new();
        for (node, gate) in [(A, &local_a), (B, &local_b)] {
            let paired = pair_node(&rt, &edge, &url, node, &ca.root);
            let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, node);
            config.backoff = Backoff {
                first: Duration::from_millis(100),
                max: Duration::from_millis(500),
            };
            config.connect_timeout = Duration::from_secs(3);
            config.web = local_web(gate.server.factory());
            configs.insert(node, config);
        }
        let mut domains = Self {
            local_a,
            local_b,
            edge,
            rt,
            root: ca.root,
            edge_port,
            configs,
            shutdown,
            _stop: stop,
            dialers: BTreeMap::new(),
            own,
            shared,
            keys,
            lmtp,
        };
        for node in [A, B] {
            domains.link(node);
            domains.wait_sent_whole(node);
        }
        domains
    }

    /// Start `node`'s link and wait for its down connection.
    fn link(&mut self, node: &'static str) {
        let dialer = Dialer::start(
            self.configs[node].clone(),
            self.rt.handle(),
            self.shutdown.clone(),
            None,
        )
        .unwrap();
        wait_until(Duration::from_secs(10), "the link up", || {
            dialer.status().state == LinkState::Linked
        });
        wait_until(Duration::from_secs(5), "the down connection", || down(&self.edge, node));
        self.dialers.insert(node, dialer);
    }

    /// Stop `node`'s link.
    fn unlink(&mut self, node: &str) {
        if let Some(dialer) = self.dialers.remove(node) {
            dialer.stop();
        }
        wait_until(Duration::from_secs(5), "the link down", || !down(&self.edge, node));
    }

    /// Wait until `node`'s mail recipients and chat bridges are on the edge node whole.
    fn wait_sent_whole(&self, node: &str) {
        wait_until(Duration::from_secs(10), "the sets sent whole", || {
            let cursors = in_context(&self.edge, || {
                moqui_core::edge::replica::cursors(&facade(&self.edge), Some(node)).unwrap()
            });
            ["mailRecipients", "chatBridges"].iter().all(|set| {
                cursors
                    .iter()
                    .any(|cursor| cursor.set_name == *set && cursor.snapshot_date.is_some())
            })
        });
    }

    /// `node`'s Configure: its host names (terminate), PASS sites and the bridge; each refused name with its error.
    fn configure(&self, node: &str, hosts: &[&str], sites: &[&str], domain: &str) -> BTreeMap<String, String> {
        let setup = EdgeSetup {
            hosts: hosts
                .iter()
                .map(|name| EdgeHostInfo {
                    host_name: (*name).to_string(),
                    paired_node_id: node.to_string(),
                    mode_enum_id: HOST_TERMINATE.to_string(),
                })
                .collect(),
            set_hosts: true,
            mail_domains: vec![domain.to_string(), "both.test".to_string()],
            chat_bridge_ids: vec![BRIDGE.to_string()],
            pass_sites: sites.iter().map(|site| (*site).to_string()).collect(),
            xmpp_domain: String::new(),
        };
        let configured = self.rt.block_on(self.dialers[node].configure(&setup)).unwrap();
        configured
            .hosts
            .into_iter()
            .filter(|host| !host.error.is_empty())
            .map(|host| (host.host_name, host.error))
            .collect()
    }

    /// A client of the edge node's HTTPS port that sends both pairings' host names there.
    fn client(&self) -> reqwest::blocking::Client {
        let at = SocketAddr::from(([127, 0, 0, 1], self.edge_port));
        reqwest::blocking::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_der(self.root.as_ref()).unwrap()])
            .resolve(HOST_A, at)
            .resolve(HOST_B, at)
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap()
    }

    /// The node a request at `host` reached, by the test component's REST service.
    fn web_node(&self, client: &reqwest::blocking::Client, host: &str) -> (u16, String) {
        let response = client
            .get(format!("https://{host}:{}/rest/s1/edgetest/who", self.edge_port))
            .send()
            .unwrap();
        let status = response.status().as_u16();
        let body: serde_json::Value = response.json().unwrap_or_default();
        (status, body["node"].as_str().unwrap_or_default().to_string())
    }

    /// Mail to `address` through the edge node's LMTP listener: the RCPT reply.
    fn mail(&self, address: &str, message_id: &str) -> String {
        let (rcpt, after) = lmtp(self.lmtp, &[address], &message(message_id, address));
        if rcpt[0].starts_with("250") {
            assert!(after[0].starts_with("250"), "{address}: {after:?}");
        }
        rcpt[0].clone()
    }
}

/// E16's rules on a served edge node: each pairing's work reaches only it, by host name, mail address, bridge id within
/// the host name, PASS site and owner scope; a name, site or address both claim stays with the first.
#[test]
fn two_pairings_each_get_only_their_own_work() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let mut domains = Domains::start("edge-dom");
    let edge = &domains.edge;

    // Configure: a name or PASS site the other named first is refused, naming neither node
    assert_eq!(domains.configure(A, &[HOST_A], &["SITE_A"], "a.test"), BTreeMap::new());
    let refused = domains.configure(B, &[HOST_B, HOST_A], &["SITE_A", "SITE_B"], "b.test");
    assert_eq!(
        refused,
        BTreeMap::from([
            (HOST_A.to_string(), "another node has this name".to_string()),
            ("SITE_A".to_string(), "another node has this name".to_string()),
        ])
    );
    let kept = rows(edge, "moqui.edge.EdgeClient", &[("pairedNodeId", B)]).remove(0);
    assert!(
        kept["setupJson"].contains("SITE_B") && !kept["setupJson"].contains("SITE_A"),
        "{kept:?}"
    );

    // the address both have stays with the first, the later copy kept as a conflict Edge Clients shows
    let (out, errors) = call(edge, "org.moqui.edge.EdgeServices.get#ReplicaConflicts", &[]);
    assert!(errors.is_empty(), "{errors:?}");
    let conflicts = format!("{:?}", out.get("conflictList"));
    assert!(
        conflicts.contains(B) && conflicts.contains("info@both.test") && conflicts.contains(A),
        "{conflicts}"
    );

    // web: each host name reaches its own node
    let client = domains.client();
    assert_eq!(domains.web_node(&client, HOST_A), (200, "A".to_string()));
    assert_eq!(domains.web_node(&client, HOST_B), (200, "B".to_string()));

    // mail in: each address to its own node; the one both have to the first
    assert!(domains.mail("ann@a.test", "<m-a@example.org>").starts_with("250"));
    assert!(domains.mail("bob@b.test", "<m-b@example.org>").starts_with("250"));
    assert!(domains
        .mail("info@both.test", "<m-both@example.org>")
        .starts_with("250"));
    wait_until(Duration::from_secs(10), "the mail stored on each side", || {
        stored(&domains.local_a, &domains.own[A], "<m-a@example.org>") == 1
            && stored(&domains.local_b, &domains.own[B], "<m-b@example.org>") == 1
            && stored(&domains.local_a, &domains.shared[A], "<m-both@example.org>") == 1
    });
    assert_eq!(stored(&domains.local_b, &domains.own[B], "<m-a@example.org>"), 0);
    assert_eq!(stored(&domains.local_a, &domains.own[A], "<m-b@example.org>"), 0);
    assert_eq!(stored(&domains.local_b, &domains.shared[B], "<m-both@example.org>"), 0);

    // chat: the same bridge id on both goes by the host name; at the edge node's own name it is refused
    let (status, body) = webhook(&client, HOST_A, domains.edge_port, &domains.keys[A], "ev-a");
    assert_eq!((status, body.contains("queued")), (200, true), "{body}");
    let (status, body) = webhook(&client, HOST_B, domains.edge_port, &domains.keys[B], "ev-b");
    assert_eq!((status, body.contains("queued")), (200, true), "{body}");
    assert_eq!(webhook_node(edge, "ev-a").as_deref(), Some(A));
    assert_eq!(webhook_node(edge, "ev-b").as_deref(), Some(B));
    let (status, _) = webhook(&client, HOST_A, domains.edge_port, &domains.keys[B], "ev-x");
    assert_eq!(status, 401, "B's network signature at A's name");
    let (status, _) = webhook(&client, "localhost", domains.edge_port, &domains.keys[A], "ev-y");
    assert_eq!(status, 401, "ambiguous at the edge node's own name");

    // PASS: each site to the authority that named it; a site neither names is refused while both are linked
    assert_eq!(pass_node(edge, "SITE_A").as_deref(), Ok("A"));
    assert_eq!(pass_node(edge, "SITE_B").as_deref(), Ok("B"));
    let unknown = pass_node(edge, "SITE_C").unwrap_err();
    assert!(unknown.contains("no paired node names PASS site SITE_C"), "{unknown}");

    // delegate: by the owner scope; with none, refused while both are linked
    assert_eq!(delegated(edge, Some(A)).as_deref(), Ok("A"));
    assert_eq!(delegated(edge, Some(B)).as_deref(), Ok("B"));
    let unscoped = delegated(edge, None).unwrap_err();
    assert!(unscoped.contains("needs its owner"), "{unscoped}");

    // B away: a call with no owner still has two pairings to choose from, so it is refused, never sent to A, and
    // B's own site is offline
    domains.unlink(B);
    let edge = &domains.edge;
    for site in ["SITE_C", ""] {
        let refused = pass_node(edge, site).unwrap_err();
        assert!(
            refused.contains("no paired node names PASS site"),
            "{site:?}: {refused}"
        );
    }
    let offline = pass_node(edge, "SITE_B").unwrap_err();
    assert!(offline.contains("local node offline"), "{offline}");
    let unscoped = delegated(edge, None).unwrap_err();
    assert!(unscoped.contains("needs its owner"), "{unscoped}");
    assert!(delegated(edge, Some(B)).unwrap_err().contains("local node offline"));

    println!("two_pairings_each_get_only_their_own_work: {:?}", started.elapsed());
}

/// The §5 E16 acceptance scenario, the parts of this side of E16 (web, mail, chat, PASS, delegate; SIP and XMPP are
/// their own parts'): both pairings at once, then one local node away and then overloaded (its delivery paused, a
/// backlog queued for it), while the other keeps working; the one away gets its queued work once when it is back.
#[test]
#[ignore = "acceptance: plans/Edge-Node2.md §5 E16, one local node away or overloaded leaves the other working"]
fn one_node_away_or_overloaded_leaves_the_other_working() {
    if !crate::common::own_process() {
        return;
    }
    let mut domains = Domains::start("edge-dom-acc");
    assert_eq!(domains.configure(A, &[HOST_A], &["SITE_A"], "a.test"), BTreeMap::new());
    assert_eq!(domains.configure(B, &[HOST_B], &["SITE_B"], "b.test"), BTreeMap::new());
    let client = domains.client();

    // B away: its host shows the offline page, its mail and webhooks are taken and wait, its site is offline; A works
    domains.unlink(B);
    assert_eq!(domains.web_node(&client, HOST_B).0, 503, "B's offline page");
    assert_eq!(domains.web_node(&client, HOST_A), (200, "A".to_string()));
    assert!(domains.mail("bob@b.test", "<away-b@example.org>").starts_with("250"));
    assert!(domains.mail("ann@a.test", "<away-a@example.org>").starts_with("250"));
    let (status, _) = webhook(&client, HOST_B, domains.edge_port, &domains.keys[B], "away-b");
    assert_eq!(status, 200, "taken for B while it is away");
    let (status, _) = webhook(&client, HOST_A, domains.edge_port, &domains.keys[A], "away-a");
    assert_eq!(status, 200);
    let offline = pass_node(&domains.edge, "SITE_B").unwrap_err();
    assert!(offline.contains("local node offline"), "{offline}");
    assert_eq!(pass_node(&domains.edge, "SITE_A").as_deref(), Ok("A"));
    assert!(delegated(&domains.edge, Some(B))
        .unwrap_err()
        .contains("local node offline"));
    assert_eq!(delegated(&domains.edge, Some(A)).as_deref(), Ok("A"));
    wait_until(Duration::from_secs(10), "A's mail while B is away", || {
        stored(&domains.local_a, &domains.own[A], "<away-a@example.org>") == 1
    });

    // B back: what waited arrives once
    domains.link(B);
    wait_until(Duration::from_secs(15), "B's mail after it is back", || {
        stored(&domains.local_b, &domains.own[B], "<away-b@example.org>") == 1
    });

    // B overloaded: its delivery paused (every item answered "paused") and a backlog of mail for it; A's mail and
    // calls go on as before
    in_context(&domains.local_b, || {
        moqui_core::edge::queue::set_paused(&facade(&domains.local_b), true)
    });
    for n in 0..40 {
        assert!(domains
            .mail("bob@b.test", &format!("<busy-{n}@example.org>"))
            .starts_with("250"));
    }
    let started = Instant::now();
    assert!(domains.mail("ann@a.test", "<busy-a@example.org>").starts_with("250"));
    wait_until(Duration::from_secs(10), "A's mail while B is overloaded", || {
        stored(&domains.local_a, &domains.own[A], "<busy-a@example.org>") == 1
    });
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(domains.web_node(&client, HOST_A), (200, "A".to_string()));
    assert_eq!(pass_node(&domains.edge, "SITE_A").as_deref(), Ok("A"));
    in_context(&domains.local_b, || {
        moqui_core::edge::queue::set_paused(&facade(&domains.local_b), false)
    });
    wait_until(Duration::from_secs(20), "B's backlog once it resumes", || {
        stored(&domains.local_b, &domains.own[B], "<busy-39@example.org>") == 1
    });
    assert_eq!(
        stored(&domains.local_b, &domains.own[B], "<away-b@example.org>"),
        1,
        "once"
    );
    let webhooks = rows(&domains.edge, "moqui.edge.EdgeQueueItem", &[("kind", "chatWebhook")]);
    assert_eq!(
        webhooks.iter().filter(|row| row["pairedNodeId"] == B).count(),
        1,
        "B's webhook queued once, for B: {webhooks:?}"
    );
    for node in [A, B] {
        domains.unlink(node);
    }
}
