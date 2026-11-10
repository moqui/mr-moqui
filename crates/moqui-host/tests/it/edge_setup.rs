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

//! Attaching a local node from the Edge Node wizard (plans/Edge-Node.md §5.7, E10). Both ends are real servers in the
//! test's own process, as in `edge_chat.rs`; the local end's link is a dialer driven with an explicit `LocalConfig`,
//! so the wizard's link calls are the dialer's (`offer`, `configure`) and the rest is called as the services call it.
//!
//! - Discover: the edge node's offer has its mail (the MTA in moqui, its MX host and DKIM record), chat, PASS, its
//!   XMPP tunnel ports, its public addresses, its media node and its SIP engine, and no area as not available.
//! - Configure: this node's host names are applied on the edge node, a name another node holds is refused, a name of
//!   this node not listed is removed, and a request that is not input is refused whole.
//! - Apply: the EDGE mail server with `edge:`, SYSTEM's and a hosted account's mail through it, and the chat and PASS
//!   settings written into the test runtime's own `.env`, never the repository's.
//! - DNS: `check#EdgeDns` against fixed records, one host pointing elsewhere, and the XMPP client SRV records.
//! - Test: `test#EdgeSetup`'s round trips on the queue answer for mail in, mail out and webhooks.
//! - The wizard: with the dialer behind the services (the process's one edge link slot holds the edge node's hook, and
//!   `node_role` is a process property, so both are set for the local node for that part), the Discover, Apply and
//!   Test steps and the status render, Discover's choices are kept, Apply applies, and the test passes. Then the XMPP
//!   area: Discover offers it for this node's `xmpp_domain` and adds the domain to the passthrough names, Apply sends
//!   it (the edge node keeps it and has the name), and Test probes the tunnel ports (this node runs no XMPP server, so
//!   those fail, saying so) and finds the name.
//! - Calls: the edge node runs a media node and TURN (the process's one media socket: the local node, started first,
//!   has none), on loopback and advertised as a documentation address. The offer names them and the Hello `calls`;
//!   Discover offers calls, on by default; Apply writes `edge_calls_enabled` and the link-kept row, with lines for the
//!   media address, the TURN secret and the row; Test finds the row live and preferred, `make#IceServers` signs the
//!   edge node's TURN URLs with its secret, and that credential allocates on its TURN with a permission for the IP of
//!   the media address. Calls off keeps the row as draining, which Test does not check, and Status says so.
//! - Phones: the edge node runs a SIP engine (the process's one: the local node has none) on loopback. The offer and
//!   the Hello name it, with another local node active making its copies unread; once that node is revoked, a trunk
//!   set up on the local node replicates and the edge node's engine loads it. Discover shows the trunk with its
//!   numbers and the fallback summary; Apply writes `edge_sip_enabled`, with lines for the sets, the link-kept row, the
//!   realm and the fallbacks; Test finds the row live, the trunk loaded and the fallbacks there. Phones off sends both
//!   sets empty, so the edge node's engine drops the trunk, which Test does not check, and Status says so; on again
//!   brings it back. The DNS check takes the SIP domain's SRV records against fixed ones.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "embedded-postgres",
    feature = "inbound-smtp",
    feature = "channels",
    feature = "rtc",
    feature = "sip"
))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::setup::{LocalChoices, MailChoice, PassChoice};
use moqui_core::edge::{
    Configured, EdgeClientInfo, EdgeHostInfo, EdgeInfo, EdgeLink, EdgeOffer, EdgeSetup, LinkState, LinkStatus,
    PairResult, PairingCode, HOST_PASSTHROUGH, HOST_TERMINATE,
};
use moqui_core::entity::EntityFacade;
use moqui_host::edge_link::{local_web, pair, turn_probe, xmpp_probe, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const NODE: &str = "local-setup";
const OTHER: &str = "other-node";
const BRIDGE: &str = "SETUP_BRIDGE";
/// The address the edge node's media node advertises (documentation range, so Apply finds it public).
const MEDIA_IP: &str = "203.0.113.5";
/// The edge node's SIP realm (its sip_domain).
const SIP_DOMAIN: &str = "sip.edge.example.test";
const SIP_NUMBER: &str = "+15551230100";
const SIP_PLAIN_NUMBER: &str = "+15551230101";

const LOCAL_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>"#;
const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1"/>
    <email-client idle-enabled="false"/>
    <smtp-server internet="true" starttls="false"/>"#;

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

/// Create (or update) a row on `gate`.
fn create(gate: &Gate, entity: &str, fields: &[(&str, &str)]) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = facade(gate);
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in fields {
        row.set(*name, Value::string(*value));
    }
    row.create_or_update(&facade).unwrap();
}

/// A field of the row of `entity` on `gate` with this key.
fn field(gate: &Gate, entity: &str, key: (&str, &str), name: &str) -> String {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    facade(gate)
        .find(entity)
        .unwrap()
        .eq(key.0, Value::string(key.1))
        .use_cache(false)
        .one()
        .unwrap()
        .and_then(|row| row.get(name).map(to_plain_string))
        .unwrap_or_default()
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn list(value: Option<&Value>) -> Vec<BTreeMap<String, String>> {
    let Some(Value::List(items)) = value else {
        panic!("not a list: {value:?}")
    };
    items
        .iter()
        .map(|item| {
            let Value::Map(row) = item else {
                panic!("not a map: {item:?}")
            };
            row.iter()
                .map(|(name, value)| (name.clone(), to_plain_string(value)))
                .collect()
        })
        .collect()
}

/// The edge node's host names, by name: (node, mode).
fn hosts(edge: &Gate) -> BTreeMap<String, (String, String)> {
    list(call(edge, "org.moqui.edge.EdgeServices.get#EdgeHosts", &[]).get("hostList"))
        .into_iter()
        .map(|row| {
            (
                row["hostName"].clone(),
                (row["pairedNodeId"].clone(), row["modeEnumId"].clone()),
            )
        })
        .collect()
}

fn host(name: &str, mode: &str) -> EdgeHostInfo {
    EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: String::new(),
        mode_enum_id: mode.to_string(),
    }
}

/// Whether the edge node has a replicated copy of `address` from the local node.
fn has_recipient(edge: &Gate, address: &str) -> bool {
    let facade = facade(edge);
    let factory = edge.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::edge::replica::list_values(
        &facade,
        Some(NODE),
        "mailRecipients",
        Some("moqui.basic.email.EmailAccount"),
        100,
    )
    .unwrap()
    .iter()
    .any(|value| value.fields["emailAddress"].as_str() == Some(address))
}

/// The local node's link as its services see it, over the test's dialer.
struct DialerLink {
    dialer: Arc<Dialer>,
    runtime: tokio::runtime::Handle,
    factory: Arc<moqui_core::context::ExecutionContextFactory>,
}

fn not_here() -> LibError {
    LibError::new("not a local node's call")
}

