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

//! The all-in-one node (plans/archive/Edge-Node.md §10, E12): a full server with its own users, mail accounts, chat bridges,
//! calls, numbers and XMPP domain that is also the edge node of a local node with no listening port. The server runs
//! in a child process of this test binary (the test `edge_allinone::server_process`), on a database of its own, as
//! `edge_call.rs`'s edge node does: the media driver, the notification socket's bindings, the SIP engine, the XMPP
//! server and the fixed DNS records are process globals, and `node_role` is a process property. The child answers
//! commands on its stdin (a service call as a user, a find, a direct write that skips the services' checks), so the
//! test reads and changes the server's own data as it does the local node's. The local node runs in the test's
//! process, pairs with the server, and its link is a dialer the test drives.
//!
//! - Mail: the server's own hosted account and the local node's replicated one; one message to both is stored once
//!   on each side (the paired node's part queued with only its recipient), a resend stores nothing new; an address
//!   both sides hold gets 451; an unknown one 550; the server's own create#EmailAccount refuses a replicated address
//!   by name; the local node's mail out through `edge:` to a server account and an outside one is stored on the
//!   server signed (DKIM pass for the local node's domain) and only the outside one goes to the MX; mail for the node
//!   taken while its link is down arrives after it returns; after unpairing the server's own mail is still stored.
//! - Ownership: a Configure that names the server's own chat bridge, XMPP domain, bounce domain and MX host has each
//!   refused with an error naming it (its setup field as the mode), and the kept setup has only the node's own; a
//!   replicated chat bridge and mail account equal to the server's own are held back as replica conflicts, listed by
//!   get#ReplicaConflicts and on System › Edge Clients, and the bridge is applied by recheck#ReplicaConflicts once the
//!   server's is gone. Webhooks: the server's own bridge answered here with nothing queued, the node's queued as
//!   chatWebhook and taken on the local node, an id on both sides 409. The server's own XMPP server answers for its
//!   domain beside the node's tunnel port, also after unpairing.
//! - Calls: the same callSessionId on both sides (ids of two databases) in rooms of their own: each side's browsers
//!   hear each other and see only their own participants, with the bare id in their frames; each call recorded, the
//!   server's segments kept on the server and the node's reaching it through the queue.
//! - SIP: the server's own number routed by the server, the node's (from its merged trunk) by the local node; with the
//!   link down the node's number is answered by its fallback and the server's still by the server. The registrar
//!   checks a phone of the server's domain here and one of the node's domain on the local node.
//!
//! Not covered here: the error the SMTP listener logs for an address on both sides (the child's log is not read),
//! the chat media 409 for an id on both sides (only its webhook), and SIP phones of the same username in both domains
//! (create#SipDevice generates a random username; the two phones here share their device id instead). The local node
//! keeps the node's SIP domain from the Configure's answer; the server's engine reads it from the kept Configure every
//! few seconds, so the test waits for its offer.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "embedded-postgres",
    feature = "rtc",
    feature = "sip",
    feature = "inbound-smtp",
    feature = "channels",
    feature = "xmpp"
))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read as _, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine as _;
use moqui_core::edge::setup::EdgeSetup;
use moqui_core::edge::{EdgeHostInfo, LinkState, HOST_PASSTHROUGH};
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::{to_plain_string, Value};
use moqui_sip::media::PhoneMedia;
use moqui_sip::sdp;
use rsipstack::sip::prelude::HeadersExt;
use rustls::pki_types::CertificateDer;
use serde_json::{json, Value as Json};
use tokio::sync::watch;

use crate::common::Gate;
use crate::edge_call::{certs, wait_until};
use crate::rtc::{open_socket, run_until, Peer};
use crate::sip::{tone, values, Trunk};

/// What the server's child process is given; set only for that child.
const SERVER_ENV: &str = "MOQUI_EDGE_ALLINONE_NODE";
const SERVER_TEST: &str = "edge_allinone::server_process";
/// How long the server's process may take to print its ready line.
const SERVER_START: Duration = Duration::from_secs(40);
/// How long one command to the server's process may take.
const ANSWER: Duration = Duration::from_secs(20);

/// Both nodes: quick queue retries, no IMAP IDLE.
const CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>"#;

// ---------------------------------------------------------------------------------------------------------------------
// The server's process

/// The server's process: an all-in-one node on its own database that answers the test's commands on its stdin until it
/// closes.
#[test]
fn server_process() {
    let Some(setup) = std::env::var_os(SERVER_ENV) else {
        return;
    };
    if !crate::common::own_process() {
        return;
    }
    let setup: Json = serde_json::from_str(&setup.to_string_lossy()).unwrap();
    let port = setup["port"].as_u64().unwrap().to_string();
    let label = setup["label"].as_str().unwrap().to_string();
    let conf = setup["conf"].as_str().unwrap_or_default().to_string();
    let mut properties: Vec<(String, String)> = vec![
        ("node_role".to_string(), "all-in-one".to_string()),
        ("webapp_https_listen_port".to_string(), port),
    ];
    if let Some(map) = setup["properties"].as_object() {
        properties.extend(
            map.iter()
                .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string())),
        );
    }
    let properties: Vec<(&str, &str)> = properties.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let (cert, key) = (setup["cert"].as_str().unwrap(), setup["key"].as_str().unwrap());
    let gate = crate::common::start_with_conf_files(
        &label,
        None,
        &conf,
        &properties,
        &[("cert/default.crt", cert), ("cert/default.key", key)],
    );
    let factory = Arc::clone(gate.server.factory());
    let node = moqui_core::cluster::instance_id(&factory).unwrap();
    let code = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let _authz = context.disable_authz();
        let out =
            moqui_core::service::sync("org.moqui.edge.EdgeServices.create#PairingCode", &BTreeMap::new()).unwrap();
        out.get("code").map(to_plain_string).unwrap_or_default()
    };
    let shown = |address: Option<SocketAddr>| address.map(|a| a.to_string()).unwrap_or_default();
    println!(
        "EDGE_NODE node={node} code={code} http={} media={} sip={} smtp={} xmpp={} runtime={}",
        gate.server.address(),
        shown(gate.server.rtc_address()),
        shown(gate.server.sip_address()),
        shown(gate.server.smtp_address()),
        shown(gate.server.xmpp_addresses().0),
        gate.runtime.display()
    );
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let command: Json = serde_json::from_str(&line).unwrap_or_default();
        let reply = answer(&factory, &command);
        println!("AIO_REPLY {reply}");
        std::io::stdout().flush().unwrap();
    }
    drop(gate);
}

