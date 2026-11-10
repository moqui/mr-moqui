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

//! The edge link as `moqui_core::edge` sees it ([`EdgeLink`]), installed when the HTTP listeners start: the local
//! node's dialer (started when `edge_node_url` is set, and by pairing without a restart) and, on an edge node, its
//! [`EdgeServer`] with its web front, SSH tunnel port and delegated calls; on both, the queue's metrics and retention.
//! An all-in-one node (`node_role=all-in-one`, §10) starts what an edge node does and never dials; its own web, mail
//! and XMPP stay a normal node's, and only its paired node's `EdgeHost` names go down the link.
//! Pairing writes `edge_node_url`, `edge_node_key` and `edge_node_pin` to `runtime/.env` and sets them as properties;
//! unpairing removes them (plans/Edge-Node.md §5.1, §5.2, §5.3, §5.7).

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{
    Configured, DelegateAnswer, DelegateCall, EdgeClientInfo, EdgeHostInfo, EdgeInfo, EdgeLink, EdgeOffer, EdgeSetup,
    LinkStatus, PairResult, PairingCode, NODE_ID, NODE_KEY, NODE_PIN, NODE_URL,
};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::sync::watch;

use super::accept::{Acceptor, EdgeServer};
use super::dial::{is_linked, Dialer};
use super::pair::{Identity, PairConfig};
use super::web::WebFront;
use super::{EdgeConfig, LocalConfig};

/// How long pairing and checking wait for the edge node.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a page waits for the edge node's offer before it says the edge node is not answering.
const OFFER_TIMEOUT: Duration = Duration::from_secs(3);
/// How long pairing waits for the new link to come up before it answers.
const LINK_WAIT: Duration = Duration::from_secs(5);

/// What [`start`] gives the listeners on an edge node: the HTTPS listener's hand-off and the web front of the paired
/// nodes' hosts.
#[derive(Default)]
pub(crate) struct Started {
    pub acceptor: Option<Arc<Acceptor>>,
    pub front: Option<Arc<WebFront>>,
}