impl EdgeLink for DialerLink {
    fn status(&self) -> LinkStatus {
        self.dialer.status()
    }
    fn check(&self, _url: &str) -> Result<EdgeInfo, LibError> {
        Err(not_here())
    }
    fn pair(&self, _url: &str, _code: &str, _expect: Option<&str>) -> Result<PairResult, LibError> {
        Err(not_here())
    }
    fn unpair(&self) -> Result<(), LibError> {
        Err(not_here())
    }
    fn create_pairing_code(&self, _minutes: u32) -> Result<PairingCode, LibError> {
        Err(not_here())
    }
    fn clients(&self) -> Result<Vec<EdgeClientInfo>, LibError> {
        Err(not_here())
    }
    fn revoke(&self, _node: &str) -> Result<bool, LibError> {
        Err(not_here())
    }
    fn ping_client(&self, _node: &str) -> Result<f64, LibError> {
        Err(not_here())
    }
    fn hosts(&self, _node: Option<&str>) -> Result<Vec<EdgeHostInfo>, LibError> {
        Err(not_here())
    }
    fn add_host(&self, _host: &EdgeHostInfo) -> Result<EdgeHostInfo, LibError> {
        Err(not_here())
    }
    fn remove_host(&self, _host_name: &str) -> Result<bool, LibError> {
        Err(not_here())
    }
    fn offer(&self) -> Result<EdgeOffer, LibError> {
        self.runtime.block_on(self.dialer.offer()).map_err(LibError::new)
    }
    fn configure(&self, setup: &EdgeSetup) -> Result<Configured, LibError> {
        self.runtime
            .block_on(self.dialer.configure(setup))
            .map_err(LibError::new)
    }
    fn xmpp_probe(&self, port: u16, direct: bool) -> Result<String, LibError> {
        self.runtime
            .block_on(xmpp_probe(
                Arc::clone(&self.factory),
                self.dialer.status().url,
                port,
                direct,
            ))
            .map_err(LibError::new)
    }
    fn turn_probe(&self, url: &str, username: &str, credential: &str, peer: &str) -> Result<String, LibError> {
        turn_probe(url, username, credential, peer).map_err(LibError::new)
    }
}

/// The process as the local node for the wizard: the dialer behind the services and no `node_role`; put back as the
/// edge node's when dropped, also when the walk panics.
struct LocalRole;

impl LocalRole {
    fn take(dialer: &Arc<Dialer>, runtime: &tokio::runtime::Handle, local: &Gate) -> Self {
        moqui_core::edge::install_edge_link(Arc::new(DialerLink {
            dialer: Arc::clone(dialer),
            runtime: runtime.clone(),
            factory: Arc::clone(local.server.factory()),
        }));
        moqui_lib::env::clear_property("node_role");
        Self
    }
}

impl Drop for LocalRole {
    fn drop(&mut self) {
        moqui_lib::env::set_property("node_role", "edge".to_string());
        moqui_core::edge::clear_edge_link();
    }
}

/// Unpairing's restore of the mail rows Apply changed, on the local node.
fn restore(local: &Gate) {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let lines = moqui_core::edge::setup::restore_local(&facade(local));
    assert!(lines.iter().all(|line| line.ok), "{lines:?}");
}

/// The wizard's steps after Connect and the status, rendered on the local node as john.doe.
fn walk_the_wizard(local: &Gate, sales: &str) {
    let factory = Arc::clone(local.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode("html");
    let show = |path: &str, texts: &[&str]| {
        let page = screens.test_render(path, BTreeMap::new(), None);
        let text = page.text();
        assert!(page.error_messages.is_empty(), "{path}: {:?}", page.error_messages);
        assert!(
            !text.contains("Error rendering") && !text.contains("Exception in render"),
            "{path}: {text}"
        );
        for wanted in texts {
            assert!(text.contains(wanted), "{path} has no {wanted}: {text}");
        }
    };
    let post = |path: &str, fields: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = fields
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let page = screens.test_render(path, parameters, Some("post"));
        page.error_messages.join(" | ")
    };
    // the default form: no localhost (the default webapp_http_host), and the system's own server kept unless chosen
    moqui_lib::env::set_property("webapp_http_host", "localhost".to_string());
    let page = screens.test_render("EdgeNode/Pair/Discover", BTreeMap::new(), None);
    assert!(
        page.error_messages.is_empty(),
        "{:?} {:?}",
        page.error_messages,
        page.status()
    );
    let discover = page.text();
    moqui_lib::env::clear_property("webapp_http_host");
    assert!(!discover.contains("value=\"localhost\""), "{discover}");
    assert!(
        discover.contains("erp.example.com"),
        "the node's names there now: {discover}"
    );
    show(
        "EdgeNode/Pair/Discover",
        &[
            "What this node uses the edge node for",
            "sales@example.com",
            "Texts",
            "so its SIP engine reads no node&#39;s trunks or fallbacks",
            "The system&#39;s mail has a server of its own now",
        ],
    );
    let refused = post(
        "EdgeNode/Pair/Discover/chooseSetup",
        &[
            ("webOn", Value::string("Y")),
            ("passthroughNames", Value::string("localhost")),
        ],
    );
    assert!(refused.contains("not a public host name"), "{refused}");
    let choose = || {
        post(
            "EdgeNode/Pair/Discover/chooseSetup",
            &[
                ("webOn", Value::string("Y")),
                ("passthroughNames", Value::string("erp.example.com")),
                ("mailOn", Value::string("Y")),
                ("mailDomains", Value::string("example.com")),
                ("systemMail", Value::string("Y")),
                ("emailAccountIds", Value::List(vec![Value::string(sales)])),
                ("chatOn", Value::string("Y")),
                ("chatBridgeIds", Value::List(vec![Value::string(BRIDGE)])),
            ],
        )
    };
    assert_eq!(choose(), "");
    show(
        "EdgeNode/Pair/Apply",
        &["What Apply sets up", "DNS records to set", "_dmarc.example.com"],
    );
    assert!(post("EdgeNode/Pair/Apply/confirmApply", &[]).contains("Apply the setup first"));
    assert_eq!(post("EdgeNode/Pair/Apply/applySetup", &[]), "");
    show(
        "EdgeNode/Pair/Apply",
        &["Apply again", "erp.example.com goes to this node"],
    );
    // back to Discover and on again: a choice made again is applied again before it is tested
    assert_eq!(choose(), "");
    assert!(post("EdgeNode/Pair/Apply/confirmApply", &[]).contains("Apply the setup first"));
    assert_eq!(post("EdgeNode/Pair/Apply/applySetup", &[]), "");
    assert_eq!(post("EdgeNode/Pair/Apply/confirmApply", &[]), "");
    show(
        "EdgeNode/Pair/Test",
        &[
            "It works",
            "The edge node takes mail for sales@example.com",
            "DKIM-signed",
        ],
    );
    show(
        "EdgeNode/Status",
        &["Set up again", "erp.example.com (passthrough)", "through the edge node"],
    );
}