/// One command of the test, run on the server as `user` (none: no one signed in) with authorization off.
fn answer(factory: &Arc<moqui_core::context::ExecutionContextFactory>, command: &Json) -> Json {
    let context = factory.new_context();
    let _entered = context.enter();
    if let Some(user) = command["user"].as_str() {
        if let Err(err) = moqui_core::user::internal_login_user(user) {
            return json!({"errors": [format!("{user} did not sign in: {err}")]});
        }
    }
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let text = |name: &str| command[name].as_str().unwrap_or_default().to_string();
    let map_of = |json: &Json| -> BTreeMap<String, Value> {
        match from_json(json) {
            Value::Map(map) => map.into_iter().collect(),
            _ => BTreeMap::new(),
        }
    };
    match text("op").as_str() {
        "call" => {
            let service = text("service");
            match moqui_core::service::sync(&service, &map_of(&command["params"])) {
                Ok(out) => {
                    let errors =
                        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec())
                            .unwrap_or_default();
                    json!({"out": to_json(&Value::Map(out.into_iter().collect())), "errors": errors})
                }
                Err(err) => json!({"errors": [format!("{service}: {err}")]}),
            }
        }
        "rows" => {
            let mut find = facade.find(&text("entity")).unwrap().use_cache(false).disable_authz();
            if let Some(conditions) = command["where"].as_object() {
                for (name, value) in conditions {
                    find = find.eq(name, Value::string(value.as_str().unwrap_or_default()));
                }
            }
            let rows: Vec<BTreeMap<String, String>> = find
                .list()
                .unwrap()
                .values()
                .iter()
                .map(|row| {
                    row.fields()
                        .iter()
                        .map(|(name, value)| (name.to_string(), to_plain_string(value)))
                        .collect()
                })
                .collect();
            json!({"rows": rows})
        }
        // a row written directly, past the services and their checks
        "write" => {
            let mut row = facade.make_value(&text("entity")).unwrap();
            for (name, value) in map_of(&command["fields"]) {
                row.set(&name, value);
            }
            match row.create_or_update(&facade) {
                Ok(_) => json!({}),
                Err(err) => json!({"errors": [err.to_string()]}),
            }
        }
        "delete" => {
            let mut find = facade.find(&text("entity")).unwrap().disable_authz();
            if let Some(conditions) = command["where"].as_object() {
                for (name, value) in conditions {
                    find = find.eq(name, Value::string(value.as_str().unwrap_or_default()));
                }
            }
            json!({"deleted": find.delete_all().unwrap()})
        }
        // the copies of a paired node's replicated set
        "replica" => {
            let values = moqui_core::edge::replica::list_values(
                &facade,
                Some(&text("node")),
                &text("set"),
                command["entity"].as_str(),
                100,
            )
            .unwrap();
            // each copy's key and fields as one object
            let merged: Vec<Json> = values
                .iter()
                .map(|value| {
                    let mut all = value.fields.as_object().cloned().unwrap_or_default();
                    if let Some(pk) = value.pk.as_object() {
                        all.extend(pk.clone());
                    }
                    Json::Object(all)
                })
                .collect();
            json!({"values": merged})
        }
        "flush_sessions" => {
            factory.flush_sessions().unwrap();
            json!({})
        }
        other => json!({"errors": [format!("no command {other}")]}),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The test's side

/// The all-in-one server, in its child process.
struct Server {
    child: Child,
    stdin: Mutex<Option<ChildStdin>>,
    replies: Mutex<Receiver<String>>,
    node: String,
    code: String,
    https_port: u16,
    root: CertificateDer<'static>,
    http: SocketAddr,
    line: String,
    runtime: std::path::PathBuf,
}

/// The server's process, started; [`Starting::ready`] waits for it.
struct Starting {
    child: Child,
    lines: Receiver<String>,
    https_port: u16,
    root: CertificateDer<'static>,
}

impl Server {
    /// Start the server's process with these properties (beside `node_role=all-in-one` and its HTTPS port) and conf.
    fn spawn(label: &str, conf: &str, properties: &[(&str, &str)]) -> Starting {
        let (root, cert, key) = certs();
        let https_port = crate::common::free_port();
        let properties: serde_json::Map<String, Json> = properties
            .iter()
            .map(|(name, value)| ((*name).to_string(), Json::String((*value).to_string())))
            .collect();
        let setup = json!({"label": label, "port": https_port, "cert": cert, "key": key, "conf": conf,
            "properties": properties});
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([SERVER_TEST, "--exact", "--nocapture", "--test-threads", "1"])
            .env("MOQUI_TEST_OWN_PROCESS", SERVER_TEST)
            .env(SERVER_ENV, setup.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server's process");
        // its ready line and its replies, read on a thread so a child that hangs fails the test instead of holding it
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                for prefix in ["EDGE_NODE ", "AIO_REPLY "] {
                    if let Some(at) = line.find(prefix) {
                        let _ = tx.send(line[at..].to_string());
                    }
                }
            }
        });
        Starting {
            child,
            lines,
            https_port,
            root,
        }
    }

    fn field(&self, name: &str) -> String {
        self.line
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .unwrap_or_default()
            .to_string()
    }

    fn address(&self, name: &str) -> SocketAddr {
        self.field(name)
            .parse()
            .unwrap_or_else(|_| panic!("the server's {name} address in {}", self.line))
    }

    fn url(&self) -> String {
        format!("https://localhost:{}", self.https_port)
    }

    fn command(&self, command: &Json) -> Json {
        {
            let mut stdin = self.stdin.lock().unwrap();
            let stdin = stdin.as_mut().expect("the server's stdin");
            writeln!(stdin, "{command}").unwrap();
            stdin.flush().unwrap();
        }
        let reply = self
            .replies
            .lock()
            .unwrap()
            .recv_timeout(ANSWER)
            .unwrap_or_else(|_| panic!("the server did not answer {command}"));
        serde_json::from_str(reply.trim_start_matches("AIO_REPLY ")).unwrap()
    }

    /// A service on the server as `user`: its out-parameters and its errors.
    fn call(&self, user: &str, service: &str, params: Json) -> (Json, Vec<String>) {
        let reply = self.command(&json!({"op": "call", "user": user, "service": service, "params": params}));
        let errors = reply["errors"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|e| e.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        (reply["out"].clone(), errors)
    }

    /// A service on the server as `user` that must add no errors.
    fn ok(&self, user: &str, service: &str, params: Json) -> Json {
        let (out, errors) = self.call(user, service, params);
        assert!(errors.is_empty(), "{service} on the server: {errors:?}");
        out
    }

    /// The rows of `entity` on the server with these field values.
    fn rows(&self, entity: &str, conditions: Json) -> Vec<BTreeMap<String, String>> {
        let reply = self.command(&json!({"op": "rows", "entity": entity, "where": conditions}));
        serde_json::from_value(reply["rows"].clone()).unwrap()
    }

    /// A row written on the server directly, past its services' checks.
    fn write(&self, entity: &str, fields: Json) {
        let reply = self.command(&json!({"op": "write", "entity": entity, "fields": fields}));
        assert!(reply["errors"].is_null(), "{reply}");
    }

    fn delete(&self, entity: &str, conditions: Json) {
        self.command(&json!({"op": "delete", "entity": entity, "where": conditions}));
    }

    /// The fields of the server's copies of `node`'s set `set`.
    fn replica(&self, node: &str, set: &str) -> Vec<Json> {
        let reply = self.command(&json!({"op": "replica", "node": node, "set": set}));
        reply["values"].as_array().cloned().unwrap_or_default()
    }

    /// Whether the server has `node`'s set `set` whole.
    fn has_set(&self, node: &str, set: &str) -> bool {
        self.rows(
            "moqui.edge.EdgeReplicaCursor",
            json!({"pairedNodeId": node, "setName": set}),
        )
        .iter()
        .any(|row| !row.get("snapshotDate").is_none_or(String::is_empty))
    }

    /// Whether the server has `node`'s down connection.
    fn down(&self, node: &str) -> bool {
        let out = self.ok("john.doe", "org.moqui.edge.EdgeServices.get#EdgeClients", json!({}));
        out["clientList"].as_array().is_some_and(|list| {
            list.iter()
                .any(|row| row["pairedNodeId"] == json!(node) && row["down"] == json!(true))
        })
    }
}

impl Starting {
    fn ready(mut self) -> Server {
        let line = match self.lines.recv_timeout(SERVER_START) {
            Ok(line) if line.starts_with("EDGE_NODE ") => line,
            outcome => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("the server did not start: {outcome:?}");
            }
        };
        let stdin = self.child.stdin.take();
        let mut server = Server {
            child: self.child,
            stdin: Mutex::new(stdin),
            replies: Mutex::new(self.lines),
            node: String::new(),
            code: String::new(),
            https_port: self.https_port,
            root: self.root,
            http: SocketAddr::from(([127, 0, 0, 1], 0)),
            line,
            runtime: std::path::PathBuf::new(),
        };
        server.node = server.field("node");
        server.code = server.field("code");
        server.http = server.address("http");
        server.runtime = std::path::PathBuf::from(server.field("runtime"));
        server
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // closing its stdin stops it in order
        drop(self.stdin.lock().unwrap().take());
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// The local node's link to the server: a dialer the test starts and stops.
struct Link {
    rt: tokio::runtime::Runtime,
    config: LocalConfig,
    shutdown: (watch::Sender<bool>, watch::Receiver<bool>),
    dialer: Option<Arc<Dialer>>,
    node: String,
}

impl Link {
    /// Pair the local node with the server, then link.
    fn up(server: &Server, local: &Gate) -> Link {
        let factory = local.server.factory();
        let node = moqui_core::cluster::instance_id(factory).unwrap();
        assert_ne!(node, server.node);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let mut pairing = PairConfig::new(&server.url(), &server.code, &node);
        pairing.trust_root = Some(server.root.clone());
        pairing.connect_timeout = Duration::from_secs(5);
        let paired = rt.block_on(pair(&pairing)).unwrap();
        let mut config = LocalConfig::new(&server.url(), &paired.key, &paired.pin, &node);
        config.backoff = Backoff {
            first: Duration::from_millis(100),
            max: Duration::from_millis(500),
        };
        config.connect_timeout = Duration::from_secs(3);
        config.web = local_web(factory);
        let mut link = Link {
            rt,
            config,
            shutdown: watch::channel(false),
            dialer: None,
            node,
        };
        link.start(server);
        link
    }

