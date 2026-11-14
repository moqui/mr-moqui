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
//! and XMPP stay a normal node's, and only its paired node's `EdgeHost` names go down the link. A node whose role has
//! no front set (`node_role=edge`, plans/Edge-Node2.md §4.3) forwards no web access: its local nodes' names are reached
//! through a front node. A front node (`node_role=front`, §4.7) has the front set alone: no queue, ports only as the
//! front's, and nothing of the edge set.
//! Pairing writes `edge_node_url`, `edge_node_key` and `edge_node_pin` to `runtime/.env` and sets them as properties;
//! unpairing removes them (plans/archive/Edge-Node.md §5.1, §5.2, §5.3, §5.7).
//!
//! **The front pairing** (plans/Edge-Node2.md §4.7, E19). Pairing with a node whose Hello names the role `front` keeps
//! it as a second pairing, `front_node_url`, `front_node_key`, `front_node_pin` and `front_node_id`, with a dialer of its
//! own whose links carry web access and ports only ([`super::front`]). A local node may hold both: its web names on the
//! front, its queue, delegated calls, replication, comms and PASS on its edge cluster. A node with the edge set
//! (`front_edge`, `edge`, `all-in-one`) dials a front as well as taking links, and pairs with nothing else: its down
//! connection serves its own app with its web front, so the front's requests for its local nodes' terminate hosts go on
//! down their links, and its passthrough hosts' tunnels too; it gives the front its host names itself.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{
    Configured, DelegateAnswer, DelegateCall, EdgeClientInfo, EdgeInfo, EdgeLink, EdgeOffer, EdgeSetup, LinkStatus,
    MemberState, NodeRole, PairResult, PairingCode, PoolMemberInfo, FRONT_ID, FRONT_KEY, FRONT_PIN, FRONT_URL, NODE_ID,
    NODE_KEY, NODE_PAIRING, NODE_PIN, NODE_URL,
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
/// How often an edge node that dials a front looks again at the host names it gave it (plans/Edge-Node2.md §4.7),
/// besides when a change here tells it.
const FRONT_NAMES_EVERY: Duration = Duration::from_secs(5);

/// Which of a node's two pairings (plans/Edge-Node2.md §4.7): with its edge node (`edge_node_*`), or with its front
/// node (`front_node_*`), whose links carry web access and ports only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pairing {
    Edge,
    Front,
}

impl Pairing {
    /// The property that keeps the ids learned on its links, the set a web protection ban goes up to.
    fn id_property(self) -> &'static str {
        match self {
            Self::Edge => NODE_ID,
            Self::Front => FRONT_ID,
        }
    }

    /// Its properties, removed at unpairing.
    fn properties(self) -> &'static [&'static str] {
        match self {
            Self::Edge => &[NODE_URL, NODE_KEY, NODE_PIN, NODE_ID, NODE_PAIRING],
            Self::Front => &[FRONT_URL, FRONT_KEY, FRONT_PIN, FRONT_ID],
        }
    }

    /// Where its list of the other end's members is kept.
    fn members_file(self, runtime: &std::path::Path) -> std::path::PathBuf {
        match self {
            Self::Edge => super::members::file(runtime),
            Self::Front => super::members::front_file(runtime),
        }
    }
}

/// Which of its pairings a node of `role` keeps a pairing with a node of `peer` role as (plans/Edge-Node2.md §4.7): a
/// front node is the front pairing, any other the edge pairing; a node that takes links pairs with a front only, and a
/// front with nothing. `peer` is `None` before the other node is known: whether this node pairs at all.
fn pairing_slot(role: NodeRole, peer: Option<&str>) -> Result<Pairing, String> {
    if !role.dials_front() {
        return Err("a front node does not pair with another node".to_string());
    }
    match peer {
        Some(peer) if peer == NodeRole::Front.as_str() => Ok(Pairing::Front),
        Some(peer) if role.takes_links() => Err(super::pair::not_front(peer)),
        _ => Ok(Pairing::Edge),
    }
}