/// Install the edge link of this process: the hook, the dialer when this node is paired (its down connection serves
/// `down`), and on an edge node the listener's hand-off (`resolver` is the HTTPS listener's certificates; `None` when
/// it has none), the web front and the SSH tunnel port. Called from `http::listen` on its runtime; [`stopped`] undoes
/// what holds the app.
pub(crate) fn start(
    factory: &Arc<ExecutionContextFactory>,
    resolver: Option<Arc<dyn rustls::server::ResolvesServerCert>>,
    shutdown: watch::Receiver<bool>,
    down: crate::http::DownServe,
) -> Started {
    let role = moqui_core::edge::node_role_kind();
    // the edge end of a link: a dedicated edge node or an all-in-one node (plans/Edge-Node.md §10)
    let takes_links = role.takes_links();
    // the edge conf is read now, with the role this node started with (plans/Edge-Node.md §5.2)
    let settings = moqui_core::edge::settings(factory);
    if settings.role != role {
        tracing::warn!("node_role changed after this server read its edge conf; it keeps the role it started with");
    }
    // the web protection, which also admits the SSH tunnel port's connections on an edge node
    let guard = Arc::clone(&down.app.guard);
    if !takes_links {
        super::down::install(factory, down);
    }
    let server = match (takes_links, resolver) {
        (true, Some(resolver)) => {
            let key = moqui_lib::env::get_prop_or_env(moqui_core::edge::LINK_KEY);
            match Identity::from_key(&key, "moqui-edge-link") {
                Ok(identity) => {
                    let mut config = EdgeConfig::from_properties(&instance_id(factory));
                    config.offline_page = offline_page(factory);
                    config.capabilities = super::offer::areas(factory, &config);
                    let server = EdgeServer::new(factory, config, &identity.fingerprint, shutdown.clone());
                    super::accept::acceptor(&server, resolver, &identity).map(|acceptor| (server, acceptor))
                }
                Err(err) => {
                    tracing::error!(
                        "node_role is {}, but {}: {err}; this node takes no links",
                        role.as_str(),
                        moqui_core::edge::LINK_KEY
                    );
                    None
                }
            }
        }
        (true, None) => {
            tracing::error!(
                "node_role is {}, but the HTTPS listener is off (no certificate in runtime/cert, or \
                 webapp_https_listen_port=off): this node takes no links",
                role.as_str()
            );
            None
        }
        (false, _) => None,
    };
    let hook = Arc::new(Hook {
        factory: Arc::downgrade(factory),
        runtime: tokio::runtime::Handle::current(),
        shutdown,
        server: server.as_ref().map(|(server, _)| Arc::downgrade(server)),
        dialer: Mutex::new(None),
    });
    if !takes_links {
        let weak = Arc::downgrade(&hook);
        moqui_core::telemetry::for_factory(factory).observe_gauge("moqui.edgelink.up", move || {
            let hook = weak.upgrade()?;
            let linked = lock(&hook.dialer).as_ref().is_some_and(is_linked);
            Some(if linked { 1.0 } else { 0.0 })
        });
    }
    let paired = (!takes_links)
        .then(|| LocalConfig::from_properties(&instance_id(factory)))
        .flatten();
    // the queue's metrics and retention, where the link carries work: an edge or all-in-one node here, a local node
    // when its dialer starts (Hook::start_dialer)
    if takes_links {
        super::queue::maintain(factory, hook.shutdown.clone());
    }
    if takes_links
        && !moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_URL)
            .trim()
            .is_empty()
    {
        tracing::warn!(
            "edge_node_url is set on an edge node (node_role={}): it does not dial an edge node itself",
            role.as_str()
        );
    }
    use moqui_core::startup::{integration, listener, State};
    match paired {
        Some(config) => {
            let url = config.url.clone();
            match hook.start_dialer(config) {
                Ok(()) => integration(factory, "Edge link", State::on(format!("dials the edge node {url}"))),
                Err(err) => {
                    tracing::error!("The edge link is not started: {err}");
                    integration(factory, "Edge link", State::failed(err));
                }
            }
        }
        None if server.is_some() => integration(
            factory,
            "Edge link",
            State::on(format!(
                "node role {}, takes links on the HTTPS listener",
                role.as_str()
            )),
        ),
        None if takes_links => integration(factory, "Edge link", State::failed("see the edge link error above")),
        None => integration(factory, "Edge link", State::off("edge_node_url is not set")),
    }
    moqui_core::edge::install_edge_link(Arc::clone(&hook) as Arc<dyn EdgeLink>);
    if let Some((server, _)) = &server {
        *lock(&CURRENT_SERVER) = Arc::downgrade(server);
    }
    match server {
        Some((server, acceptor)) => {
            let loading = Arc::clone(&server);
            tokio::spawn(async move { loading.load_hosts().await });
            let mut opened = Vec::new();
            let ports = super::tunnel::ports(server.config());
            let busy = crate::port_check::busy(super::TUNNEL_LISTENER);
            if ports.is_empty() {
                listener(
                    factory,
                    super::TUNNEL_LISTENER,
                    State::off("edge_ssh_tunnel_port, edge_xmpp_port and edge_xmpp_tls_port are not set"),
                );
            } else if let Some(busy) = &busy {
                listener(factory, super::TUNNEL_LISTENER, State::Skipped(busy.clone()));
            }
            for port in ports.into_iter().filter(|_| busy.is_none()) {
                // an all-in-one node's own XMPP server keeps its ports; the paired node's domain has its own (§10)
                if let Some(own) = role
                    .all_in_one()
                    .then(|| super::tunnel::own_xmpp_clash(port.bind))
                    .flatten()
                {
                    tracing::error!(
                        "The edge {} tunnel port {} is the server's own XMPP port ({own}); that tunnel is off: give \
                         it a port of its own",
                        port.kind,
                        port.bind
                    );
                    continue;
                }
                let kind = port.kind;
                if let Some(address) = super::tunnel::listen(&server, port, Arc::clone(&guard)) {
                    opened.push(format!("{kind} tcp {address}"));
                }
            }
            if busy.is_none() && !opened.is_empty() {
                listener(factory, super::TUNNEL_LISTENER, State::On(opened.join(", ")));
            } else {
                moqui_core::startup::report(factory).record_default(
                    moqui_core::startup::Section::Listener,
                    super::TUNNEL_LISTENER,
                    State::failed("see the edge tunnel errors above"),
                );
            }
            Started {
                acceptor: Some(acceptor),
                front: Some(Arc::new(WebFront::new(&server))),
            }
        }
        None => {
            listener(
                factory,
                super::TUNNEL_LISTENER,
                State::off(if takes_links {
                    "this node takes no links"
                } else {
                    "node_role is not edge or all-in-one"
                }),
            );
            Started::default()
        }
    }
}