    fn start(&mut self, server: &Server) {
        let dialer = Dialer::start(self.config.clone(), self.rt.handle(), self.shutdown.1.clone(), None).unwrap();
        wait_until(Duration::from_secs(10), "the link up", || {
            dialer.status().state == LinkState::Linked
        });
        wait_until(Duration::from_secs(5), "the down connection", || {
            server.down(&self.node)
        });
        self.dialer = Some(Arc::new(dialer));
    }

    fn stop(&mut self) {
        if let Some(dialer) = self.dialer.take() {
            dialer.stop();
        }
    }

    fn dialer(&self) -> &Dialer {
        self.dialer.as_ref().expect("the link")
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A service on the local node as `user` with authorization off: its out-parameters and its errors.
fn local_call(
    local: &Gate,
    user: &str,
    name: &str,
    parameters: &[(&str, Value)],
) -> (BTreeMap<String, Value>, Vec<String>) {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    (out, errors)
}

/// A service on the local node as `user` that must add no errors.
fn local_ok(local: &Gate, user: &str, name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let (out, errors) = local_call(local, user, name, parameters);
    assert!(errors.is_empty(), "{name} on the local node: {errors:?}");
    out
}

/// The rows of `entity` on the local node with these field values.
fn local_rows(local: &Gate, entity: &str, conditions: &[(&str, &str)]) -> Vec<BTreeMap<String, String>> {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut find = factory
        .entity()
        .unwrap()
        .find(entity)
        .unwrap()
        .use_cache(false)
        .disable_authz();
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

fn text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn json_text(out: &Json, key: &str) -> String {
    match &out[key] {
        Json::String(text) => text.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Mail

/// What an MX took: each message's RCPT TO addresses and its data, with its `MAIL FROM` line first.
type Taken = Mutex<Vec<(Vec<String>, String)>>;

/// A recipient domain's MX.
struct Mx {
    address: SocketAddr,
    taken: Arc<Taken>,
}

fn mx() -> Mx {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let taken = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&taken);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let keep = Arc::clone(&keep);
            std::thread::spawn(move || mx_session(stream, &keep));
        }
    });
    Mx { address, taken }
}

fn mx_session(stream: TcpStream, taken: &Taken) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut say = |line: &str| writer.write_all(format!("{line}\r\n").as_bytes());
    if say("220 mx.example.net ESMTP").is_err() {
        return;
    }
    let (mut sender, mut rcpts) = (String::new(), Vec::new());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let upper = line.trim_end().to_ascii_uppercase();
        let answer = if upper.starts_with("MAIL FROM") {
            sender = line.trim_end().to_string();
            rcpts.clear();
            "250 2.1.0 Ok"
        } else if upper.starts_with("RCPT TO") {
            rcpts.push(line.trim_end()["RCPT TO:".len()..].trim().to_string());
            "250 2.1.5 Ok"
        } else if upper == "DATA" {
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
            taken
                .lock()
                .unwrap()
                .push((rcpts.clone(), format!("{sender}\r\n{message}")));
            "250 2.0.0 Ok: queued"
        } else if upper == "QUIT" {
            let _ = say("221 2.0.0 Bye");
            return;
        } else {
            "250 mx.example.net"
        };
        if say(answer).is_err() {
            return;
        }
    }
}

/// One reply of an SMTP server: its lines joined with `|`.
fn smtp_reply(reader: &mut BufReader<TcpStream>) -> String {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
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

/// A gateway's message to the server's SMTP listener: each RCPT's reply, and the reply after the data (empty when no
/// recipient was taken).
fn smtp(address: SocketAddr, recipients: &[&str], message: &str) -> (Vec<String>, String) {
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut command = |line: &str, reader: &mut BufReader<TcpStream>| {
        writer.write_all(format!("{line}\r\n").as_bytes()).unwrap();
        smtp_reply(reader)
    };
    assert!(smtp_reply(&mut reader).starts_with("220 "));
    assert!(command("EHLO gateway.example.org", &mut reader).starts_with("250"));
    assert!(command("MAIL FROM:<dan@example.org>", &mut reader).starts_with("250"));
    let rcpt: Vec<String> = recipients
        .iter()
        .map(|address| command(&format!("RCPT TO:<{address}>"), &mut reader))
        .collect();
    if !rcpt.iter().any(|line| line.starts_with("250")) {
        command("QUIT", &mut reader);
        return (rcpt, String::new());
    }
    assert!(command("DATA", &mut reader).starts_with("354"));
    let mut stuffed = String::new();
    for line in message.split_inclusive("\r\n") {
        if line.starts_with('.') {
            stuffed.push('.');
        }
        stuffed.push_str(line);
    }
    stuffed.push_str(".\r\n");
    let after = command(stuffed.trim_end_matches("\r\n"), &mut reader);
    command("QUIT", &mut reader);
    (rcpt, after)
}

fn message(message_id: &str, to: &str, body: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nFrom: Dan <dan@example.org>\r\nTo: {to}\r\nSubject: To the all-in-one node\r\n\
         Date: Mon, 2 Nov 2026 12:00:00 +0000\r\n\r\n{body}\r\n"
    )
}

const SALES: &str = "sales@server.example";
const ANN: &str = "ann@desk.example";
const BOB: &str = "bob@desk.example";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";