/// The properties pairing writes for `pairing` (`runtime/.env` and the process): the URL, this node's key, the pin and
/// the other node's id, and for the edge pairing the pairing id every member of this cluster is known by
/// (plans/Edge-Node2.md §4.4); the front pairing keeps no queue rows, so it has none.
fn pairing_properties(
    pairing: Pairing,
    url: &str,
    paired: &super::pair::Paired,
    instance: &str,
) -> Vec<(String, String)> {
    // in the order of `Pairing::properties`: the edge pairing's last one is its pairing id
    pairing
        .properties()
        .iter()
        .zip([
            url.trim().to_string(),
            paired.key.clone(),
            paired.pin.clone(),
            paired.edge.instance_id.clone(),
            instance.to_string(),
        ])
        .map(|(name, value)| ((*name).to_string(), value))
        .collect()
}

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
    // the edge end of a link: a dedicated edge node or an all-in-one node (plans/archive/Edge-Node.md §10)
    let takes_links = role.takes_links();
    // the edge conf is read now, with the role this node started with (plans/archive/Edge-Node.md §5.2)
    let settings = moqui_core::edge::settings(factory);
    if settings.role != role {
        tracing::warn!("node_role changed after this server read its edge conf; it keeps the role it started with");
    }
    // the web protection, which also admits the SSH tunnel port's connections on an edge node
    let guard = Arc::clone(&down.app.guard);
    let down = if takes_links {
        Some(down)
    } else {
        super::down::install(factory, down, None);
        None
    };
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
    // the front set's web access by host name (plans/Edge-Node2.md §4.3): an edge node behind a front has none
    let front = server
        .as_ref()
        .filter(|_| role.has_front())
        .map(|(server, _)| Arc::new(WebFront::new(server)));
    // a node with the edge set that dials its front (plans/Edge-Node2.md §4.7): its down connection serves its app with
    // its web front, and its passthrough hosts' tunnels go on to their nodes
    if let (Some(down), Some((server, _)), true) = (down, &server, role.dials_front()) {
        let app = crate::http::WebApp {
            edge: front.clone(),
            ..down.app.clone()
        };
        let down = crate::http::DownServe { app, ..down };
        super::down::install(factory, down, Some(Arc::downgrade(server)));
    }
    let hook = Arc::new(Hook {
        factory: Arc::downgrade(factory),
        runtime: tokio::runtime::Handle::current(),
        shutdown,
        role,
        server: server.as_ref().map(|(server, _)| Arc::downgrade(server)),
        dialer: Mutex::new(None),
        front: Mutex::new(None),
        names: Arc::new(tokio::sync::Notify::new()),
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
    // its front pairing (plans/Edge-Node2.md §4.7): on a local node and on a node with the edge set, never on a front
    let front_paired = role
        .dials_front()
        .then(|| LocalConfig::front_from_properties(&instance_id(factory)))
        .flatten();
    // the queue's metrics and retention, where the link carries work: an edge or all-in-one node here, a local node
    // when its dialer starts (Hook::start_dialer); a front node has no queue
    if takes_links && role.has_edge() {
        super::queue::maintain(factory, hook.shutdown.clone());
    }
    if takes_links
        && !moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_URL)
            .trim()
            .is_empty()
    {
        tracing::warn!(
            "edge_node_url is set on an edge node (node_role={}): it does not dial an edge node itself; it dials only a \
             front node (front_node_url)",
            role.as_str()
        );
    }
    if !role.dials_front()
        && !moqui_lib::env::get_prop_or_env(moqui_core::edge::FRONT_URL)
            .trim()
            .is_empty()
    {
        tracing::warn!("front_node_url is set on a front node: a front node dials no other node");
    }
    use moqui_core::startup::{integration, listener, State};
    match front_paired {
        Some(config) => {
            let url = config.url.clone();
            match hook.start_dialer(config, Pairing::Front) {
                Ok(()) => integration(factory, "Front link", State::on(format!("dials the front node {url}"))),
                Err(err) => {
                    tracing::error!("The front link is not started: {err}");
                    integration(factory, "Front link", State::failed(err));
                }
            }
        }
        None => integration(factory, "Front link", State::off("front_node_url is not set")),
    }
    // the names an edge node gives its front, kept while it runs (plans/Edge-Node2.md §4.7)
    if takes_links && role.dials_front() && server.is_some() {
        tokio::spawn(keep_front_names(Arc::downgrade(&hook), hook.shutdown.clone()));
    }
    match paired {
        Some(config) => {
            let url = config.url.clone();
            match hook.start_dialer(config, Pairing::Edge) {
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
            // the links this member holds, for admission on the other members of an edge cluster
            let writing = Arc::clone(&server);
            tokio::spawn(async move { writing.keep_member_links().await });
            // another member of an edge cluster changed the host names or the pairings (plans/Edge-Node2.md §4.6)
            let (weak_server, weak_factory) = (Arc::downgrade(&server), Arc::downgrade(factory));
            moqui_core::edge::cluster_state::install(
                factory,
                Arc::new(move || {
                    let (Some(server), Some(factory)) = (weak_server.upgrade(), weak_factory.upgrade()) else {
                        return;
                    };
                    let context = factory.new_context();
                    let _entered = context.enter();
                    let _authz = context.disable_authz();
                    match server.hosts.load(&factory) {
                        Ok(count) => tracing::info!(
                            "Edge node: the host table loaded again after another member's change ({count} host names)"
                        ),
                        Err(err) => tracing::error!(
                            "Edge node: the host table was not loaded again after another member's change: {err}"
                        ),
                    }
                }),
                {
                    let weak_server = Arc::downgrade(&server);
                    Arc::new(move |ops: &[moqui_core::edge::cluster_state::LinkOp]| {
                        if let Some(server) = weak_server.upgrade() {
                            for op in ops {
                                server.apply_link_op(op);
                            }
                        }
                    })
                },
            );
            let mut opened = Vec::new();
            // the TCP ports are the front set's (plans/Edge-Node2.md §4.3): an edge node behind a front binds none
            let ports = if role.has_front() {
                super::tunnel::ports(server.config())
            } else {
                Vec::new()
            };
            let busy = crate::port_check::busy(super::TUNNEL_LISTENER);
            if !role.has_front() {
                listener(
                    factory,
                    super::TUNNEL_LISTENER,
                    State::off("node_role edge has no ports: its front node takes them"),
                );
            } else if ports.is_empty() {
                listener(
                    factory,
                    super::TUNNEL_LISTENER,
                    State::off("edge_ssh_tunnel_port, edge_xmpp_port and edge_xmpp_tls_port are not set"),
                );
            } else if let Some(busy) = &busy {
                listener(factory, super::TUNNEL_LISTENER, State::Skipped(busy.clone()));
            }
            for port in ports.into_iter().filter(|_| busy.is_none()) {
                // an all-in-one node's own XMPP ports take its pairings' domains too (E16, tunnel::xmpp_to_pairing)
                if let Some(own) = role
                    .all_in_one()
                    .then(|| super::tunnel::own_xmpp_clash(port.bind))
                    .flatten()
                {
                    tracing::info!(
                        "The edge {} tunnel port {} is the server's own XMPP port ({own}), which takes the paired \
                         nodes' XMPP domains too",
                        port.kind,
                        port.bind
                    );
                    continue;
                }
                let kind = port.kind;
                match super::tunnel::listen(&server, port, Arc::clone(&guard), Arc::downgrade(factory), None) {
                    Ok(address) => opened.push(format!("{kind} tcp {address}")),
                    Err(err) => tracing::error!("{err}"),
                }
            }
            // the ports of one pairing each, as the System app sets them (E16); not while another process holds the
            // tunnel listeners (the port check skipped them), whose rows it serves
            if busy.is_none() && role.has_front() {
                super::tunnel::watch_rows(&server, Arc::clone(&guard), Arc::downgrade(factory));
            }
            if busy.is_none() && !opened.is_empty() {
                listener(factory, super::TUNNEL_LISTENER, State::On(opened.join(", ")));
            } else if role.has_front() {
                moqui_core::startup::report(factory).record_default(
                    moqui_core::startup::Section::Listener,
                    super::TUNNEL_LISTENER,
                    State::failed("see the edge tunnel errors above"),
                );
            }
            Started {
                acceptor: Some(acceptor),
                front,
            }
        }
        None => {
            listener(
                factory,
                super::TUNNEL_LISTENER,
                State::off(if takes_links {
                    "this node takes no links"
                } else {
                    "node_role is not set"
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
    /// The role this node started with: a test runs nodes of several roles in one process.
    role: NodeRole,
    server: Option<Weak<EdgeServer>>,
    /// The edge pairing's dialer (`edge_node_*`).
    dialer: Mutex<Option<Dialer>>,
    /// The front pairing's dialer (`front_node_*`, plans/Edge-Node2.md §4.7).
    front: Mutex<Option<Dialer>>,
    /// Wakes the keeper of the names an edge node gives its front: its host names changed.
    names: Arc<tokio::sync::Notify>,
}

impl Hook {
    fn factory(&self) -> Result<Arc<ExecutionContextFactory>, Error> {
        self.factory
            .upgrade()
            .ok_or_else(|| Error::new("the server is stopping"))
    }

    fn server(&self) -> Result<Arc<EdgeServer>, Error> {
        if !self.role.takes_links() {
            return Err(Error::new("this is not an edge node (node_role is not set)"));
        }
        self.server
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| Error::new("this edge node takes no links: its HTTPS listener is off"))
    }

    fn slot(&self, pairing: Pairing) -> &Mutex<Option<Dialer>> {
        match pairing {
            Pairing::Edge => &self.dialer,
            Pairing::Front => &self.front,
        }
    }

    fn start_dialer(&self, mut config: LocalConfig, pairing: Pairing) -> Result<(), String> {
        let factory = self.factory.upgrade();
        config.front = pairing == Pairing::Front;
        if config.web.is_none() {
            config.web = factory.as_deref().and_then(super::down::local_web);
        }
        // this instance's own key, which its links prove (plans/Edge-Node2.md §4.11), and the edge cluster's list it
        // keeps (§4.6), each pairing's apart
        if let Some(factory) = &factory {
            config.member_key = Some(super::member_key::MemberKey::load_or_create(factory.runtime_dir())?);
            if config.members_file.is_none() {
                config.members_file = Some(pairing.members_file(factory.runtime_dir()));
            }
        }
        // edge_node_id (front_node_id) is the set of the other end's member ids, each learned on a link
        // (plans/Edge-Node2.md §4.6), which a ban goes up to (crate::guard_share); a node paired before E7 writes the
        // first to runtime/.env
        if config.on_pin.is_none() {
            if let Ok(env) = self.env_path() {
                let property = pairing.id_property();
                config.on_pin = Some(Arc::new(move |id: &str| {
                    let current = moqui_lib::env::get_prop_or_env(property);
                    let mut ids: Vec<String> = current
                        .split(',')
                        .map(str::trim)
                        .filter(|known| !known.is_empty())
                        .map(str::to_string)
                        .collect();
                    if ids.is_empty() {
                        let set = [(property.to_string(), id.to_string())];
                        if let Err(err) = moqui_lib::env::update_dotenv_file(&env, &set, &[]) {
                            tracing::error!("{property} was not written to .env: {err}");
                        }
                    }
                    if !ids.iter().any(|known| known == id) {
                        ids.push(id.to_string());
                    }
                    moqui_lib::env::set_property(property, ids.join(","));
                }));
            }
        }
        // a local node's queue goes up its edge pairing; a front pairing carries none
        if let (Some(factory), Pairing::Edge) = (&factory, pairing) {
            super::queue::maintain(factory, self.shutdown.clone());
        }
        let telemetry = factory.map(|factory| moqui_core::telemetry::for_factory(&factory));
        let dialer = Dialer::start(config, &self.runtime, self.shutdown.clone(), telemetry)?;
        // the dialer it replaces stops when dropped
        *lock(self.slot(pairing)) = Some(dialer);
        // a new front link gives the front this node's names at once (an edge node's, plans/Edge-Node2.md §4.7)
        if pairing == Pairing::Front {
            self.names.notify_one();
        }
        Ok(())
    }

    /// The up connections' clients of this node's pairings, the edge pairing's and the front's, each `None` while it
    /// is not paired; an error on a node that takes links (it offers, and local nodes ask) or one not paired at all.
    #[allow(clippy::type_complexity)]
    fn up_clients(&self) -> Result<(Option<Option<super::LinkClient>>, Option<Option<super::LinkClient>>), Error> {
        if self.role.takes_links() {
            return Err(Error::new("this is an edge node: it offers, and local nodes ask"));
        }
        let edge = lock(&self.dialer).as_ref().map(Dialer::up_client);
        let front = lock(&self.front).as_ref().map(Dialer::up_client);
        if edge.is_none() && front.is_none() {
            return Err(Error::new("this node is not paired with an edge node"));
        }
        Ok((edge, front))
    }

    /// The URL of the pairing that carries web access and ports: the front's when this node has one, else the edge
    /// node's (plans/Edge-Node2.md §4.7).
    fn web_url(&self) -> String {
        match lock(&self.front).as_ref() {
            Some(front) => front.status().url,
            None => lock(&self.dialer)
                .as_ref()
                .map(|dialer| dialer.status().url)
                .unwrap_or_default(),
        }
    }

    /// The SIP realm a Configure's answer from the edge pairing says goes to the edge node's SipNode row now, not at the
    /// next link (E12).
    fn note_configured(&self, configured: &Configured) {
        if let Some(dialer) = lock(&self.dialer).as_ref() {
            dialer.note_configured(configured);
        }
    }

    fn env_path(&self) -> Result<std::path::PathBuf, Error> {
        Ok(self.factory()?.runtime_dir().join(".env"))
    }

    /// Wait up to [`LINK_WAIT`] for a link of `pairing` to come up: the wizard's next step asks on it.
    fn wait_linked(&self, pairing: Pairing) {
        let waiting = std::time::Instant::now();
        while waiting.elapsed() < LINK_WAIT && !lock(self.slot(pairing)).as_ref().is_some_and(is_linked) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Keep a pairing with the front node at `url` as this node's front pairing (plans/Edge-Node2.md §4.7): written to
    /// `front_node_*` beside any edge pairing, and its dialer started.
    fn pair_front(
        &self,
        factory: &ExecutionContextFactory,
        url: &str,
        paired: &super::pair::Paired,
        instance: &str,
        host: &str,
    ) -> Result<PairResult, Error> {
        // a list kept from an earlier front pairing is not this one's
        super::members::forget_at(&Pairing::Front.members_file(factory.runtime_dir()));
        let set = pairing_properties(Pairing::Front, url, paired, instance);
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &set, &[])?;
        for (name, value) in &set {
            moqui_lib::env::set_property(name, value.clone());
        }
        tracing::info!(
            "Paired with the front node at {} (fingerprint {}); this node's key is {}",
            url.trim(),
            paired.pin,
            paired.key_fingerprint
        );
        let mut local = LocalConfig::new(url.trim(), &paired.key, &paired.pin, instance);
        local.edge_id.clone_from(&paired.edge.instance_id);
        if !host.is_empty() {
            local.node_name = host.to_string();
        }
        self.start_dialer(local, Pairing::Front).map_err(Error::new)?;
        self.wait_linked(Pairing::Front);
        Ok(PairResult {
            edge: paired.edge.clone(),
            key_fingerprint: paired.key_fingerprint.clone(),
        })
    }

    /// Unpair the front pairing: tell the front node, stop its dialer, forget its list and remove `front_node_*`.
    fn unpair_front_pairing(&self) -> Result<(), Error> {
        let dialer = lock(&self.front).take().map(Arc::new);
        let told = match &dialer {
            Some(dialer) => {
                let unpairing = Arc::clone(dialer);
                crate::edge::block_on(async move { unpairing.unpair().await })?
            }
            None => Ok(()),
        };
        drop(dialer);
        let factory = self.factory()?;
        super::members::forget_at(&Pairing::Front.members_file(factory.runtime_dir()));
        let names = Pairing::Front.properties();
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &[], names)?;
        for name in names {
            moqui_lib::env::clear_property(name);
        }
        match told {
            Ok(()) => {
                tracing::info!("Unpaired from the front node");
                Ok(())
            }
            Err(err) => {
                tracing::warn!("Unpaired from the front node here, but it was not told: {err}");
                Err(Error::new(format!("unpaired here, but {err}")))
            }
        }
    }
}

/// How long an edge node waits before it gives its front names it refused again (plans/Edge-Node2.md §4.7): another
/// pairing's claim may be gone.
const FRONT_NAMES_RETRY: Duration = Duration::from_secs(60);

/// The names an edge node gave its front last.
struct Given {
    /// The generations of the front links up then.
    generations: Vec<u64>,
    names: Vec<moqui_core::edge::EdgeHostInfo>,
    /// When, if the front refused any.
    refused: Option<std::time::Instant>,
}

impl Given {
    /// Whether to give them again: a link came up since, the names changed, or the front refused some a while ago.
    fn again(&self, generations: &[u64], names: &[moqui_core::edge::EdgeHostInfo]) -> bool {
        self.generations != generations
            || self.names != names
            || self.refused.is_some_and(|at| at.elapsed() >= FRONT_NAMES_RETRY)
    }
}

/// While an edge node that dials a front runs (plans/Edge-Node2.md §4.7): give the front its host names
/// ([`super::front::names_for_front`]) on each new front link, and again when they change, by Configure on the front
/// pairing. A Configure that fails is logged and tried again; a name the front refuses is logged with why and given
/// again after [`FRONT_NAMES_RETRY`].
async fn keep_front_names(hook: Weak<Hook>, mut shutdown: watch::Receiver<bool>) {
    // the generations the names were given on, the names, and when, while the front refused any of them
    let mut sent: Option<Given> = None;
    loop {
        let Some(strong) = hook.upgrade() else { return };
        let names_changed = Arc::clone(&strong.names);
        let front = lock(&strong.front)
            .as_ref()
            .map(|dialer| (dialer.up_client(), dialer.status()));
        let (role, factory) = (strong.role, Weak::clone(&strong.factory));
        drop(strong);
        if let Some((Some(client), status)) = front {
            let generations: Vec<u64> = status
                .members
                .iter()
                .filter(|member| member.state == moqui_core::edge::LinkState::Linked)
                .map(|member| member.generation)
                .collect();
            let names = crate::worker::on_pool(&factory, move |factory| {
                super::front::names_for_front(factory, role)
                    .map_err(|err| moqui_core::context::Error::new(err.message().to_string()))
            })
            .await;
            match names {
                Ok(names) if sent.as_ref().is_none_or(|given| given.again(&generations, &names)) => {
                    let setup = EdgeSetup {
                        hosts: names.clone(),
                        set_hosts: true,
                        ..EdgeSetup::default()
                    };
                    match super::dial::configure_on(Some(client), &setup, CALL_TIMEOUT, true).await {
                        Ok(configured) => {
                            let mut refused_any = false;
                            for refused in configured.hosts.iter().filter(|host| !host.error.is_empty()) {
                                refused_any = true;
                                tracing::warn!(
                                    "Front link: the front node did not take {}: {}",
                                    refused.host_name,
                                    refused.error
                                );
                            }
                            tracing::info!("Front link: {} host names given to the front node", names.len());
                            sent = Some(Given {
                                generations,
                                names,
                                refused: refused_any.then(std::time::Instant::now),
                            });
                        }
                        Err(err) => {
                            tracing::warn!("Front link: the host names were not given to the front node: {err}")
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => tracing::error!("Front link: the host names for the front node were not read: {err}"),
            }
        }
        tokio::select! {
            () = tokio::time::sleep(FRONT_NAMES_EVERY) => {}
            () = names_changed.notified() => {}
            _ = shutdown.wait_for(|stopped| *stopped) => return,
        }
    }
}

impl EdgeLink for Hook {
    fn status(&self) -> LinkStatus {
        // a node paired with a front alone shows that link, as its only one (plans/Edge-Node2.md §4.7)
        let mut status = match lock(&self.dialer).as_ref() {
            Some(dialer) => dialer.status(),
            None => lock(&self.front).as_ref().map(Dialer::status).unwrap_or_default(),
        };
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
        pairing_slot(self.role, None).map_err(Error::new)?;
        let factory = self.factory()?;
        let instance = instance_id(&factory);
        if instance.is_empty() {
            return Err(Error::new("this node has no instance id yet"));
        }
        let mut config = PairConfig::new(url, code, &instance);
        config.expect_pin = expect_fingerprint.map(str::to_string);
        config.connect_timeout = CALL_TIMEOUT;
        // a node with a role of its own pairs with a front node only (plans/Edge-Node2.md §4.7)
        config.only_front = self.role.takes_links();
        let host = moqui_lib::env::get_prop_or_env("webapp_http_host");
        if !host.trim().is_empty() {
            config.node_name = host.trim().to_string();
        }
        let paired = crate::edge::block_on(async move { super::pair::pair(&config).await })?
            .map_err(|err| Error::new(err.message().to_string()))?;
        // its instance id is pinned with its key: never this node's own (a live cluster member's is refused as the
        // link comes up)
        super::check_edge_member(None, &paired.edge.instance_id, &instance).map_err(Error::new)?;
        // a front node is this node's front pairing, beside an edge pairing it may have (plans/Edge-Node2.md §4.7)
        if pairing_slot(self.role, Some(&paired.edge.role)).map_err(Error::new)? == Pairing::Front {
            return self.pair_front(&factory, url, &paired, &instance, host.trim());
        }
        // a list kept from an earlier pairing is not this one's (plans/Edge-Node2.md §4.6)
        super::members::forget(factory.runtime_dir());
        let set = pairing_properties(Pairing::Edge, url, &paired, &instance);
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
        self.start_dialer(local, Pairing::Edge).map_err(Error::new)?;
        // the wizard's next step asks the edge node's offer on the link: give it a few seconds to come up
        self.wait_linked(Pairing::Edge);
        Ok(PairResult {
            edge: paired.edge,
            key_fingerprint: paired.key_fingerprint,
        })
    }

    fn unpair(&self) -> Result<(), Error> {
        // a node paired with a front alone unpairs that (plans/Edge-Node2.md §4.7)
        if lock(&self.dialer).is_none() && lock(&self.front).is_some() {
            return self.unpair_front_pairing();
        }
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
        // process started, and so does its TURN secret (plans/archive/Edge-Node.md §5.4)
        if let Some(dialer) = dialer {
            let waiting = std::time::Instant::now();
            while is_linked(&dialer) && waiting.elapsed() < LINK_WAIT {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(dialer);
        }
        let factory = self.factory()?;
        moqui_core::edge::media::clear_peer_turn(&factory);
        // the edge cluster's list goes with the pairing (plans/Edge-Node2.md §4.6)
        super::members::forget(factory.runtime_dir());
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
        let names = [NODE_URL, NODE_KEY, NODE_PIN, NODE_ID, NODE_PAIRING, calls, sip];
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &[], &names)?;
        for name in names {
            moqui_lib::env::clear_property(name);
        }
        // nothing more goes to that edge node: replication stops noting writes, and the outbox fails, kept for
        // inspection, rather than going to whichever edge node this node pairs with next (plans/archive/Edge-Node.md §5.2)
        if let Some(facade) = self.factory()?.entity() {
            moqui_core::edge::replica::disable(&facade);
            // the mail servers the wizard's Apply set to edge: send as they did before it (plans/archive/Edge-Node.md §5.7)
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

    fn serving(&self) -> bool {
        !self.role.takes_links() || self.server().is_ok()
    }

    fn front_status(&self) -> Option<LinkStatus> {
        lock(&self.front).as_ref().map(Dialer::status)
    }

    fn unpair_front(&self) -> Result<(), Error> {
        if lock(&self.front).is_none() {
            return Err(Error::new("this node is not paired with a front node"));
        }
        self.unpair_front_pairing()
    }

    fn clients(&self) -> Result<Vec<EdgeClientInfo>, Error> {
        self.server()?.clients()
    }

    fn revoke(&self, paired_node_id: &str) -> Result<bool, Error> {
        self.server()?.revoke(paired_node_id)
    }

    fn hosts_changed(&self) {
        let Ok(server) = self.server() else {
            return;
        };
        // the names given to the front change with them (plans/Edge-Node2.md §4.7)
        self.names.notify_one();
        match self.factory() {
            Ok(factory) => {
                if let Err(err) = server.hosts.changed(&factory) {
                    tracing::error!("Edge node: the host table was not loaded again after a change: {err}");
                }
            }
            Err(err) => tracing::error!("Edge node: the host table was not loaded again after a change: {err}"),
        }
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
        let (edge, front) = self.up_clients()?;
        let (edge, front) = match (edge, front) {
            (Some(client), None) | (None, Some(client)) => {
                return crate::edge::block_on(async move { super::dial::offer_on(client, OFFER_TIMEOUT).await })?
                    .map_err(Error::new);
            }
            (Some(edge), Some(front)) => (edge, front),
            (None, None) => return Err(Error::new("this node is not paired with an edge node")),
        };
        // with two pairings each is asked, and what answered is shown (plans/Edge-Node2.md §4.7): the edge pairing's
        // offer with the front's web access and ports
        let (edge, front) = crate::edge::block_on(async move {
            tokio::join!(
                super::dial::offer_on(edge, OFFER_TIMEOUT),
                super::dial::offer_on(front, OFFER_TIMEOUT)
            )
        })?;
        match (edge, front) {
            (Ok(edge), Ok(front)) => Ok(super::front::merge_offer(edge, front)),
            (Ok(edge), Err(err)) => {
                tracing::warn!("The front node's offer was not read, so the edge node's is shown alone: {err}");
                Ok(edge)
            }
            (Err(err), Ok(front)) => {
                tracing::warn!("The edge node's offer was not read, so the front node's is shown alone: {err}");
                Ok(front)
            }
            (Err(edge), Err(front)) => Err(Error::new(format!("the edge node: {edge}; the front node: {front}"))),
        }
    }

    fn configure(&self, setup: &EdgeSetup) -> Result<Configured, Error> {
        let (edge, front) = self.up_clients()?;
        let (edge, front) = match (edge, front) {
            // with one pairing, all to it: a front refuses what it has not
            (None, Some(front)) => {
                let setup = setup.clone();
                return crate::edge::block_on(async move {
                    super::dial::configure_on(front, &setup, CALL_TIMEOUT, true).await
                })?
                .map_err(Error::new);
            }
            (Some(edge), None) => {
                let setup = setup.clone();
                let configured = crate::edge::block_on(async move {
                    super::dial::configure_on(edge, &setup, CALL_TIMEOUT, false).await
                })?
                .map_err(Error::new)?;
                self.note_configured(&configured);
                return Ok(configured);
            }
            (Some(edge), Some(front)) => (edge, front),
            (None, None) => return Err(Error::new("this node is not paired with an edge node")),
        };
        // with two pairings each gets its part, both sent; a part a pairing did not take is answered item by item with
        // why, and the rest stands (plans/Edge-Node2.md §4.7)
        let (front_part, edge_part) = super::front::split_setup(setup);
        let (sent_front, sent_edge) = (front_part.clone(), edge_part.clone());
        let (edge_answer, front_answer) = crate::edge::block_on(async move {
            tokio::join!(
                super::dial::configure_on(edge, &sent_edge, CALL_TIMEOUT, false),
                super::dial::configure_on(front, &sent_front, CALL_TIMEOUT, true)
            )
        })?;
        if let (Err(edge), Err(front)) = (&edge_answer, &front_answer) {
            return Err(Error::new(format!("the edge node: {edge}; the front node: {front}")));
        }
        if let Ok(configured) = &edge_answer {
            self.note_configured(configured);
        }
        let mut failed = Vec::new();
        if let Err(why) = &edge_answer {
            tracing::warn!("The edge node did not take its part of the setup: {why}");
            failed.extend(super::front::failed_results(&edge_part, "edge node", why));
        }
        if let Err(why) = &front_answer {
            tracing::warn!("The front node did not take its part of the setup: {why}");
            failed.extend(super::front::failed_results(&front_part, "front node", why));
        }
        let mut configured = super::front::merge_configured(edge_answer.ok(), front_answer.ok());
        configured.hosts.extend(failed);
        Ok(configured)
    }

    fn xmpp_probe(&self, port: u16, direct: bool) -> Result<String, Error> {
        let factory = self.factory()?;
        if moqui_core::edge::settings(&factory).takes_links() {
            return Err(Error::new("this is an edge node: its local nodes run the XMPP servers"));
        }
        // the XMPP ports are the front's when this node has one (plans/Edge-Node2.md §4.7)
        let url = self.web_url();
        crate::edge::block_on(super::probe::xmpp_probe(factory, url, port, direct))?.map_err(Error::new)
    }

    fn turn_probe(&self, url: &str, username: &str, credential: &str, peer: &str) -> Result<String, Error> {
        if moqui_core::edge::settings(&*self.factory()?).takes_links() {
            return Err(Error::new("this is an edge node: its local nodes check its TURN"));
        }
        super::probe::turn_probe(url, username, credential, peer).map_err(Error::new)
    }

    fn members(&self, paired_node_id: Option<&str>) -> Result<Vec<PoolMemberInfo>, Error> {
        self.server()?.members(paired_node_id)
    }

    fn balancing(&self, paired_node_id: Option<&str>) -> Result<moqui_core::edge::BalancingInfo, Error> {
        Ok(self.server()?.balancing(paired_node_id))
    }

    fn edge_cluster(&self) -> Result<Vec<moqui_core::edge::EdgeClusterMember>, Error> {
        self.server()?.edge_cluster()
    }

    fn drain_member(&self, paired_node_id: &str, instance_id: &str, drain: bool) -> Result<MemberState, Error> {
        let server = self.server()?;
        let (node, member) = (paired_node_id.to_string(), instance_id.to_string());
        crate::edge::block_on(async move { server.drain_member(&node, &member, drain).await })?.map_err(Error::new)
    }

    fn member_forgotten(&self, paired_node_id: &str, instance_id: &str) {
        if let Ok(server) = self.server() {
            if server.close_member(paired_node_id, instance_id) {
                tracing::warn!(
                    "Edge node: the link of member {instance_id} of {paired_node_id} is closed, its key forgotten"
                );
            }
            // and on the other members of an edge cluster (plans/Edge-Node2.md §4.6)
            if let Ok(factory) = self.factory() {
                super::accept::tell_links(
                    &factory,
                    moqui_core::edge::cluster_state::LinkOp::MemberForgotten {
                        node: paired_node_id.to_string(),
                        member: instance_id.to_string(),
                    },
                );
            }
        }
    }

    fn state_changed(&self) {
        for slot in [&self.dialer, &self.front] {
            if let Some(dialer) = lock(slot).as_ref() {
                dialer.state_changed();
            }
        }
    }

    fn ping_client(&self, paired_node_id: &str) -> Result<f64, Error> {
        let server = self.server()?;
        let node = paired_node_id.to_string();
        let rtt = crate::edge::block_on(async move { server.ping_local(&node).await })?.map_err(Error::new)?;
        Ok(rtt.as_secs_f64() * 1000.0)
    }

    fn ping_member(&self, paired_node_id: &str, instance_id: &str) -> Result<f64, Error> {
        let server = self.server()?;
        let (node, member) = (paired_node_id.to_string(), instance_id.to_string());
        let rtt =
            crate::edge::block_on(async move { server.ping_member(&node, &member).await })?.map_err(Error::new)?;
        Ok(rtt.as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;