/// The XMPP area of the wizard on the local node, for its `xmpp_domain` (set as a property after the servers started,
/// so the local node runs no XMPP server).
fn xmpp_area(local: &Gate, edge: &Gate) {
    moqui_lib::env::set_property("xmpp_domain", "chat.example.com".to_string());
    let factory = Arc::clone(local.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode("html");
    let show = |path: &str, texts: &[&str]| {
        let page = screens.test_render(path, BTreeMap::new(), None);
        let text = page.text();
        assert!(page.error_messages.is_empty(), "{path}: {:?}", page.error_messages);
        for wanted in texts {
            assert!(text.contains(wanted), "{path} has no {wanted}: {text}");
        }
    };
    let post = |path: &str, fields: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = fields
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        screens
            .test_render(path, parameters, Some("post"))
            .error_messages
            .join(" | ")
    };
    show(
        "EdgeNode/Pair/Discover",
        &["XMPP clients of chat.example.com come through", "Tunnel ports"],
    );
    let chosen = post(
        "EdgeNode/Pair/Discover/chooseSetup",
        &[
            ("webOn", Value::string("Y")),
            ("passthroughNames", Value::string("erp.example.com")),
            ("xmppOn", Value::string("Y")),
        ],
    );
    assert_eq!(chosen, "");
    show(
        "EdgeNode/Pair/Apply",
        &[
            "erp.example.com, chat.example.com (passthrough)",
            "_xmpp-client._tcp.chat.example.com",
            "_xmpp-server._tcp.chat.example.com",
        ],
    );
    assert_eq!(post("EdgeNode/Pair/Apply/applySetup", &[]), "");
    show(
        "EdgeNode/Pair/Apply",
        &["XMPP clients of chat.example.com come through the edge node"],
    );
    // the edge node's host table, read as rows: its services' link is the dialer here
    let edge_host = || {
        let key = ("hostName", "chat.example.com");
        (
            field(edge, "moqui.edge.EdgeHost", key, "pairedNodeId"),
            field(edge, "moqui.edge.EdgeHost", key, "modeEnumId"),
        )
    };
    assert_eq!(edge_host(), (NODE.to_string(), HOST_PASSTHROUGH.to_string()));
    let kept = field(edge, "moqui.edge.EdgeClient", ("pairedNodeId", NODE), "setupJson");
    assert!(kept.contains(r#""xmppDomain":"chat.example.com""#), "{kept}");
    // Test: the tunnels are probed, and this node runs no XMPP server, so they fail saying so; the name is there
    let tested = call(
        local,
        "org.moqui.edge.EdgeServices.test#EdgeSetup",
        &[("xmppOn", Value::Bool(true))],
    );
    let tests: BTreeMap<String, BTreeMap<String, String>> = list(tested.get("testList"))
        .into_iter()
        .map(|row| (row["check"].clone(), row))
        .collect();
    for check in ["xmpp", "xmpps"] {
        assert_eq!(tests[check]["ok"], "false", "{tests:?}");
        assert!(tests[check]["message"].contains("does not take"), "{tests:?}");
    }
    assert_eq!(tests["xmppWeb"]["ok"], "true", "{tests:?}");
    show("EdgeNode/Status", &["chat.example.com through ports"]);
    // web access off: the edge node has the name (a trailing dot is the same domain), so Discover and Apply take it
    moqui_lib::env::set_property("xmpp_domain", "Chat.example.com.".to_string());
    assert_eq!(
        post("EdgeNode/Pair/Discover/chooseSetup", &[("xmppOn", Value::string("Y"))]),
        ""
    );
    let xmpp_lines = |out: BTreeMap<String, Value>| -> Vec<BTreeMap<String, String>> {
        list(out.get("lineList"))
            .into_iter()
            .filter(|line| line["area"] == "xmpp")
            .collect()
    };
    let applied = xmpp_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("xmppOn", Value::Bool(true))],
    ));
    assert!(
        !applied.is_empty() && applied.iter().all(|line| line["ok"] == "true"),
        "{applied:?}"
    );
    // and a domain the edge node does not have: Discover refuses, Apply applies the rest with a failed line
    moqui_lib::env::set_property("xmpp_domain", "talk.example.com".to_string());
    let refused = post("EdgeNode/Pair/Discover/chooseSetup", &[("xmppOn", Value::string("Y"))]);
    assert!(refused.contains("turn web access on"), "{refused}");
    let applied = call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("xmppOn", Value::Bool(true))],
    );
    assert_eq!(applied["configured"], Value::Bool(true), "the rest is applied");
    let lines = xmpp_lines(applied);
    assert!(
        lines
            .iter()
            .any(|line| line["ok"] == "false" && line["text"].contains("turn web access on")),
        "{lines:?}"
    );
    // with no xmpp_domain, nothing is changed
    moqui_lib::env::clear_property("xmpp_domain");
    let parameters = BTreeMap::from([("xmppOn".to_string(), Value::Bool(true))]);
    let refused = moqui_core::service::sync("org.moqui.edge.EdgeServices.apply#EdgeSetup", &parameters);
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    assert!(
        refused.is_err() || errors.iter().any(|error| error.contains("no XMPP server")),
        "{refused:?} {errors:?}"
    );
    assert_eq!(edge_host().0, NODE, "nothing changed");
}