/// The server's own mail and the local node's at once: split by recipient, stored once on each side.
#[test]
fn mail_to_recipients_on_both_sides_is_delivered_once_to_each() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let mx = mx();
    let mx_override = format!("example.net=127.0.0.1:{}", mx.address.port());
    let starting = Server::spawn(
        "aio-mail-server",
        CONF,
        &[
            ("smtp_listen_port", "0"),
            ("smtp_allowed_addresses", "127.0.0.1"),
            ("smtp_hostname", "mx.server.example"),
            ("edge_mail_mta", "moqui"),
            ("edge_mail_bounce_domain", "bounce.server.example"),
            ("mail_mx_override", &mx_override),
            ("mail_delivery_retry_seconds", "0.2"),
            ("mail_dns_offline", "true"),
        ],
    );
    let local =
        crate::common::start_with_conf_files("aio-mail-local", None, CONF, &[("mail_dns_offline", "true")], &[]);
    let server = starting.ready();
    let smtp_at = server.address("smtp");
    let account = |kind: &str, address: &str| {
        text(
            &local_ok(
                &local,
                "john.doe",
                "org.moqui.mail.EmailClientServices.create#EmailAccount",
                &[
                    ("accountKindEnumId", Value::string(kind)),
                    ("emailAddress", Value::string(address)),
                ],
            ),
            "emailAccountId",
        )
    };
    let ann = account("EakHosted", ANN);
    account("EakHosted", BOB);
    let sales = json_text(
        &server.ok(
            "john.doe",
            "org.moqui.mail.EmailClientServices.create#EmailAccount",
            json!({"accountKindEnumId": "EakHosted", "emailAddress": SALES}),
        ),
        "emailAccountId",
    );
    let mut link = Link::up(&server, &local);
    let node = link.node.clone();
    wait_until(Duration::from_secs(10), "the node's recipients replicated", || {
        server.has_set(&node, "mailRecipients")
    });
    let stored = |message_id: &str| {
        (
            server
                .rows(MESSAGE, json!({"emailAccountId": sales, "messageId": message_id}))
                .len(),
            local_rows(&local, MESSAGE, &[("emailAccountId", &ann), ("messageId", message_id)]).len(),
        )
    };
    let mail_items = || {
        server.rows(
            "moqui.edge.EdgeQueueItem",
            json!({"pairedNodeId": node, "kind": "mail"}),
        )
    };

    // one message to a recipient on each side, and one on neither
    let both = message("<both@example.org>", &format!("{SALES}, {ANN}"), "To both sides");
    let (rcpt, after) = smtp(smtp_at, &[SALES, ANN, "nobody@server.example"], &both);
    assert!(rcpt[0].starts_with("250"), "{rcpt:?}");
    assert!(rcpt[1].starts_with("250"), "{rcpt:?}");
    assert!(rcpt[2].starts_with("550"), "an unknown address: {rcpt:?}");
    assert!(after.starts_with("250"), "{after}");
    wait_until(Duration::from_secs(10), "stored once on each side", || {
        stored("<both@example.org>") == (1, 1)
    });
    let items = mail_items();
    assert_eq!(items.len(), 1, "one item for the node: {items:?}");
    let payload: Json = serde_json::from_str(&items[0]["payload"]).unwrap();
    let recipients: Vec<&str> = payload["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["address"].as_str())
        .collect();
    assert_eq!(recipients, [ANN], "only the node's recipient goes to it");
    // the same message again: nothing new on either side
    let (rcpt, after) = smtp(smtp_at, &[SALES, ANN], &both);
    assert!(
        rcpt.iter().all(|r| r.starts_with("250")) && after.starts_with("250"),
        "{rcpt:?} {after}"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(stored("<both@example.org>"), (1, 1), "a resend stores nothing new");

    // the server's own services refuse an address the node holds, naming it
    let (_, errors) = server.call(
        "john.doe",
        "org.moqui.mail.EmailClientServices.create#EmailAccount",
        json!({"accountKindEnumId": "EakHosted", "emailAddress": ANN}),
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains(&format!("mail address {ANN} is held by paired node {node}"))),
        "{errors:?}"
    );
    assert!(server
        .rows("moqui.basic.email.EmailAccount", json!({"emailAddress": ANN}))
        .is_empty());
    // an address both sides hold (made past the check) is refused for now, never delivered twice
    server.write(
        "moqui.basic.email.EmailAccount",
        json!({"emailAccountId": "AIO_BOB", "emailAddress": BOB, "accountKindEnumId": "EakHosted"}),
    );
    let (rcpt, _) = smtp(smtp_at, &[BOB], &message("<bob@example.org>", BOB, "To bob"));
    assert!(rcpt[0].starts_with("451 4.3.0"), "{rcpt:?}");
    server.delete("moqui.basic.email.EmailAccount", json!({"emailAccountId": "AIO_BOB"}));

    // the node's mail out through the server: a server account's copy stored there, signed for the node's domain; only
    // the outside recipient goes to the MX
    local_ok(
        &local,
        "john.doe",
        "create#moqui.basic.email.EmailServer",
        &[
            ("emailServerId", Value::string("EDGE_OUT")),
            ("smtpHost", Value::string("edge:")),
        ],
    );
    local_ok(
        &local,
        "john.doe",
        &format!("create#{MESSAGE}"),
        &[
            ("emailMessageId", Value::string("AIO_OUT_1")),
            ("statusId", Value::string("ES_READY")),
            ("emailAccountId", Value::string(&ann)),
            ("emailServerId", Value::string("EDGE_OUT")),
            ("fromAddress", Value::string(ANN)),
            ("toAddresses", Value::string(format!("{SALES}, out@example.net"))),
            ("subject", Value::string("Out through the all-in-one node")),
            ("bodyText", Value::string("Hello from the desk")),
        ],
    );
    let sent = local_ok(
        &local,
        "john.doe",
        "org.moqui.mail.EmailServices.send#EmailMessage",
        &[("emailMessageId", Value::string("AIO_OUT_1"))],
    );
    assert_eq!(sent.get("sendError"), None, "{sent:?}");
    let out_id = local_rows(&local, MESSAGE, &[("emailMessageId", "AIO_OUT_1")])[0]["messageId"].clone();
    wait_until(
        Duration::from_secs(10),
        "the node's mail in the server's account",
        || {
            !server
                .rows(MESSAGE, json!({"emailAccountId": sales, "messageId": out_id}))
                .is_empty()
        },
    );
    let headers = server.rows(MESSAGE, json!({"emailAccountId": sales, "messageId": out_id}))[0]
        .get("headersString")
        .cloned()
        .unwrap_or_default();
    assert!(
        headers.contains("Authentication-Results: mx.server.example;"),
        "{headers}"
    );
    assert!(headers.contains("dkim=pass header.d=desk.example"), "{headers}");
    wait_until(Duration::from_secs(10), "the outside recipient at the MX", || {
        !mx.taken.lock().unwrap().is_empty()
    });
    std::thread::sleep(Duration::from_millis(300));
    let taken = mx.taken.lock().unwrap().clone();
    assert_eq!(taken.len(), 1, "{taken:?}");
    assert_eq!(taken[0].0, ["<out@example.net>"], "only the outside recipient");
    assert!(taken[0].1.contains("DKIM-Signature: "), "signed on the server");

    // the node away: its mail waits on the server and arrives after it returns; the server's own is stored meanwhile
    link.stop();
    wait_until(Duration::from_secs(5), "the link down", || !server.down(&node));
    let (rcpt, after) = smtp(
        smtp_at,
        &[SALES, ANN],
        &message(
            "<away@example.org>",
            &format!("{SALES}, {ANN}"),
            "While the desk is away",
        ),
    );
    assert!(
        rcpt.iter().all(|r| r.starts_with("250")) && after.starts_with("250"),
        "{rcpt:?} {after}"
    );
    wait_until(Duration::from_secs(10), "the server's own copy", || {
        stored("<away@example.org>").0 == 1
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(stored("<away@example.org>").1, 0, "it waits while the node is away");
    link.start(&server);
    wait_until(Duration::from_secs(10), "the node's copy after it returns", || {
        stored("<away@example.org>") == (1, 1)
    });

    // unpaired: the node's addresses are no longer taken, the server's own mail still is
    link.stop();
    server.ok(
        "john.doe",
        "org.moqui.edge.EdgeServices.revoke#EdgeClient",
        json!({"pairedNodeId": node}),
    );
    let (rcpt, after) = smtp(
        smtp_at,
        &[SALES, ANN],
        &message("<after@example.org>", &format!("{SALES}, {ANN}"), "After unpairing"),
    );
    assert!(rcpt[0].starts_with("250"), "{rcpt:?}");
    assert!(!rcpt[1].starts_with("250"), "the unpaired node's address: {rcpt:?}");
    assert!(after.starts_with("250"), "{after}");
    wait_until(Duration::from_secs(10), "the server's own mail after unpairing", || {
        stored("<after@example.org>").0 == 1
    });
    drop(link);
    drop(server);
    eprintln!("all-in-one mail test took {:?}", started.elapsed());
}

// ---------------------------------------------------------------------------------------------------------------------
// Ownership, chat and XMPP

const OWN_BRIDGE: &str = "OWN1";
/// The server's bridge the local node also replicates (a late claim), removed later.
const OWN_CLAIMED: &str = "OWN2";
const LOCAL_BRIDGE: &str = "LOCAL1";
const XMPP_DOMAIN: &str = "chat.server.example";

/// A Telnyx webhook of `body`, signed now with `key`, posted to the server: its status and body.
fn post_webhook(
    client: &reqwest::blocking::Client,
    url: &str,
    bridge_id: &str,
    key: &Ed25519KeyPair,
    body: &Json,
) -> (u16, String) {
    let body = body.to_string();
    let stamp = jiff::Timestamp::now().as_second().to_string();
    let mut signed = stamp.as_bytes().to_vec();
    signed.push(b'|');
    signed.extend_from_slice(body.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&signed).as_ref());
    let response = client
        .post(format!("{url}/channels/{bridge_id}/webhook"))
        .header("content-type", "application/json")
        .header("telnyx-signature-ed25519", signature)
        .header("telnyx-timestamp", stamp)
        .body(body)
        .send()
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().unwrap_or_default())
}

/// The first reply of the server's own XMPP port to a stream for `domain`.
fn xmpp_features(address: SocketAddr, domain: &str) -> String {
    let mut tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let open = format!(
        "<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' \
         to='{domain}' version='1.0'>"
    );
    tcp.write_all(open.as_bytes()).unwrap();
    let mut seen = String::new();
    let mut buf = [0_u8; 4096];
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !seen.contains("</features>") && !seen.contains("</stream:features>") {
        match tcp.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }
    seen
}