/// This process's edge node, while it runs: what a PASS frontend's `edge:` calls go down (`crate::pass_authority`).
/// One per process, as the edge link hook is.
static CURRENT_SERVER: Mutex<Weak<EdgeServer>> = Mutex::new(Weak::new());

/// The edge node of this process, while it runs.
pub(crate) fn edge_server() -> Option<Arc<EdgeServer>> {
    lock(&CURRENT_SERVER).upgrade()
}

/// The listeners have stopped: let go of the app this node's down connections served, of replication's notes, and of
/// the edge link hook (its dialer stops when dropped).
pub(crate) fn stopped(factory: &ExecutionContextFactory) {
    moqui_core::edge::clear_edge_link();
    super::down::clear(factory);
    // replication's notes go with the server, as a restart's would
    if let Some(facade) = factory.entity() {
        moqui_core::edge::replica::disable(&facade);
    }
}

/// The page a terminate host shows while its node's link is down: the text at `edge_offline_page`, else the default.
fn offline_page(factory: &ExecutionContextFactory) -> Arc<str> {
    let location = moqui_lib::env::get_prop_or_env("edge_offline_page");
    let location = location.trim();
    if location.is_empty() {
        return Arc::from(super::web::DEFAULT_OFFLINE_PAGE);
    }
    match factory.resource().location_text(location) {
        Ok(text) => Arc::from(text),
        Err(err) => {
            tracing::error!("edge_offline_page {location} could not be read ({err}); the default page is used");
            Arc::from(super::web::DEFAULT_OFFLINE_PAGE)
        }
    }
}

fn instance_id(factory: &ExecutionContextFactory) -> String {
    moqui_core::cluster::instance_id(factory).unwrap_or_else(|| moqui_lib::env::get_prop_or_env("instance_id"))
}

struct Hook {
    factory: Weak<ExecutionContextFactory>,
    runtime: tokio::runtime::Handle,
    shutdown: watch::Receiver<bool>,
    server: Option<Weak<EdgeServer>>,
    dialer: Mutex<Option<Dialer>>,
}

impl Hook {
    fn factory(&self) -> Result<Arc<ExecutionContextFactory>, Error> {
        self.factory
            .upgrade()
            .ok_or_else(|| Error::new("the server is stopping"))
    }