/// The calls area of the wizard on the local node: the edge node's media node at `media`, with its TURN.
fn calls_area(local: &Gate, media: &str) {
    let factory = Arc::clone(local.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode("html");
    let show = |path: &str, texts: &[&str]| {
        let page = screens.test_render(path, BTreeMap::new(), None);
        let text = page.text();
        assert!(page.error_messages.is_empty(), "{path}: {:?}", page.error_messages);
        for wanted in texts {
            assert!(text.contains(wanted), "{path} has no {wanted}: {text}");
        }
    };
    let post = |path: &str, fields: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = fields
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        screens
            .test_render(path, parameters, Some("post"))
            .error_messages
            .join(" | ")
    };
    let offered = format!("New calls have their media on the edge node&#39;s media node at {media}");
    show(
        "EdgeNode/Pair/Discover",
        &[&offered, "office users in a call with each other", "Media node at"],
    );
    // a choice the form did not offer is no choice
    let unoffered = post("EdgeNode/Pair/Discover/chooseSetup", &[("callsOn", Value::string("Y"))]);
    assert!(unoffered.contains("at least one area"), "{unoffered}");
    assert_eq!(
        post(
            "EdgeNode/Pair/Discover/chooseSetup",
            &[("callsOffered", Value::string("Y")), ("callsOn", Value::string("Y"))]
        ),
        "",
        "calls alone is a setup"
    );
    show("EdgeNode/Pair/Apply", &["New calls on the edge node&#39;s media node"]);
    assert_eq!(post("EdgeNode/Pair/Apply/applySetup", &[]), "");
    let edge_id = local_edge_id(local);
    let row = |name: &str| field(local, "moqui.call.CallMediaNode", ("instanceId", &edge_id), name);
    assert_eq!((row("edgeLinked"), row("draining")), ("Y".to_string(), "N".to_string()));
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap();
    assert!(env.contains("edge_calls_enabled=true"), "{env}");
    let calls_lines = |out: BTreeMap<String, Value>| -> Vec<BTreeMap<String, String>> {
        list(out.get("lineList"))
            .into_iter()
            .filter(|line| line["area"] == "calls")
            .collect()
    };
    let applied = calls_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("callsOn", Value::Bool(true))],
    ));
    for wanted in [
        "New calls have their media on the edge node's media node",
        &format!("Browsers send call media to {media}"),
        "takes credentials signed with the secret its link handed this node",
        &format!("New calls have their media on the edge node at {media}, before this node's own media nodes"),
    ] {
        assert!(
            applied
                .iter()
                .any(|line| line["ok"] == "true" && line["text"].contains(wanted)),
            "{wanted}: {applied:?}"
        );
    }
    assert!(applied.iter().all(|line| line["ok"] == "true"), "{applied:?}");
    // Test: the row live and preferred, the edge node's TURN URLs signed with its secret, and an allocation on it
    let tests = |out: BTreeMap<String, Value>| -> BTreeMap<String, BTreeMap<String, String>> {
        list(out.get("testList"))
            .into_iter()
            .map(|row| (row["check"].clone(), row))
            .collect()
    };
    let tested = tests(call(
        local,
        "org.moqui.edge.EdgeServices.test#EdgeSetup",
        &[("callsOn", Value::Bool(true))],
    ));
    for check in ["callsNode", "iceServers", "turn"] {
        assert_eq!(tested[check]["ok"], "true", "{check}: {tested:?}");
    }
    assert!(
        tested["turn"]["message"].contains(&format!("permission for its media address's IP {MEDIA_IP}")),
        "{tested:?}"
    );
    // an Apply that was not asked about calls leaves them as they are
    let untouched = calls_lines(call(local, "org.moqui.edge.EdgeServices.apply#EdgeSetup", &[]));
    assert!(untouched.is_empty(), "{untouched:?}");
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap();
    assert!(env.contains("edge_calls_enabled=true"), "{env}");
    assert_eq!(row("draining"), "N");
    show(
        "EdgeNode/Status",
        &[&format!("New calls on the edge node at {media}, with its TURN")],
    );
    // calls off: the row stays, draining, so calls on it go on; this node has no media node of its own, so a line
    // says no new call can start
    let off = calls_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("callsOn", Value::Bool(false))],
    ));
    assert!(
        off.iter()
            .any(|line| line["ok"] == "true" && line["text"].contains("takes no new calls; calls on it go on")),
        "{off:?}"
    );
    let failed: Vec<&BTreeMap<String, String>> = off.iter().filter(|line| line["ok"] == "false").collect();
    assert!(
        failed.len() == 1 && failed[0]["text"].contains("no new call can start"),
        "{off:?}"
    );
    assert_eq!((row("edgeLinked"), row("draining")), ("Y".to_string(), "Y".to_string()));
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap();
    assert!(env.contains("edge_calls_enabled=false"), "{env}");
    let tested = tests(call(
        local,
        "org.moqui.edge.EdgeServices.test#EdgeSetup",
        &[("callsOn", Value::Bool(true))],
    ));
    assert_eq!(tested["callsNode"]["checked"], "false", "{tested:?}");
    assert!(!tested.contains_key("turn"), "{tested:?}");
    show(
        "EdgeNode/Status",
        &["Off, and this node has no live media node of its own, so no new call can start"],
    );
    // after unpairing has forgotten the rows, an Apply in flight does not make the row again
    moqui_core::edge::calls::forget_rows(&factory, || ());
    let late = calls_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("callsOn", Value::Bool(true))],
    ));
    assert!(
        late.iter()
            .any(|line| line["ok"] == "false" && line["text"].contains("this node unpaired")),
        "{late:?}"
    );
    assert_eq!(row("draining"), "Y", "the row is not written");
    // and on again, as the wizard leaves it, once the link allows the row again
    moqui_core::edge::calls::allow_row(&factory, &edge_id);
    let on = calls_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("callsOn", Value::Bool(true))],
    ));
    assert!(on.iter().all(|line| line["ok"] == "true"), "{on:?}");
    assert_eq!(row("draining"), "N");
}

/// The edge node's copy of the local node's set `set`: how many records.
fn copies(edge: &Gate, set: &str) -> usize {
    let factory = edge.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::edge::replica::list_values(&facade(edge), Some(NODE), set, None, 100)
        .unwrap()
        .len()
}

