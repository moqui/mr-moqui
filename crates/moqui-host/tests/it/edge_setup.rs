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
//!   public addresses, and calls, SIP and XMPP as not available.
//! - Configure: this node's host names are applied on the edge node, a name another node holds is refused, a name of
//!   this node not listed is removed, and a request that is not input is refused whole.
//! - Apply: the EDGE mail server with `edge:`, SYSTEM's and a hosted account's mail through it, and the chat and PASS
//!   settings written into the test runtime's own `.env`, never the repository's.
//! - DNS: `check#EdgeDns` against fixed records, one host pointing elsewhere.
//! - Test: `test#EdgeSetup`'s round trips on the queue answer for mail in, mail out and webhooks.
//! - The wizard: with the dialer behind the services (the process's one edge link slot holds the edge node's hook, and
//!   `node_role` is a process property, so both are set for the local node for that part), the Discover, Apply and
//!   Test steps and the status render, Discover's choices are kept, Apply applies, and the test passes.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "embedded-postgres",
    feature = "inbound-smtp",
    feature = "channels"
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
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const NODE: &str = "local-setup";
const OTHER: &str = "other-node";
const BRIDGE: &str = "SETUP_BRIDGE";

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
    list(call(edge, "org.moqui.impl.EdgeServices.get#EdgeHosts", &[]).get("hostList"))
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
}

/// The process as the local node for the wizard: the dialer behind the services and no `node_role`; put back as the
/// edge node's when dropped, also when the walk panics.
struct LocalRole;

impl LocalRole {
    fn take(dialer: &Arc<Dialer>, runtime: &tokio::runtime::Handle) -> Self {
        moqui_core::edge::install_edge_link(Arc::new(DialerLink {
            dialer: Arc::clone(dialer),
            runtime: runtime.clone(),
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
            "Not available on edge nodes yet",
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
            "org.moqui.impl.EmailClientServices.create#EmailAccount",
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
        "org.moqui.impl.EdgeServices.add#EdgeHost",
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
    let code = to_plain_string(&call(&edge, "org.moqui.impl.EdgeServices.create#PairingCode", &[])["code"]);
    let mut pairing = PairConfig::new(&url, &code, NODE);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    assert!(
        paired.edge.capabilities.iter().any(|area| area == "mail")
            && paired.edge.capabilities.iter().any(|area| area == "chat"),
        "the Hello names the areas: {:?}",
        paired.edge.capabilities
    );
    call(
        &edge,
        "org.moqui.impl.EdgeServices.add#EdgeHost",
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
    assert_eq!(offer.not_available, ["calls", "sip", "xmpp"]);
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
    let clients = list(call(&edge, "org.moqui.impl.EdgeServices.get#EdgeClients", &[]).get("clientList"));
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
    let dns = call(
        &local,
        "org.moqui.impl.EdgeServices.check#EdgeDns",
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
        "org.moqui.impl.EdgeServices.test#EdgeSetup",
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
        "org.moqui.impl.EdgeServices.test#EdgeSetup",
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
        let _role = LocalRole::take(&dialer, rt.handle());
        walk_the_wizard(&local, &sales);
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