    fn server(&self) -> Result<Arc<EdgeServer>, Error> {
        if !moqui_core::edge::takes_links() {
            return Err(Error::new(
                "this is not an edge node (node_role is not edge or all-in-one)",
            ));
        }
        self.server
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| Error::new("this edge node takes no links: its HTTPS listener is off"))
    }

    fn start_dialer(&self, mut config: LocalConfig) -> Result<(), String> {
        let factory = self.factory.upgrade();
        if config.web.is_none() {
            config.web = factory.as_deref().and_then(super::down::local_web);
        }
        // a node paired before E7 pins its edge node's instance id on its first link, into runtime/.env
        if config.on_pin.is_none() {
            if let Ok(env) = self.env_path() {
                config.on_pin = Some(Arc::new(move |id: &str| {
                    let set = [(NODE_ID.to_string(), id.to_string())];
                    match moqui_lib::env::update_dotenv_file(&env, &set, &[]) {
                        Ok(()) => moqui_lib::env::set_property(NODE_ID, id.to_string()),
                        Err(err) => tracing::error!("The edge node's instance id was not written to .env: {err}"),
                    }
                }));
            }
        }
        if let Some(factory) = &factory {
            super::queue::maintain(factory, self.shutdown.clone());
        }
        let telemetry = factory.map(|factory| moqui_core::telemetry::for_factory(&factory));
        let dialer = Dialer::start(config, &self.runtime, self.shutdown.clone(), telemetry)?;
        // the dialer it replaces stops when dropped
        *lock(&self.dialer) = Some(dialer);
        Ok(())
    }

    /// The up connection's client of this local node's link; an error on an edge node or a node not paired.
    fn up_client(&self) -> Result<Option<super::LinkClient>, Error> {
        if moqui_core::edge::takes_links() {
            return Err(Error::new("this is an edge node: it offers, and local nodes ask"));
        }
        match lock(&self.dialer).as_ref() {
            Some(dialer) => Ok(dialer.up_client()),
            None => Err(Error::new("this node is not paired with an edge node")),
        }
    }

    fn env_path(&self) -> Result<std::path::PathBuf, Error> {
        Ok(self.factory()?.runtime_dir().join(".env"))
    }
}

impl EdgeLink for Hook {
    fn status(&self) -> LinkStatus {
        let mut status = lock(&self.dialer).as_ref().map(Dialer::status).unwrap_or_default();
        if let Some(server) = self.server.as_ref().and_then(Weak::upgrade) {
            status.link_fingerprint = server.link_fingerprint().to_string();
        }
        status
    }

    fn check(&self, url: &str) -> Result<EdgeInfo, Error> {
        let url = url.to_string();
        crate::edge::block_on(async move { super::pair::check(&url, None, CALL_TIMEOUT).await })?
            .map_err(|err| Error::new(err.message().to_string()))
    }

    fn pair(&self, url: &str, code: &str, expect_fingerprint: Option<&str>) -> Result<PairResult, Error> {
        if moqui_core::edge::takes_links() {
            return Err(Error::new("an edge node does not pair with another edge node"));
        }
        let factory = self.factory()?;
        let instance = instance_id(&factory);
        if instance.is_empty() {
            return Err(Error::new("this node has no instance id yet"));
        }
        let mut config = PairConfig::new(url, code, &instance);
        config.expect_pin = expect_fingerprint.map(str::to_string);
        config.connect_timeout = CALL_TIMEOUT;
        let host = moqui_lib::env::get_prop_or_env("webapp_http_host");
        if !host.trim().is_empty() {
            config.node_name = host.trim().to_string();
        }
        let paired = crate::edge::block_on(async move { super::pair::pair(&config).await })?
            .map_err(|err| Error::new(err.message().to_string()))?;
        // its instance id is pinned with its key: never this node's own (a live cluster member's is refused as the
        // link comes up)
        super::check_edge_id("", &paired.edge.instance_id, &instance).map_err(Error::new)?;
        let set = [
            (NODE_URL.to_string(), url.trim().to_string()),
            (NODE_KEY.to_string(), paired.key.clone()),
            (NODE_PIN.to_string(), paired.pin.clone()),
            (NODE_ID.to_string(), paired.edge.instance_id.clone()),
        ];
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &set, &[])?;
        for (name, value) in &set {
            moqui_lib::env::set_property(name, value.clone());
        }
        tracing::info!(
            "Paired with the edge node at {} (fingerprint {}); this node's key is {}",
            url.trim(),
            paired.pin,
            paired.key_fingerprint
        );
        let mut local = LocalConfig::new(url.trim(), &paired.key, &paired.pin, &instance);
        local.edge_id.clone_from(&paired.edge.instance_id);
        if !host.trim().is_empty() {
            local.node_name = host.trim().to_string();
        }
        self.start_dialer(local).map_err(Error::new)?;
        // the wizard's next step asks the edge node's offer on the link: give it a few seconds to come up
        let waiting = std::time::Instant::now();
        while waiting.elapsed() < LINK_WAIT && !lock(&self.dialer).as_ref().is_some_and(is_linked) {
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(PairResult {
            edge: paired.edge,
            key_fingerprint: paired.key_fingerprint,
        })
    }