/// The phones area of the wizard on the local node, with the edge node's SIP engine at `sip_address`.
fn sip_area(local: &Gate, edge: &Gate, sip_address: &str) {
    let factory = Arc::clone(local.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let screens = moqui_core::screen::ScreenTest::new(&factory)
        .root_screen("component://tools/screen/System.xml")
        .render_mode("html");
    let show = |path: &str, texts: &[&str]| {
        let page = screens.test_render(path, BTreeMap::new(), None);
        let text = page.text();
        assert!(page.error_messages.is_empty(), "{path}: {:?}", page.error_messages);
        for wanted in texts {
            assert!(text.contains(wanted), "{path} has no {wanted}: {text}");
        }
    };
    let post = |path: &str, fields: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = fields
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        screens
            .test_render(path, parameters, Some("post"))
            .error_messages
            .join(" | ")
    };
    let link = moqui_core::edge::link().unwrap();
    let offered_trunks = || link.offer().unwrap().sip.trunks;
    // the other local node gone, the edge node's engine reads this node's copies; then a trunk with two numbers, one
    // with a fallback (john's: a message)
    create(
        edge,
        "moqui.edge.EdgeClient",
        &[("pairedNodeId", OTHER), ("statusId", "EdgeClientRevoked")],
    );
    assert!(!link.offer().unwrap().sip.other_nodes);
    // a fake trunk on loopback that answers the engine's OPTIONS pings, until the walk ends
    let fake = crate::sip::Trunk::new();
    let trunk_port = fake.addr().port();
    let answering = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pings = {
        let answering = Arc::clone(&answering);
        std::thread::spawn(move || {
            while answering.load(std::sync::atomic::Ordering::Acquire) {
                let until = Instant::now() + Duration::from_millis(200);
                if let Some((rsipstack::sip::SipMessage::Request(req), from)) = fake.recv(until) {
                    if req.method.to_string() == "OPTIONS" {
                        fake.reply(&req, from, 200, "", "");
                    }
                }
            }
        })
    };
    let trunk_id = to_plain_string(
        &call(
            local,
            "org.moqui.call.VoiceServices.store#VoiceTrunk",
            &[
                ("name", Value::string("Setup trunk")),
                ("host", Value::string("127.0.0.1")),
                ("port", Value::I32(i32::from(trunk_port))),
                ("transportEnumId", Value::string("VtrUdp")),
                ("mediaEncryptionEnumId", Value::string("VmePlain")),
                ("providerProfileEnumId", Value::string("VppPlainLan")),
                ("username", Value::string("acct")),
                ("password", Value::string("pw-of-acct")),
                ("registerFlag", Value::string("N")),
            ],
        )["voiceTrunkId"],
    );
    let john = moqui_core::user::current_user_id().unwrap().expect("john is signed in");
    for (number, route) in [(SIP_NUMBER, Some(john.as_str())), (SIP_PLAIN_NUMBER, None)] {
        let mut fields = vec![
            ("phoneNumber", Value::string(number)),
            ("voiceTrunkId", Value::string(&trunk_id)),
        ];
        if let Some(user) = route {
            fields.push(("routeTypeEnumId", Value::string("PnrUser")));
            fields.push(("routeId", Value::string(user)));
        }
        call(local, "org.moqui.call.VoiceServices.store#PhoneNumber", &fields);
    }
    call(local, "org.moqui.call.VoiceServices.refresh#PhoneFallbacks", &[]);
    wait_until(
        Duration::from_secs(10),
        "the edge node's engine loads the trunk",
        || offered_trunks().iter().any(|trunk| trunk.voice_trunk_id == trunk_id),
    );
    let loaded = offered_trunks();
    assert_eq!(loaded[0].name, "Setup trunk", "{loaded:?}");
    wait_until(Duration::from_secs(10), "the trunk answers the engine's ping", || {
        offered_trunks()
            .iter()
            .any(|trunk| trunk.voice_trunk_id == trunk_id && trunk.state == "ok")
    });
    show(
        "EdgeNode/Pair/Discover",
        &[
            &format!("This node&#39;s trunks and phones go through the edge node&#39;s SIP engine at {sip_address} (UDP/TCP)"),
            "Phones through the edge node",
            "Setup trunk",
            &format!("{SIP_NUMBER}, {SIP_PLAIN_NUMBER}"),
            "1 of 2 numbers have a fallback",
            &format!("On, realm {SIP_DOMAIN}"),
            "they cannot register while this node is away",
            "following no call-handling rules or schedules",
        ],
    );
    let unoffered = post("EdgeNode/Pair/Discover/chooseSetup", &[("sipOn", Value::string("Y"))]);
    assert!(
        unoffered.contains("at least one area"),
        "a choice the form did not offer: {unoffered}"
    );
    assert_eq!(
        post(
            "EdgeNode/Pair/Discover/chooseSetup",
            &[("sipOffered", Value::string("Y")), ("sipOn", Value::string("Y"))]
        ),
        "",
        "phones alone is a setup"
    );
    show(
        "EdgeNode/Pair/Apply",
        &[
            "This node&#39;s trunks and phones through the edge node&#39;s SIP engine",
            &format!("_sip._udp.{SIP_DOMAIN}"),
        ],
    );
    assert_eq!(post("EdgeNode/Pair/Apply/applySetup", &[]), "");
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap();
    assert!(env.contains("edge_sip_enabled=true"), "{env}");
    let sip_lines = |out: BTreeMap<String, Value>| -> Vec<BTreeMap<String, String>> {
        list(out.get("lineList"))
            .into_iter()
            .filter(|line| line["area"] == "sip")
            .collect()
    };
    let applied = sip_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("sipOn", Value::Bool(true))],
    ));
    for wanted in [
        "This node's trunks and phones go through the edge node's SIP engine",
        "The set voiceTrunks goes to the edge node whole again (cursor",
        "The set voiceFallbacks goes to the edge node whole again",
        &format!("go to the edge node's SIP engine at {sip_address}"),
        &format!("Phones made here get verifiers for the edge node's realm {SIP_DOMAIN}"),
        "1 of 2 numbers have a fallback for while this node is away; calls to the others are refused (480)",
    ] {
        assert!(
            applied
                .iter()
                .any(|line| line["ok"] == "true" && line["text"].contains(wanted)),
            "{wanted}: {applied:?}"
        );
    }
    assert!(applied.iter().all(|line| line["ok"] == "true"), "{applied:?}");
    // Test: the row live, the trunk loaded, the fallback there once the full sends Apply asked for are done
    let tests = || -> BTreeMap<String, BTreeMap<String, String>> {
        list(
            call(
                local,
                "org.moqui.edge.EdgeServices.test#EdgeSetup",
                &[("sipOn", Value::Bool(true))],
            )
            .get("testList"),
        )
        .into_iter()
        .map(|row| (row["check"].clone(), row))
        .collect()
    };
    wait_until(Duration::from_secs(10), "the fallbacks replicated", || {
        tests()["sipFallbacks"]["ok"] == "true"
    });
    let tested = tests();
    for check in ["sipNode", "sipTrunks", "sipFallbacks"] {
        assert_eq!(tested[check]["ok"], "true", "{check}: {tested:?}");
    }
    assert!(
        tested["sipTrunks"]["message"].contains("Setup trunk (ok)"),
        "a reload (Apply's full sends) keeps the trunk's state: {tested:?}"
    );
    assert!(
        tested["sipFallbacks"]["message"].contains("each of the 1 numbers"),
        "{tested:?}"
    );
    // phone calls go to the edge node's SIP engine (Voice.js voiceNodes): the only voice node here
    let active = || call(local, "org.moqui.call.VoiceServices.get#VoiceCapabilities", &[]);
    assert_eq!(active()["active"], Value::Bool(true), "{:?}", active());
    show(
        "EdgeNode/Status",
        &[
            &format!("Trunks and phones through the edge node at {sip_address}, registrar on"),
            "1 trunks loaded there, 1 numbers with a fallback",
        ],
    );
    // an Apply not asked about phones leaves them as they are
    let untouched = sip_lines(call(local, "org.moqui.edge.EdgeServices.apply#EdgeSetup", &[]));
    assert!(untouched.is_empty(), "{untouched:?}");
    // phones off: both sets go empty, so the edge node's engine has no trunk; the row stays, taking no new call
    let off = sip_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("sipOn", Value::Bool(false))],
    ));
    for wanted in [
        "The set voiceTrunks goes to the edge node empty",
        "The set voiceFallbacks goes to the edge node empty",
        "stays but takes no new call",
        "No phone is registered at the edge node's registrar",
    ] {
        assert!(
            off.iter()
                .any(|line| line["ok"] == "true" && line["text"].contains(wanted)),
            "{wanted}: {off:?}"
        );
    }
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap();
    assert!(env.contains("edge_sip_enabled=false"), "{env}");
    wait_until(
        Duration::from_secs(10),
        "the edge node's copies emptied and its engine without the trunk",
        || copies(edge, "voiceTrunks") == 0 && copies(edge, "voiceFallbacks") == 0 && offered_trunks().is_empty(),
    );
    let edge_id = local_edge_id(local);
    assert_eq!(
        field(local, "moqui.call.SipNode", ("instanceId", &edge_id), "edgeLinked"),
        "Y",
        "the row stays"
    );
    assert_eq!(tests()["sipNode"]["checked"], "false");
    let off = active();
    assert_eq!(
        off["active"],
        Value::Bool(false),
        "no new call goes to the edge node: {off:?}"
    );
    assert_eq!(to_plain_string(&off["reasonCode"]), "noSipServer", "{off:?}");
    show("EdgeNode/Status", &["Off: the edge node gets no trunks or fallbacks"]);
    // and on again, as the wizard leaves it
    let on = sip_lines(call(
        local,
        "org.moqui.edge.EdgeServices.apply#EdgeSetup",
        &[("sipOn", Value::Bool(true))],
    ));
    assert!(on.iter().all(|line| line["ok"] == "true"), "{on:?}");
    wait_until(Duration::from_secs(10), "the trunk loaded again", || {
        offered_trunks().iter().any(|trunk| trunk.voice_trunk_id == trunk_id) && copies(edge, "voiceFallbacks") == 1
    });
    assert_eq!(active()["active"], Value::Bool(true));
    answering.store(false, std::sync::atomic::Ordering::Release);
    pings.join().unwrap();
}