/// Configure and replication refuse what the server holds, by name; webhooks go by the bridge's owner; the server's
/// own XMPP server stays its own.
#[test]
fn claimed_items_are_refused_by_name_and_webhooks_go_by_owner() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let tunnel_port = crate::common::free_port().to_string();
    let starting = Server::spawn(
        "aio-own-server",
        CONF,
        &[
            ("channels_webhooks_enabled", "true"),
            ("channels_media_key", "an-all-in-one-test-key"),
            ("smtp_hostname", "mx.server.example"),
            ("edge_mail_bounce_domain", "bounce.server.example"),
            ("xmpp_domain", XMPP_DOMAIN),
            ("xmpp_port", "0"),
            ("xmpp_host", "127.0.0.1"),
            ("edge_xmpp_port", &tunnel_port),
            ("edge_xmpp_host", "127.0.0.1"),
        ],
    );
    let local = crate::common::start_with_conf_files(
        "aio-own-local",
        None,
        CONF,
        &[("channels_webhooks_enabled", "true")],
        &[],
    );
    let server = starting.ready();
    let signer = Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
    let public = base64::engine::general_purpose::STANDARD.encode(signer.public_key().as_ref());
    // a provider no one reaches: these webhooks need only the key
    let config = json!({"apiKey": "KEY-AIO", "publicKey": public, "apiBaseUrl": "http://127.0.0.1:9"}).to_string();
    // bridges of fixed ids, written as rows (store#Bridge makes its own id)
    let bridge = |id: &str| {
        json!({"chatBridgeId": id, "name": id, "typeEnumId": "CbtSms", "adapterName": "sms",
            "statusId": "CbsNotConnected", "configJson": config, "webhookPublicKey": public})
    };
    for id in [OWN_BRIDGE, OWN_CLAIMED] {
        server.write("moqui.chat.ChatBridge", bridge(id));
    }
    let local_bridge = |id: &str| {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.chat.ChatBridge").unwrap();
        if let Value::Map(fields) = from_json(&bridge(id)) {
            for (name, value) in fields {
                row.set(&name, value);
            }
        }
        row.create_or_update(&facade).unwrap();
    };
    local_bridge(LOCAL_BRIDGE);
    server.ok(
        "john.doe",
        "org.moqui.mail.EmailClientServices.create#EmailAccount",
        json!({"accountKindEnumId": "EakHosted", "emailAddress": SALES}),
    );
    let link = Link::up(&server, &local);
    let node = link.node.clone();
    wait_until(Duration::from_secs(10), "the node's bridges replicated", || {
        server.has_set(&node, "chatBridges")
    });

    // Configure: each item the server holds is refused by name, with its setup field as the mode
    let setup = EdgeSetup {
        hosts: vec![EdgeHostInfo {
            host_name: "mx.server.example".to_string(),
            paired_node_id: String::new(),
            mode_enum_id: HOST_PASSTHROUGH.to_string(),
            sticky_cookie: None,
        }],
        set_hosts: true,
        mail_domains: vec!["bounce.server.example".to_string(), "desk.example".to_string()],
        chat_bridge_ids: vec![OWN_BRIDGE.to_string(), LOCAL_BRIDGE.to_string()],
        xmpp_domain: XMPP_DOMAIN.to_string(),
        ..EdgeSetup::default()
    };
    let configured = link.rt.block_on(link.dialer().configure(&setup)).unwrap();
    let refused = |mode: &str, name: &str| {
        configured
            .hosts
            .iter()
            .find(|h| h.mode_enum_id == mode && h.host_name == name)
            .map(|h| h.error.clone())
            .unwrap_or_else(|| panic!("no refusal of {mode} {name}: {:?}", configured.hosts))
    };
    assert!(refused("chatBridgeIds", OWN_BRIDGE).contains(OWN_BRIDGE));
    assert!(refused("xmppDomain", XMPP_DOMAIN).contains(XMPP_DOMAIN));
    assert!(refused("mailDomains", "bounce.server.example").contains("bounce.server.example"));
    assert!(
        !refused(HOST_PASSTHROUGH, "mx.server.example").is_empty(),
        "the server's MX host"
    );
    assert!(
        !configured
            .hosts
            .iter()
            .any(|h| !h.error.is_empty() && (h.host_name == LOCAL_BRIDGE || h.host_name == "desk.example")),
        "{:?}",
        configured.hosts
    );
    let client_row = server.rows("moqui.edge.EdgeClient", json!({"pairedNodeId": node}));
    let kept: Json = serde_json::from_str(&client_row[0]["setupJson"]).unwrap();
    assert_eq!(kept["chatBridgeIds"], json!([LOCAL_BRIDGE]), "{kept}");
    assert_eq!(kept["xmppDomain"], json!(""), "{kept}");
    assert_eq!(kept["mailDomains"], json!(["desk.example"]), "{kept}");

    // late claims: a bridge and an address of the server's, replicated by the node, are held back and listed
    local_bridge(OWN_CLAIMED);
    local_ok(
        &local,
        "john.doe",
        "org.moqui.mail.EmailClientServices.create#EmailAccount",
        &[
            ("accountKindEnumId", Value::string("EakHosted")),
            ("emailAddress", Value::string(SALES)),
        ],
    );
    let conflicts = || {
        let out = server.ok(
            "john.doe",
            "org.moqui.edge.EdgeServices.get#ReplicaConflicts",
            json!({"pairedNodeId": node}),
        );
        out["conflictList"].as_array().cloned().unwrap_or_default()
    };
    wait_until(Duration::from_secs(10), "the late claims held back", || {
        let found = conflicts();
        found.iter().any(|c| c["itemKey"] == json!(OWN_CLAIMED)) && found.iter().any(|c| c["itemKey"] == json!(SALES))
    });
    let copies = server.replica(&node, "chatBridges");
    assert!(
        copies.iter().any(|c| c["chatBridgeId"] == json!(LOCAL_BRIDGE)),
        "{copies:?}"
    );
    assert!(
        !copies.iter().any(|c| c["chatBridgeId"] == json!(OWN_CLAIMED)),
        "no copy of the claimed bridge: {copies:?}"
    );
    server.command(&json!({"op": "flush_sessions"}));
    let admin = crate::common::sign_in(server.http, "john.doe", "moqui");
    let page = crate::common::http_request("GET", "/System/EdgeClients/Domains", &[("Cookie", &admin)], "");
    let (status, _, page) = crate::common::exchange(server.http, &page);
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("Replica conflicts") && page.contains(OWN_CLAIMED),
        "{page}"
    );
    // once the server's bridge is gone, the node's copy is applied
    server.delete("moqui.chat.ChatBridge", json!({"chatBridgeId": OWN_CLAIMED}));
    let rechecked = server.ok(
        "john.doe",
        "org.moqui.edge.EdgeServices.recheck#ReplicaConflicts",
        json!({"pairedNodeId": node}),
    );
    assert_eq!(rechecked["applied"], json!(1), "{rechecked}");
    assert!(server
        .replica(&node, "chatBridges")
        .iter()
        .any(|c| c["chatBridgeId"] == json!(OWN_CLAIMED)));
    assert!(
        conflicts().iter().all(|c| c["itemKey"] != json!(OWN_CLAIMED)),
        "the address is still held back"
    );

    // webhooks by owner: the server's own answered here, the node's queued for it, an id on both sides refused
    let client = reqwest::blocking::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_der(server.root.as_ref()).unwrap()])
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let webhooks = || {
        server
            .rows(
                "moqui.edge.EdgeQueueItem",
                json!({"pairedNodeId": node, "kind": "chatWebhook"}),
            )
            .len()
    };
    let probe = |id: &str| {
        json!({"data": {"event_type": "message.finalized", "id": format!("ev-{id}"),
            "payload": {"id": "none", "to": [{"phone_number": "+15557778888", "status": "delivered"}]}}})
    };
    let (status, body) = post_webhook(&client, &server.url(), OWN_BRIDGE, &signer, &probe("own"));
    assert_eq!(status, 200, "the server's own bridge: {body}");
    assert!(!body.contains("queued"), "{body}");
    assert_eq!(webhooks(), 0, "nothing queued for the server's own bridge");
    let (status, body) = post_webhook(&client, &server.url(), LOCAL_BRIDGE, &signer, &probe("node"));
    assert_eq!(
        (status, body.contains("queued")),
        (200, true),
        "the node's bridge: {body}"
    );
    assert_eq!(webhooks(), 1);
    wait_until(
        Duration::from_secs(10),
        "the node's webhook taken on the local node",
        || !local_rows(&local, "moqui.edge.EdgeReceipt", &[("kind", "chatWebhook")]).is_empty(),
    );
    server.write("moqui.chat.ChatBridge", bridge(LOCAL_BRIDGE));
    let (status, body) = post_webhook(&client, &server.url(), LOCAL_BRIDGE, &signer, &probe("both"));
    assert_eq!(status, 409, "an id on both sides: {body}");
    assert!(body.contains(LOCAL_BRIDGE), "{body}");
    assert_eq!(webhooks(), 1, "nothing queued for it");
    server.delete("moqui.chat.ChatBridge", json!({"chatBridgeId": LOCAL_BRIDGE}));

    // the server's own XMPP server answers for its domain, beside the node's tunnel port
    let xmpp = server.address("xmpp");
    assert_ne!(xmpp.port().to_string(), tunnel_port);
    let features = xmpp_features(xmpp, XMPP_DOMAIN);
    assert!(
        features.contains(&format!("from='{XMPP_DOMAIN}'")) && features.contains("starttls"),
        "{features}"
    );

    // unpaired: the server's own bridge and XMPP server still answer
    drop(link);
    server.ok(
        "john.doe",
        "org.moqui.edge.EdgeServices.revoke#EdgeClient",
        json!({"pairedNodeId": node}),
    );
    let (status, body) = post_webhook(&client, &server.url(), OWN_BRIDGE, &signer, &probe("after"));
    assert_eq!(status, 200, "the server's own bridge after unpairing: {body}");
    let features = xmpp_features(xmpp, XMPP_DOMAIN);
    assert!(features.contains(&format!("from='{XMPP_DOMAIN}'")), "{features}");
    drop(server);
    eprintln!("all-in-one ownership test took {:?}", started.elapsed());
}