    fn unpair(&self) -> Result<(), Error> {
        let dialer = lock(&self.dialer).take().map(Arc::new);
        let told = match &dialer {
            Some(dialer) => {
                let unpairing = Arc::clone(dialer);
                crate::edge::block_on(async move { unpairing.unpair().await })?
            }
            None => Ok(()),
        };
        // the dialer stopped: wait for its link's work to end (the media row's heartbeat among it), then the edge
        // node is no media node of this node's calls any more: its rows go, whether or not a link came up since this
        // process started, and so does its TURN secret (plans/Edge-Node.md §5.4)
        if let Some(dialer) = dialer {
            let waiting = std::time::Instant::now();
            while is_linked(&dialer) && waiting.elapsed() < LINK_WAIT {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(dialer);
        }
        let factory = self.factory()?;
        moqui_core::edge::media::clear_peer_turn(&factory);
        // under the rows' lock, and no row may be written after it: a heartbeat or an Apply in flight does not make
        // a row again (moqui_core::edge::calls::register_row, register_sip_row): its call media row (E7) and its SIP
        // engine's (E8)
        moqui_core::edge::calls::forget_rows(&factory, || {
            if let Some(facade) = factory.entity() {
                for (entity, what) in [
                    ("moqui.call.CallMediaNode", "call media"),
                    ("moqui.call.SipNode", "SIP node"),
                ] {
                    let removed = facade.find(entity).and_then(|find| {
                        find.eq("edgeLinked", moqui_lib::object::Value::string("Y"))
                            .disable_authz()
                            .delete_all()
                    });
                    if let Err(err) = removed {
                        tracing::warn!("Unpaired, but the edge node's {what} row was not removed: {err}");
                    }
                }
            }
        });
        // the wizard's calls and phones choices go too: a node paired again starts with both on its edge node, as E7
        // and E8 do
        let calls = moqui_core::edge::calls::CALLS_ENABLED;
        let sip = moqui_core::edge::sip::SIP_ENABLED;
        let names = [NODE_URL, NODE_KEY, NODE_PIN, NODE_ID, calls, sip];
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &[], &names)?;
        for name in names {
            moqui_lib::env::clear_property(name);
        }
        // nothing more goes to that edge node: replication stops noting writes, and the outbox fails, kept for
        // inspection, rather than going to whichever edge node this node pairs with next (plans/Edge-Node.md §5.2)
        if let Some(facade) = self.factory()?.entity() {
            moqui_core::edge::replica::disable(&facade);
            // the mail servers the wizard's Apply set to edge: send as they did before it (plans/Edge-Node.md §5.7)
            for line in moqui_core::edge::setup::restore_local(&facade) {
                if line.ok {
                    tracing::info!("Unpaired: {}", line.text);
                }
            }
            let queue = moqui_core::edge::queue::TO_EDGE;
            let settings = moqui_core::edge::settings(&*self.factory()?);
            let now = moqui_core::user::now_millis();
            match moqui_core::edge::queue::fail_pending(&facade, &settings, queue, "unpaired", now) {
                Ok(0) => {}
                Ok(count) => tracing::warn!("Unpaired: {count} queued items for the edge node failed (unpaired)"),
                Err(err) => tracing::error!("Unpaired, but the queued items for the edge node were not failed: {err}"),
            }
        }
        match told {
            Ok(()) => {
                tracing::info!("Unpaired from the edge node");
                Ok(())
            }
            Err(err) => {
                tracing::warn!("Unpaired here, but the edge node was not told: {err}");
                Err(Error::new(format!("unpaired here, but {err}")))
            }
        }
    }

    fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, Error> {
        self.server()?.create_pairing_code(valid_minutes)
    }

    fn clients(&self) -> Result<Vec<EdgeClientInfo>, Error> {
        self.server()?.clients()
    }

    fn revoke(&self, paired_node_id: &str) -> Result<bool, Error> {
        self.server()?.revoke(paired_node_id)
    }

    fn hosts(&self, paired_node_id: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error> {
        self.server()?;
        super::hosts::list(&*self.factory()?, paired_node_id)
    }

    fn add_host(&self, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error> {
        let server = self.server()?;
        let factory = self.factory()?;
        let saved = super::hosts::add(&factory, host)?;
        server.hosts.load(&factory)?;
        tracing::info!(
            "Edge node: host {} goes to local node {} ({})",
            saved.host_name,
            saved.paired_node_id,
            saved.mode_enum_id
        );
        Ok(saved)
    }

    fn remove_host(&self, host_name: &str) -> Result<bool, Error> {
        let server = self.server()?;
        let factory = self.factory()?;
        let removed = super::hosts::remove(&factory, host_name)?;
        server.hosts.load(&factory)?;
        if removed {
            tracing::info!("Edge node: host {} removed", host_name.trim());
        }
        Ok(removed)
    }

    fn delegate(&self, call: &DelegateCall) -> Result<DelegateAnswer, Error> {
        let server = self
            .server
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| Error::new(format!("{}: this node takes no links", moqui_core::edge::OFFLINE)))?;
        let call = call.clone();
        crate::edge::block_on(async move { server.delegate(call).await })?
    }

    fn offer(&self) -> Result<EdgeOffer, Error> {
        let client = self.up_client()?;
        crate::edge::block_on(async move { super::dial::offer_on(client, OFFER_TIMEOUT).await })?.map_err(Error::new)
    }

    fn configure(&self, setup: &EdgeSetup) -> Result<Configured, Error> {
        let client = self.up_client()?;
        let setup = setup.clone();
        let configured =
            crate::edge::block_on(async move { super::dial::configure_on(client, &setup, CALL_TIMEOUT).await })?
                .map_err(Error::new)?;
        // the SIP realm the answer says goes to the edge node's SipNode row now, not at the next link (E12)
        if let Some(dialer) = lock(&self.dialer).as_ref() {
            dialer.note_configured(&configured);
        }
        Ok(configured)
    }

    fn xmpp_probe(&self, port: u16, direct: bool) -> Result<String, Error> {
        let factory = self.factory()?;
        if moqui_core::edge::settings(&factory).takes_links() {
            return Err(Error::new("this is an edge node: its local nodes run the XMPP servers"));
        }
        let url = self.status().url;
        crate::edge::block_on(super::probe::xmpp_probe(factory, url, port, direct))?.map_err(Error::new)
    }

    fn turn_probe(&self, url: &str, username: &str, credential: &str, peer: &str) -> Result<String, Error> {
        if moqui_core::edge::settings(&*self.factory()?).takes_links() {
            return Err(Error::new("this is an edge node: its local nodes check its TURN"));
        }
        super::probe::turn_probe(url, username, credential, peer).map_err(Error::new)
    }

    fn ping_client(&self, paired_node_id: &str) -> Result<f64, Error> {
        let server = self.server()?;
        let node = paired_node_id.to_string();
        let rtt = crate::edge::block_on(async move { server.ping_local(&node).await })?.map_err(Error::new)?;
        Ok(rtt.as_secs_f64() * 1000.0)
    }
}