/// The edge node's instance id, as the local node pinned it from its Hello.
fn local_edge_id(local: &Gate) -> String {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::edge::link().unwrap().status().peer.instance_id
}

#[test]
fn a_local_node_is_attached_from_the_wizard() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let repo_env = crate::common::repo_runtime().join(".env");
    let repo_env_before = std::fs::read(&repo_env).ok();
    let (root, cert, key) = certs();
    let local = crate::common::start_with_conf_files(
        "edge-setup-local",
        None,
        LOCAL_CONF,
        &[("mail_dns_offline", "true")],
        &[],
    );
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
    create(
        &local,
        "moqui.chat.ChatBridge",
        &[
            ("chatBridgeId", BRIDGE),
            ("name", "Texts"),
            ("typeEnumId", "CbtSms"),
            ("adapterName", "sms"),
        ],
    );

    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let (xmpp_port, xmpp_tls_port) = (crate::common::free_port(), crate::common::free_port());
    // the edge node's media node on loopback, advertised as a documentation address, and its TURN on loopback
    let (media_port, turn_port) = (crate::common::free_port(), crate::common::free_port());
    let media = format!("{MEDIA_IP}:{media_port}");
    let edge = crate::common::start_second_with(
        "edge-setup-edge",
        EDGE_CONF,
        &[
            ("node_role", "edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("smtp_listen_port", "0"),
            ("smtp_hostname", "edge.example.net"),
            ("edge_mail_mta", "moqui"),
            ("edge_mail_bounce_domain", "bounce.example.net"),
            ("channels_webhooks_enabled", "true"),
            ("edge_public_addresses", "203.0.113.5, not-an-address"),
            ("edge_xmpp_port", &xmpp_port.to_string()),
            ("edge_xmpp_tls_port", &xmpp_tls_port.to_string()),
            ("edge_xmpp_host", "127.0.0.1"),
            ("rtc_media_port", &media_port.to_string()),
            ("rtc_media_host", "127.0.0.1"),
            ("rtc_public_address", &media),
            ("turn_port", &turn_port.to_string()),
            ("turn_host", "127.0.0.1"),
            ("turn_public_host", "127.0.0.1"),
            ("turn_relay_ports", "51310-51400"),
            ("turn_auth_secret", "edge-setup-turn-secret"),
            // its SIP engine, on loopback (the local node, started first, runs none)
            ("sip_port", "0"),
            ("sip_rtp_ports", "42700-42799"),
            ("sip_registrar_enabled", "true"),
            ("sip_domain", SIP_DOMAIN),
        ],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    // another local node, with a name of its own, and a name of this node an earlier setup made
    create(
        &edge,
        "moqui.edge.EdgeClient",
        &[
            ("pairedNodeId", OTHER),
            ("keyFingerprint", &"0".repeat(64)),
            ("statusId", "EdgeClientActive"),
        ],
    );
    call(
        &edge,
        "org.moqui.edge.EdgeServices.add#EdgeHost",
        &[
            ("hostName", Value::string("taken.example.com")),
            ("pairedNodeId", Value::string(OTHER)),
        ],
    );

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
    assert!(
        paired.edge.capabilities.iter().any(|area| area == "mail")
            && paired.edge.capabilities.iter().any(|area| area == "chat")
            && paired.edge.capabilities.iter().any(|area| area == "xmpp")
            && paired.edge.capabilities.iter().any(|area| area == "calls")
            && paired.edge.capabilities.iter().any(|area| area == "sip"),
        "the Hello names the areas: {:?}",
        paired.edge.capabilities
    );
    call(
        &edge,
        "org.moqui.edge.EdgeServices.add#EdgeHost",
        &[
            ("hostName", Value::string("stale.example.com")),
            ("pairedNodeId", Value::string(NODE)),
        ],
    );
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(5);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Arc::new(Dialer::start(config, rt.handle(), shutdown, None).unwrap());
    wait_until(Duration::from_secs(10), "the link up", || {
        dialer.status().state == LinkState::Linked
    });

    // Discover: the offer
    let offer = rt.block_on(dialer.offer()).unwrap();
    assert!(offer.mail.available && offer.mail.listener, "{offer:?}");
    assert_eq!(offer.mail.mta, "moqui");
    assert_eq!(offer.mail.mx_host, "edge.example.net");
    assert_eq!(offer.mail.bounce_domain, "bounce.example.net");
    assert!(offer.mail.dkim_record.starts_with("v=DKIM1"), "{offer:?}");
    assert!(offer.chat.webhooks);
    assert!(!offer.pass.available, "no mr-pass in the test runtime");
    assert_eq!(offer.pass.client_id, "edge");
    assert!(offer.not_available.is_empty(), "{:?}", offer.not_available);
    let sip_address = offer.sip.sip_address.clone();
    assert!(
        offer.sip.available
            && sip_address.starts_with("127.0.0.1:")
            && offer.sip.tls_address.is_empty()
            && offer.sip.domain == SIP_DOMAIN
            && offer.sip.registrar,
        "the SIP engine: {:?}",
        offer.sip
    );
    assert!(
        offer.sip.other_nodes && offer.sip.trunks.is_empty(),
        "another local node active, so the engine reads no copies: {:?}",
        offer.sip
    );
    let sip_port: u16 = sip_address.rsplit_once(':').unwrap().1.parse().unwrap();
    assert!(
        offer.calls.available && offer.calls.media_address == media && !offer.calls.other_nodes,
        "the media node: {:?}",
        offer.calls
    );
    assert_eq!(
        offer.calls.turn_urls.first().map(String::as_str),
        Some(format!("turn:127.0.0.1:{turn_port}?transport=udp").as_str()),
        "{:?}",
        offer.calls
    );
    assert!(
        offer.xmpp.available
            && offer.xmpp.port == xmpp_port
            && offer.xmpp.tls_port == xmpp_tls_port
            && !offer.xmpp.other_node
            && !offer.xmpp.needs_node,
        "the tunnel ports: {:?}",
        offer.xmpp
    );
    assert_eq!(offer.addresses, ["203.0.113.5"], "a bad address is left out");
    assert_eq!(
        offer.node_hosts.len(),
        1,
        "this node's names only: {:?}",
        offer.node_hosts
    );
    assert_eq!(offer.host_modes, [HOST_PASSTHROUGH, HOST_TERMINATE]);

    // Configure: this node's names, one held by another node, the stale one removed
    let setup = EdgeSetup {
        hosts: vec![
            host("ERP.example.com", HOST_TERMINATE),
            host("taken.example.com", HOST_PASSTHROUGH),
        ],
        set_hosts: true,
        mail_domains: vec!["example.com".to_string()],
        chat_bridge_ids: vec![BRIDGE.to_string()],
        pass_sites: vec!["SITE1".to_string()],
        xmpp_domain: "Chat.Example.com".to_string(),
    };
    let configured = rt.block_on(dialer.configure(&setup)).unwrap();
    let errors: BTreeMap<&str, &str> = configured
        .hosts
        .iter()
        .map(|host| (host.host_name.as_str(), host.error.as_str()))
        .collect();
    assert_eq!(errors["erp.example.com"], "");
    assert_eq!(
        errors["taken.example.com"], "another local node has this name",
        "the other node is not named"
    );
    let names = hosts(&edge);
    assert_eq!(names["erp.example.com"], (NODE.to_string(), HOST_TERMINATE.to_string()));
    assert_eq!(
        names["taken.example.com"].0, OTHER,
        "another node's name is never moved"
    );
    assert!(
        !names.contains_key("stale.example.com"),
        "a name of this node not listed is removed"
    );
    assert_eq!(configured.offer.node_hosts.len(), 1);
    let kept = field(&edge, "moqui.edge.EdgeClient", ("pairedNodeId", NODE), "setupJson");
    assert!(
        kept.contains("example.com") && kept.contains(BRIDGE) && kept.contains("SITE1"),
        "{kept}"
    );
    assert!(kept.contains(r#""xmppDomain":"chat.example.com""#), "{kept}");
    let clients = list(call(&edge, "org.moqui.edge.EdgeServices.get#EdgeClients", &[]).get("clientList"));
    let ours = clients.iter().find(|row| row["pairedNodeId"] == NODE).unwrap();
    assert!(
        ours["setup"].contains("SITE1"),
        "Edge Clients shows what Configure set: {ours:?}"
    );
    let refused = rt.block_on(dialer.configure(&EdgeSetup {
        hosts: vec![host("other.example.com", HOST_PASSTHROUGH)],
        set_hosts: true,
        mail_domains: vec!["not a domain".to_string()],
        ..EdgeSetup::default()
    }));
    assert!(refused.unwrap_err().contains("not a mail domain"));
    let refused = rt.block_on(dialer.configure(&EdgeSetup {
        xmpp_domain: "localhost".to_string(),
        ..EdgeSetup::default()
    }));
    assert!(refused.unwrap_err().contains("not an XMPP domain"));
    assert_eq!(
        hosts(&edge).get("erp.example.com").map(|(node, _)| node.as_str()),
        Some(NODE),
        "nothing changed"
    );
    // the edge node's own names, their subdomains, and names that are no public host are refused, one by one
    let own = rt
        .block_on(
            dialer.configure(&EdgeSetup {
                hosts: [
                    "erp.example.com",
                    "edge.example.net",
                    "admin.edge.example.net",
                    "bounce.example.net",
                    "localhost",
                    "10.0.0.1",
                    "mail",
                ]
                .iter()
                .map(|name| host(name, HOST_TERMINATE))
                .collect(),
                set_hosts: true,
                ..EdgeSetup::default()
            }),
        )
        .unwrap();
    let errors: BTreeMap<&str, &str> = own
        .hosts
        .iter()
        .map(|host| (host.host_name.as_str(), host.error.as_str()))
        .collect();
    assert_eq!(errors["erp.example.com"], "", "{errors:?}");
    for name in ["edge.example.net", "admin.edge.example.net", "bounce.example.net"] {
        assert_eq!(errors[name], "the edge node answers for this name itself", "{errors:?}");
    }
    assert!(
        errors.len() == 7 && errors.values().filter(|error| error.is_empty()).count() == 1,
        "{errors:?}"
    );
    let names = hosts(&edge);
    assert!(
        !names.contains_key("edge.example.net") && !names.contains_key("localhost") && !names.contains_key("10.0.0.1"),
        "{names:?}"
    );

    // Apply: the local rows and settings; the system's mail has a server of its own, which unpairing puts back
    create(
        &local,
        "moqui.basic.email.EmailServer",
        &[("emailServerId", "SYSTEM"), ("smtpHost", "smtp.example.com")],
    );
    let env_file = local.runtime.join(".env");
    assert_ne!(env_file, repo_env);
    let choices = LocalChoices {
        edge_url: url.clone(),
        mail: Some(MailChoice {
            system: true,
            account_ids: vec![sales.clone()],
        }),
        chat: Some(vec![BRIDGE.to_string()]),
        pass: Some(PassChoice {
            sites: vec!["SITE1".to_string()],
            callback: "https://forum.example.com/account/Sso".to_string(),
        }),
    };
    let lines = {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::edge::setup::apply_local(&facade(&local), &env_file, &choices, &offer)
    };
    assert!(lines.iter().all(|line| line.ok), "{lines:?}");
    let server = |id: &str| {
        field(
            &local,
            "moqui.basic.email.EmailServer",
            ("emailServerId", id),
            "smtpHost",
        )
    };
    assert_eq!(server("EDGE"), "edge:");
    assert_eq!(server("SYSTEM"), "edge:");
    assert_eq!(
        field(
            &local,
            "moqui.basic.email.EmailAccount",
            ("emailAccountId", &sales),
            "emailServerId"
        ),
        "EDGE"
    );
    let env = std::fs::read_to_string(&env_file).unwrap();
    assert!(env.contains(&format!("channels_public_url={url}")), "{env}");
    assert!(env.contains("pass_edge_client=edge"), "{env}");
    assert!(env.contains("pass_client_edge_sites=SITE1"), "{env}");
    assert!(
        env.contains("pass_client_edge_callback=https://forum.example.com/account/Sso"),
        "{env}"
    );
    assert_eq!(
        std::fs::read(&repo_env).ok(),
        repo_env_before,
        "the repository's runtime/.env is not touched"
    );

    // the DNS records, fixed: one host name still points elsewhere
    let records = moqui_mail::dns::records();
    records.fix_mx("example.com", &[(10, "edge.example.net")]);
    records.fix_txt("example.com", "v=spf1 mx -all").unwrap();
    records
        .fix_txt("moqui._domainkey.example.com", &offer.mail.dkim_record)
        .unwrap();
    records.fix_txt("_dmarc.example.com", "v=DMARC1; p=quarantine").unwrap();
    records.fix_ips("erp.example.com", &["203.0.113.5".parse().unwrap()]);
    records.fix_ips("old.example.com", &["198.51.100.7".parse().unwrap()]);
    records.fix_ips("turn.example.net", &[MEDIA_IP.parse().unwrap()]);
    let srv = |port: u16, target: &str| moqui_mail::dns::Srv {
        priority: 0,
        weight: 5,
        port,
        target: target.to_string(),
    };
    records.fix_srv(
        "_xmpp-client._tcp.chat.example.com",
        &[srv(xmpp_port, "edge.example.net")],
    );
    records.fix_srv(
        "_xmpps-client._tcp.chat.example.com",
        &[srv(xmpp_tls_port, "office.example.com")],
    );
    records.fix_srv(&format!("_sip._udp.{SIP_DOMAIN}"), &[srv(sip_port, "edge.example.net")]);
    let dns = call(
        &local,
        "org.moqui.edge.EdgeServices.check#EdgeDns",
        &[
            ("domains", Value::List(vec![Value::string("example.com")])),
            (
                "hostNames",
                Value::List(vec![Value::string("erp.example.com"), Value::string("old.example.com")]),
            ),
            ("mxHost", Value::string(&offer.mail.mx_host)),
            ("edgeAddresses", Value::List(vec![Value::string("203.0.113.5")])),
            ("dkimSelector", Value::string(&offer.mail.dkim_selector)),
            ("dkimRecord", Value::string(&offer.mail.dkim_record)),
            ("mailMta", Value::string(&offer.mail.mta)),
            ("xmppDomain", Value::string("chat.example.com")),
            ("xmppHosts", Value::List(vec![Value::string("edge.example.net")])),
            ("xmppPort", Value::I64(i64::from(offer.xmpp.port))),
            ("xmppTlsPort", Value::I64(i64::from(offer.xmpp.tls_port))),
            (
                "turnHosts",
                Value::List(vec![Value::string("turn.example.net"), Value::string("127.0.0.1")]),
            ),
            ("mediaAddress", Value::string(&offer.calls.media_address)),
            ("sipDomain", Value::string(&offer.sip.domain)),
            ("sipHosts", Value::List(vec![Value::string("edge.example.net")])),
            ("sipPort", Value::I64(i64::from(sip_port))),
            ("sipTlsPort", Value::I64(0)),
        ],
    );
    let rows = list(dns.get("recordList"));
    let ok = |kind: &str, name: &str| {
        rows.iter()
            .find(|row| row["kind"] == kind && row["name"] == name)
            .unwrap_or_else(|| panic!("{kind} {name}: {rows:?}"))["ok"]
            == "true"
    };
    assert!(ok("MX", "example.com"), "{rows:?}");
    assert!(ok("SPF", "example.com"), "{rows:?}");
    assert!(ok("DKIM", "moqui._domainkey.example.com"), "{rows:?}");
    assert!(ok("DMARC", "_dmarc.example.com"), "{rows:?}");
    assert!(ok("A/AAAA", "erp.example.com"));
    assert!(!ok("A/AAAA", "old.example.com"));
    assert!(ok("A/AAAA", "turn.example.net"), "the TURN host: {rows:?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.get("name").map(String::as_str) == Some("127.0.0.1")),
        "a TURN address needs no record: {rows:?}"
    );
    assert!(ok("SRV", "_xmpp-client._tcp.chat.example.com"), "{rows:?}");
    assert!(
        !ok("SRV", "_xmpps-client._tcp.chat.example.com"),
        "a record past the edge node"
    );
    assert!(ok("SRV", &format!("_sip._udp.{SIP_DOMAIN}")), "{rows:?}");
    assert!(!ok("SRV", &format!("_sip._tcp.{SIP_DOMAIN}")), "no record yet");
    assert!(
        !rows.iter().any(|row| row["name"].starts_with("_sips.")),
        "no TLS listener, so no _sips._tcp: {rows:?}"
    );
    assert_eq!(dns["passed"], Value::Bool(false));

    // Test: round trips on the queue, once the edge node has the copies they check
    wait_until(Duration::from_secs(10), "the account replicated", || {
        has_recipient(&edge, "sales@example.com")
    });
    wait_until(Duration::from_secs(10), "the bridge replicated", || {
        let facade = facade(&edge);
        moqui_core::edge::replica::list_values(&facade, Some(NODE), "chatBridges", None, 10)
            .unwrap()
            .iter()
            .any(|value| value.pk["chatBridgeId"].as_str() == Some(BRIDGE))
    });
    let tested = call(
        &local,
        "org.moqui.edge.EdgeServices.test#EdgeSetup",
        &[
            ("mailOn", Value::Bool(true)),
            ("mailAddress", Value::string("Sales@example.com")),
            ("chatOn", Value::Bool(true)),
            ("chatBridgeIds", Value::List(vec![Value::string(BRIDGE)])),
        ],
    );
    let tests: BTreeMap<String, BTreeMap<String, String>> = list(tested.get("testList"))
        .into_iter()
        .map(|row| (row["check"].clone(), row))
        .collect();
    for check in ["mailIn", "mailOut", "webhook"] {
        assert_eq!(tests[check]["ok"], "true", "{check}: {tests:?}");
    }
    let unknown = call(
        &local,
        "org.moqui.edge.EdgeServices.test#EdgeSetup",
        &[
            ("mailOn", Value::Bool(true)),
            ("mailAddress", Value::string("nobody@example.com")),
        ],
    );
    let unknown = list(unknown.get("testList"));
    let mail_in = unknown.iter().find(|row| row["check"] == "mailIn").unwrap();
    assert_eq!(mail_in["ok"], "false", "{mail_in:?}");
    assert!(mail_in["message"].contains("nobody@example.com"), "{mail_in:?}");

    // unpairing puts the system's server and the account's back
    restore(&local);
    let server = |id: &str| {
        field(
            &local,
            "moqui.basic.email.EmailServer",
            ("emailServerId", id),
            "smtpHost",
        )
    };
    assert_eq!(server("SYSTEM"), "smtp.example.com");
    assert_eq!(
        field(
            &local,
            "moqui.basic.email.EmailAccount",
            ("emailAccountId", &sales),
            "emailServerId"
        ),
        ""
    );

    // the wizard, with the dialer behind the local node's services and this process's role the local node's
    {
        let _role = LocalRole::take(&dialer, rt.handle(), &local);
        walk_the_wizard(&local, &sales);
        xmpp_area(&local, &edge);
        calls_area(&local, &media);
        sip_area(&local, &edge, &sip_address);
    }
    assert_eq!(server("SYSTEM"), "edge:", "the wizard's Apply");
    restore(&local);
    assert_eq!(
        server("SYSTEM"),
        "smtp.example.com",
        "unpairing after the wizard's Apply"
    );
    {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let facade = facade(&local);
        assert!(moqui_core::edge::setup::is_paired(&facade), "the dialer replicates");
        moqui_core::edge::replica::disable(&facade);
        assert!(
            !moqui_core::edge::setup::is_paired(&facade),
            "mail for edge: is not queued after unpairing"
        );
    }
    assert_eq!(
        field(
            &edge,
            "moqui.edge.EdgeHost",
            ("hostName", "erp.example.com"),
            "modeEnumId"
        ),
        HOST_PASSTHROUGH,
        "Apply sent the wizard's names"
    );
    dialer.stop();
    eprintln!("edge_setup took {:?}", started.elapsed());
}