// ---------------------------------------------------------------------------------------------------------------------
// Calls

const SERVER_CALLEE: &str = "server.callee";
const LOCAL_CALLEE: &str = "desk.callee";
const CALLEE_PASSWORD: &str = "All-in-one-77!";

/// A call of john.doe to a new user `callee` on one side, answered: (call, john's leg, the callee's leg).
fn start_call(call: &dyn Fn(&str, &str, Json) -> Json, callee: &str) -> (String, String, String) {
    let created = call(
        "john.doe",
        "org.moqui.impl.UserServices.create#UserAccount",
        json!({"username": callee, "newPassword": CALLEE_PASSWORD, "newPasswordVerify": CALLEE_PASSWORD,
            "userFullName": "All-in-one Callee"}),
    );
    let callee_id = json_text(&created, "userId");
    let started = call(
        "john.doe",
        "org.moqui.call.CallServices.start#Call",
        json!({"calleeUserIdList": [callee_id]}),
    );
    let call_id = json_text(&started, "callSessionId");
    let answered = call(
        callee,
        "org.moqui.call.CallServices.answer#Call",
        json!({"callSessionId": call_id}),
    );
    (
        call_id,
        json_text(&started, "callLegId"),
        json_text(&answered, "callLegId"),
    )
}

/// The segments of the recordings of `call_id`, as `find#Recordings` gives them.
fn segments(found: &Json) -> Vec<Json> {
    found["recordingList"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|r| r["periods"].as_array())
        .map(|periods| {
            periods
                .iter()
                .flat_map(|p| p["segments"].as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// The server's own call and the node's, with the same id from two databases, at once on the server's media node.
#[test]
fn the_same_call_id_on_both_sides_stays_apart_and_each_recording_on_its_side() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let starting = Server::spawn(
        "aio-call-server",
        CONF,
        &[
            ("rtc_media_port", "0"),
            ("turn_port", "0"),
            ("turn_relay_ports", "51400-51500"),
            ("turn_auth_secret", "all-in-one-turn-secret"),
            ("call_recording_enabled", "true"),
        ],
    );
    let local = crate::common::start_with_properties(
        "aio-call-local",
        None,
        &[
            ("turn_auth_secret", "desk-turn-secret"),
            ("call_recording_enabled", "true"),
        ],
    );
    let server = starting.ready();
    let media: SocketAddr = server.address("media");
    assert!(
        local.server.rtc_address().is_none(),
        "the local node has no media socket"
    );
    let link = Link::up(&server, &local);
    wait_until(
        Duration::from_secs(5),
        "the server's media row on the local node",
        || {
            local_rows(&local, "moqui.call.CallMediaNode", &[("instanceId", &server.node)])
                .first()
                .is_some_and(|row| row.get("edgeLinked").map(String::as_str) == Some("Y"))
        },
    );
    assert_eq!(
        server
            .rows("moqui.call.CallMediaNode", json!({"instanceId": server.node}))
            .len(),
        1,
        "the server registers its own media node"
    );

    let on_server = |user: &str, service: &str, params: Json| server.ok(user, service, params);
    let on_local = |user: &str, service: &str, params: Json| {
        let parameters = match from_json(&params) {
            Value::Map(map) => map.into_iter().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let parameters: Vec<(&str, Value)> = parameters.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        to_json(&Value::Map(
            local_ok(&local, user, service, &parameters).into_iter().collect(),
        ))
    };
    let (server_call, server_caller, server_callee) = start_call(&on_server, SERVER_CALLEE);
    let (local_call, local_caller, local_callee) = start_call(&on_local, LOCAL_CALLEE);
    assert_eq!(server_call, local_call, "the same id from the two databases");
    assert_eq!(
        server.rows("moqui.call.CallSession", json!({"callSessionId": server_call}))[0]["mediaNodeId"],
        server.node
    );
    assert_eq!(
        local_rows(&local, "moqui.call.CallSession", &[("callSessionId", &local_call)])[0]["mediaNodeId"],
        server.node,
        "the node's call has its media on the server"
    );

    // four browsers: the server's two on its own /notws, the node's two on the local node's
    server.command(&json!({"op": "flush_sessions"}));
    let server_john = crate::common::sign_in(server.http, "john.doe", "moqui");
    let server_other = crate::common::sign_in(server.http, SERVER_CALLEE, CALLEE_PASSWORD);
    server.command(&json!({"op": "flush_sessions"}));
    let local_http = local.server.address();
    let local_john = crate::common::sign_in(local_http, "john.doe", "moqui");
    let local_other = crate::common::sign_in(local_http, LOCAL_CALLEE, CALLEE_PASSWORD);
    local.server.factory().flush_sessions().unwrap();
    let mut a = Peer::new(open_socket(server.http, &server_john), &server_call, &server_caller);
    let mut b = Peer::new(open_socket(server.http, &server_other), &server_call, &server_callee);
    let mut c = Peer::new(open_socket(local_http, &local_john), &local_call, &local_caller);
    let mut d = Peer::new(open_socket(local_http, &local_other), &local_call, &local_callee);
    for peer in [&mut a, &mut b, &mut c, &mut d] {
        peer.join();
    }
    let flowed = run_until(&mut [&mut a, &mut b, &mut c, &mut d], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "audio in both calls: {} {} {} {}",
        a.received, b.received, c.received, d.received
    );
    for peer in [&a, &b, &c, &d] {
        let joined = peer.frame("joined").cloned().unwrap_or_default();
        assert!(
            joined["sdp"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!(" {} typ host", media.port())),
            "the server's media socket: {joined}"
        );
        // the bare id in every frame, and only the browser's own call's participants
        assert!(
            peer.frames.iter().any(|f| f.get("callSessionId").is_some()),
            "{:?}",
            peer.frames
        );
        for frame in &peer.frames {
            if let Some(id) = frame.get("callSessionId") {
                assert_eq!(id, &json!(server_call), "{frame}");
            }
            if let Some(topic) = frame.get("topic") {
                assert_eq!(topic, &json!(format!("rtc:{server_call}")), "{frame}");
            }
        }
        let participants: std::collections::BTreeSet<String> = peer
            .frames
            .iter()
            .filter(|f| f["rtc"] == json!("participant"))
            .filter_map(|f| f["callLegId"].as_str().map(str::to_string))
            .collect();
        assert!(participants.len() <= 1, "one other participant: {participants:?}");
    }
    let own_participant = |p: &Peer, leg: &str| {
        p.frames
            .iter()
            .any(|f| f["rtc"] == json!("participant") && f["callLegId"] == json!(leg))
    };
    assert!(own_participant(&b, &server_caller) && own_participant(&d, &local_caller));

    // each call recorded: the server's segments stay on the server, the node's reach it through the queue
    let server_recording = json_text(
        &server.ok(
            "john.doe",
            "org.moqui.call.CallRecordingServices.start#Recording",
            json!({"callSessionId": server_call}),
        ),
        "callRecordingId",
    );
    let local_recording = text(
        &local_ok(
            &local,
            "john.doe",
            "org.moqui.call.CallRecordingServices.start#Recording",
            &[("callSessionId", Value::string(&local_call))],
        ),
        "callRecordingId",
    );
    assert!(!server_recording.is_empty() && !local_recording.is_empty());
    run_until(
        &mut [&mut a, &mut b, &mut c, &mut d],
        Duration::from_millis(1_200),
        |_| false,
    );
    server.ok(
        "john.doe",
        "org.moqui.call.CallRecordingServices.stop#Recording",
        json!({"callSessionId": server_call}),
    );
    local_ok(
        &local,
        "john.doe",
        "org.moqui.call.CallRecordingServices.stop#Recording",
        &[("callSessionId", Value::string(&local_call))],
    );
    let mut server_segments = Vec::new();
    let mut local_segments = Vec::new();
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until && (server_segments.len() < 3 || local_segments.len() < 3) {
        run_until(
            &mut [&mut a, &mut b, &mut c, &mut d],
            Duration::from_millis(100),
            |_| false,
        );
        server_segments = segments(&server.ok(
            "john.doe",
            "org.moqui.call.CallRecordingServices.find#Recordings",
            json!({"callSessionId": server_call}),
        ));
        local_segments = segments(&on_local(
            "john.doe",
            "org.moqui.call.CallRecordingServices.find#Recordings",
            json!({"callSessionId": local_call}),
        ));
    }
    assert!(
        server_segments.len() >= 3,
        "the server's call's tracks and mix: {server_segments:?}"
    );
    assert!(
        local_segments.len() >= 3,
        "the node's call's, on the local node: {local_segments:?}; the server's queue: {:?}; now {}",
        server
            .rows("moqui.edge.EdgeQueueItem", json!({}))
            .iter()
            .map(|r| {
                let mut r = r.clone();
                r.remove("payload");
                r
            })
            .collect::<Vec<_>>(),
        moqui_core::user::now_millis()
    );
    // the server keeps only its own call's recording; the node's is the local node's alone
    let server_recordings = server.rows("moqui.call.CallRecording", json!({}));
    assert_eq!(server_recordings.len(), 1, "{server_recordings:?}");
    assert_eq!(server_recordings[0]["callRecordingId"], server_recording);
    assert_eq!(local_rows(&local, "moqui.call.CallRecording", &[]).len(), 1);
    for segment in &local_segments {
        let location = text(
            &local_ok(
                &local,
                "john.doe",
                "org.moqui.call.CallRecordingServices.get#RecordingFile",
                &[("callRecordingSegmentId", from_json(&segment["callRecordingSegmentId"]))],
            ),
            "location",
        );
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let bytes = factory
            .resource()
            .location_bytes(&location)
            .unwrap()
            .unwrap_or_default();
        assert_eq!(&bytes[..4.min(bytes.len())], b"OggS", "{location}");
    }
    drop((a, b, c, d));
    drop(link);
    drop(server);
    eprintln!("all-in-one call test took {:?}", started.elapsed());
}

// ---------------------------------------------------------------------------------------------------------------------
// SIP

const OWN_NUMBER: &str = "+15552220000";
const NODE_NUMBER: &str = "+15553330000";
const SERVER_DOMAIN: &str = "sip.server.example";
const LOCAL_DOMAIN: &str = "sip.local.example";

/// A call from a trunk to `number` at the server's SIP address, answered (200), then hung up by the caller.
fn answered(trunk: &Trunk, sip_at: SocketAddr, number: &str, call_id: &str) {
    let mut buf = vec![0u8; 2048];
    while trunk.rtp.recv_from(&mut buf).is_ok() {}
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let from = "+15559990000";
    let invite = format!(
        "INVITE sip:{number}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}\r\nMax-Forwards: 70\r\n\
         From: <sip:{from}@127.0.0.1>;tag={call_id}\r\nTo: <sip:{number}@{sip_at}>\r\nCall-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\nContact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    let ok = final_response(trunk, call_id, "INVITE");
    let answer = String::from_utf8_lossy(&ok.body).to_string();
    let settled = sdp::accept(&phone_local, &sdp::parse(&answer).unwrap())
        .unwrap_or_else(|err| panic!("{call_id}: the answer {answer:?}: {err:?}"));
    let to = ok.to_header().unwrap().value().to_string();
    let in_dialog = |method: &str, cseq: u32| {
        format!(
            "{method} sip:{number}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}-{cseq}\r\n\
             Max-Forwards: 70\r\nFrom: <sip:{from}@127.0.0.1>;tag={call_id}\r\nTo: {to}\r\nCall-ID: {call_id}\r\n\
             CSeq: {cseq} {method}\r\nContent-Length: 0\r\n\r\n",
            trunk.addr()
        )
    };
    trunk.send(&in_dialog("ACK", 1), sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();
    let until = Instant::now() + Duration::from_millis(400);
    let mut tick = 0;
    while Instant::now() < until {
        if let Some(packet) = phone.send(&tone(tick)) {
            trunk.rtp.send_to(&packet, phone.remote()).unwrap();
        }
        tick += 1;
        while let Ok((len, at)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(at, &buf[..len]);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    trunk.send(&in_dialog("BYE", 2), sip_at);
    final_response(trunk, call_id, "BYE");
}

/// The 200 to `method` of the call `call_id` on the trunk, past anything else (a request, another call's answer).
fn final_response(trunk: &Trunk, call_id: &str, method: &str) -> rsipstack::sip::Response {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match trunk.recv(until) {
            Some((rsipstack::sip::SipMessage::Response(resp), _))
                if u16::from(resp.status_code.clone()) == 200
                    && resp.call_id_header().is_ok_and(|id| id.value() == call_id)
                    && resp.cseq_header().is_ok_and(|cseq| cseq.value().ends_with(method)) =>
            {
                return resp
            }
            Some(_) => continue,
            None => panic!("{call_id}: no 200 to its {method}"),
        }
    }
}

/// A phone's REGISTER to `domain` at the server's SIP address.
fn register_to(phone: SocketAddr, domain: &str, user: &str, cseq: u32, auth: Option<&str>) -> String {
    let auth = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    format!(
        "REGISTER sip:{domain} SIP/2.0\r\nVia: SIP/2.0/UDP {phone};branch=z9hG4bK-reg{cseq}-{user};rport\r\n\
         Max-Forwards: 70\r\nFrom: <sip:{user}@{domain}>;tag=desk\r\nTo: <sip:{user}@{domain}>\r\n\
         Call-ID: reg-{user}-{domain}\r\nCSeq: {cseq} REGISTER\r\nContact: <sip:{user}@{phone}>\r\nExpires: 300\r\n\
         {auth}User-Agent: Spec Desk Phone\r\nContent-Length: 0\r\n\r\n"
    )
}

/// A phone registers `user` with `password` in `domain`: the realm it was challenged in.
fn sign_in_phone(server: SocketAddr, domain: &str, user: &str, password: &str) -> String {
    let phone = Trunk::new();
    let at = phone.addr();
    phone.send(&register_to(at, domain, user, 1, None), server);
    let challenge = phone.response(401);
    let challenges: Vec<moqui_sip::digest::Challenge> = values(&challenge.headers, "WWW-Authenticate")
        .iter()
        .filter_map(|v| moqui_sip::digest::Challenge::parse(v))
        .collect();
    let credentials = moqui_sip::digest::Credentials {
        username: user.to_string(),
        password: password.to_string(),
        algorithms: vec![moqui_sip::digest::Algorithm::Sha256],
        ..Default::default()
    };
    let chosen = moqui_sip::digest::choose(&challenges, &credentials).expect("a SHA-256 challenge");
    let realm = chosen.realm.clone();
    let auth = moqui_sip::digest::answer(&credentials, chosen, "REGISTER", &format!("sip:{domain}"), "c1").unwrap();
    phone.send(&register_to(at, domain, user, 2, Some(&auth)), server);
    phone.response(200);
    realm
}

/// The trunk's REGISTER from the server, answered.
fn registered(trunk: &Trunk) {
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");
}

/// Numbers and phones by owner on one SIP engine: the server's own here, the node's delegated to it.
#[test]
fn own_numbers_and_phones_stay_on_the_server_and_the_nodes_go_to_it() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let starting = Server::spawn(
        "aio-sip-server",
        CONF,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42700-42799"),
            ("sip_registrar_enabled", "true"),
            ("sip_domain", SERVER_DOMAIN),
        ],
    );
    let local = crate::common::start_with_properties("aio-sip-local", None, &[("sip_domain", LOCAL_DOMAIN)]);
    let server = starting.ready();
    let sip_at = server.address("sip");
    assert!(local.server.sip_address().is_none(), "the local node has no SIP engine");
    let live_sip = |status: &Json| {
        status["sipNodeList"].as_array().is_some_and(|nodes| {
            nodes.iter().any(|row| {
                row["instanceId"].as_str() == Some(server.node.as_str()) && row["live"].as_bool() == Some(true)
            })
        })
    };
    // A bound socket may precede the live SipNode row that store#VoiceTrunk uses as its reload target.
    wait_until(Duration::from_secs(5), "the server's live SIP reload target", || {
        live_sip(&server.ok("john.doe", "org.moqui.call.VoiceServices.get#VoiceStatus", json!({})))
    });

    // the server's own trunk and number, to john, who is in a call: his calls go to voicemail
    let own_trunk = Trunk::new();
    let trunk = |port: u16| {
        json!({"name": "Fake trunk", "host": "127.0.0.1", "port": port, "transportEnumId": "VtrUdp",
            "mediaEncryptionEnumId": "VmePlain", "providerProfileEnumId": "VppPlainLan", "username": "acct",
            "password": "pw-of-acct", "registerFlag": "Y"})
    };
    let own_trunk_id = json_text(
        &server.ok(
            "john.doe",
            "org.moqui.call.VoiceServices.store#VoiceTrunk",
            trunk(own_trunk.addr().port()),
        ),
        "voiceTrunkId",
    );
    server.ok(
        "john.doe",
        "org.moqui.call.VoiceServices.store#PhoneNumber",
        json!({"phoneNumber": OWN_NUMBER, "voiceTrunkId": own_trunk_id, "routeTypeEnumId": "PnrUser",
            "routeId": "EX_JOHN_DOE"}),
    );
    server.ok(
        "john.doe",
        "store#moqui.security.UserStatus",
        json!({"userId": "EX_JOHN_DOE", "inCall": "Y"}),
    );
    registered(&own_trunk);

    // the node's: its trunk, number and fallback, replicated to the server
    let mut link = Link::up(&server, &local);
    // Linked transport starts the row keeper asynchronously; wait for the local database's reload target too.
    wait_until(
        Duration::from_secs(5),
        "the local node's live SIP reload target",
        || {
            let status = local_ok(&local, "john.doe", "org.moqui.call.VoiceServices.get#VoiceStatus", &[]);
            live_sip(&to_json(&Value::Map(status.into_iter().collect())))
        },
    );
    let node_trunk = Trunk::new();
    let node_trunk_id = text(
        &local_ok(
            &local,
            "john.doe",
            "org.moqui.call.VoiceServices.store#VoiceTrunk",
            &[
                ("name", Value::string("Desk trunk")),
                ("host", Value::string("127.0.0.1")),
                ("port", Value::I32(i32::from(node_trunk.addr().port()))),
                ("transportEnumId", Value::string("VtrUdp")),
                ("mediaEncryptionEnumId", Value::string("VmePlain")),
                ("providerProfileEnumId", Value::string("VppPlainLan")),
                ("username", Value::string("desk")),
                ("password", Value::string("pw-of-desk")),
                ("registerFlag", Value::string("Y")),
            ],
        ),
        "voiceTrunkId",
    );
    local_ok(
        &local,
        "john.doe",
        "org.moqui.call.VoiceServices.store#PhoneNumber",
        &[
            ("phoneNumber", Value::string(NODE_NUMBER)),
            ("voiceTrunkId", Value::string(&node_trunk_id)),
            ("routeTypeEnumId", Value::string("PnrUser")),
            ("routeId", Value::string("EX_JOHN_DOE")),
        ],
    );
    local_ok(
        &local,
        "john.doe",
        "store#moqui.security.UserStatus",
        &[("userId", Value::string("EX_JOHN_DOE")), ("inCall", Value::string("Y"))],
    );
    let refreshed = local_ok(
        &local,
        "john.doe",
        "org.moqui.call.VoiceServices.refresh#PhoneFallbacks",
        &[],
    );
    assert_eq!(text(&refreshed, "writtenCount"), "1", "{refreshed:?}");
    // the node's SIP domain goes with its Configure, and its answer says the registrar takes it
    let configured = link
        .rt
        .block_on(link.dialer().configure(&EdgeSetup::default()))
        .unwrap();
    assert_eq!(
        (configured.offer.sip.domain.as_str(), configured.offer.sip.registrar),
        (LOCAL_DOMAIN, true)
    );
    let node = link.node.clone();
    wait_until(
        Duration::from_secs(10),
        "the node's trunk and fallback on the server",
        || {
            server.has_set(&node, "voiceTrunks")
                && server
                    .replica(&node, "voiceFallbacks")
                    .iter()
                    .any(|f| f["phoneNumber"] == json!(NODE_NUMBER))
        },
    );
    registered(&node_trunk);
    // the server's engine reads the kept domain every few seconds; the local node keeps it from the answer, at once
    wait_until(
        Duration::from_secs(10),
        "the server's registrar takes the node's domain",
        || {
            link.rt
                .block_on(link.dialer().offer())
                .is_ok_and(|offer| offer.sip.domain == LOCAL_DOMAIN && offer.sip.registrar)
        },
    );
    wait_until(
        Duration::from_secs(5),
        "the node's domain kept on the local node",
        || {
            local_rows(&local, "moqui.call.SipNode", &[("instanceId", &server.node)])
                .first()
                .and_then(|row| row.get("sipDomain").cloned())
                .as_deref()
                == Some(LOCAL_DOMAIN)
        },
    );

    let sessions = || {
        (
            server.rows("moqui.call.CallSession", json!({})).len(),
            local_rows(&local, "moqui.call.CallSession", &[]).len(),
        )
    };
    let (server_before, local_before) = sessions();
    // the server's own number: a call on the server only
    answered(&own_trunk, sip_at, OWN_NUMBER, "aio-sip-own-1");
    wait_until(Duration::from_secs(5), "the own number's call on the server", || {
        sessions().0 == server_before + 1
    });
    assert_eq!(sessions().1, local_before, "nothing on the local node");
    // the node's number: routed by the local node, its media on the server
    answered(&node_trunk, sip_at, NODE_NUMBER, "aio-sip-node-1");
    wait_until(
        Duration::from_secs(5),
        "the node's number's call on the local node",
        || sessions().1 == local_before + 1,
    );
    assert_eq!(sessions().0, server_before + 1, "no call of the node's on the server");
    let node_call = local_rows(&local, "moqui.call.CallSession", &[])
        .into_iter()
        .max_by_key(|row| row["callSessionId"].clone())
        .unwrap();
    assert_eq!(node_call["mediaNodeId"], server.node);

    // phones by domain: the server's domain checked here, the node's on the local node
    let own_device = server.ok(
        "john.doe",
        "org.moqui.call.SipDeviceServices.create#SipDevice",
        json!({"name": "Office"}),
    );
    assert_eq!(json_text(&own_device, "domain"), SERVER_DOMAIN, "{own_device}");
    let realm = sign_in_phone(
        sip_at,
        SERVER_DOMAIN,
        &json_text(&own_device, "username"),
        &json_text(&own_device, "password"),
    );
    assert_eq!(realm, SERVER_DOMAIN);
    let own_device_id = json_text(&own_device, "sipDeviceId");
    wait_until(
        Duration::from_secs(5),
        "the server's phone's binding on the server",
        || {
            !server
                .rows("moqui.call.SipRegistration", json!({"sipDeviceId": own_device_id}))
                .is_empty()
        },
    );
    let node_device = local_ok(
        &local,
        "john.doe",
        "org.moqui.call.SipDeviceServices.create#SipDevice",
        &[("name", Value::string("Home"))],
    );
    assert_eq!(text(&node_device, "domain"), LOCAL_DOMAIN, "{node_device:?}");
    let realm = sign_in_phone(
        sip_at,
        LOCAL_DOMAIN,
        &text(&node_device, "username"),
        &text(&node_device, "password"),
    );
    assert_eq!(realm, LOCAL_DOMAIN);
    let node_device_id = text(&node_device, "sipDeviceId");
    wait_until(
        Duration::from_secs(5),
        "the node's phone checked and bound on the local node",
        || {
            !local_rows(
                &local,
                "moqui.call.SipRegistration",
                &[("sipDeviceId", &node_device_id)],
            )
            .is_empty()
                && local_rows(&local, "moqui.call.SipDevice", &[("sipDeviceId", &node_device_id)])
                    .first()
                    .is_some_and(|row| !row.get("edgeDigestDate").is_none_or(String::is_empty))
        },
    );
    // the two databases gave both phones the same device id: each binding is kept on its own side
    assert_eq!(own_device_id, node_device_id);
    let server_bindings = server.rows("moqui.call.SipRegistration", json!({}));
    assert_eq!(
        server_bindings.len(),
        1,
        "only the server's own phone's binding is on the server: {server_bindings:?}"
    );
    assert_eq!(local_rows(&local, "moqui.call.SipRegistration", &[]).len(), 1);

    // the link down: the node's number is answered from its fallback, the server's own still by the server
    link.stop();
    wait_until(Duration::from_secs(5), "the link down", || !server.down(&node));
    let (server_before, local_before) = sessions();
    answered(&node_trunk, sip_at, NODE_NUMBER, "aio-sip-node-2");
    answered(&own_trunk, sip_at, OWN_NUMBER, "aio-sip-own-2");
    wait_until(Duration::from_secs(5), "the own number's call on the server", || {
        sessions().0 == server_before + 1
    });
    assert_eq!(
        sessions(),
        (server_before + 1, local_before),
        "the fallback makes no call"
    );
    drop(link);
    drop(server);
    eprintln!("all-in-one SIP test took {:?}", started.elapsed());
}
