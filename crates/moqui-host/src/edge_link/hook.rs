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

//! The edge link as `moqui_core::edge` sees it ([`EdgeLink`]), installed when the HTTP listeners start: the local node's
//! dialer (started when `edge_node_url` is set, and by pairing without a restart) and, on an edge node, its
//! [`EdgeServer`]. Pairing writes `edge_node_url`, `edge_node_key` and `edge_node_pin` to `runtime/.env` and sets them as
//! properties; unpairing removes them (plans/Edge-Node.md §5.1, §5.7).

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{
    EdgeClientInfo, EdgeInfo, EdgeLink, LinkStatus, PairResult, PairingCode, NODE_KEY, NODE_PIN, NODE_URL,
};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::sync::watch;

use super::accept::{Acceptor, EdgeServer};
use super::dial::{is_linked, Dialer};
use super::pair::{Identity, PairConfig};
use super::{EdgeConfig, LocalConfig};

/// How long pairing and checking wait for the edge node.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Install the edge link of this process: the hook, the dialer when this node is paired, and on an edge node the
/// listener's hand-off (`resolver` is the HTTPS listener's certificates; `None` when it has none). Called from
/// `http::listen` on its runtime.
pub(crate) fn start(
    factory: &Arc<ExecutionContextFactory>,
    resolver: Option<Arc<dyn rustls::server::ResolvesServerCert>>,
    shutdown: watch::Receiver<bool>,
) -> Option<Arc<Acceptor>> {
    let edge_node = moqui_core::edge::is_edge_node();
    let server = match (edge_node, resolver) {
        (true, Some(resolver)) => {
            let key = moqui_lib::env::get_prop_or_env(moqui_core::edge::LINK_KEY);
            match Identity::from_key(&key, "moqui-edge-link") {
                Ok(identity) => {
                    let server = EdgeServer::new(
                        factory,
                        EdgeConfig::from_properties(&instance_id(factory)),
                        &identity.fingerprint,
                        shutdown.clone(),
                    );
                    super::accept::acceptor(&server, resolver, &identity).map(|acceptor| (server, acceptor))
                }
                Err(err) => {
                    tracing::error!(
                        "node_role is edge, but {}: {err}; this edge node takes no links",
                        moqui_core::edge::LINK_KEY
                    );
                    None
                }
            }
        }
        (true, None) => {
            tracing::error!(
                "node_role is edge, but the HTTPS listener is off (no certificate in runtime/cert, or \
                 webapp_https_listen_port=off): this edge node takes no links"
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
    if !edge_node {
        let weak = Arc::downgrade(&hook);
        moqui_core::telemetry::for_factory(factory).observe_gauge("moqui.edgelink.up", move || {
            let hook = weak.upgrade()?;
            let linked = lock(&hook.dialer).as_ref().is_some_and(is_linked);
            Some(if linked { 1.0 } else { 0.0 })
        });
    }
    let paired = (!edge_node)
        .then(|| LocalConfig::from_properties(&instance_id(factory)))
        .flatten();
    if edge_node
        && !moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_URL)
            .trim()
            .is_empty()
    {
        tracing::warn!("edge_node_url is set on an edge node (node_role=edge): it does not dial an edge node itself");
    }
    if let Some(config) = paired {
        if let Err(err) = hook.start_dialer(config) {
            tracing::error!("The edge link is not started: {err}");
        }
    }
    moqui_core::edge::install_edge_link(Arc::clone(&hook) as Arc<dyn EdgeLink>);
    server.map(|(_, acceptor)| acceptor)
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
        if !moqui_core::edge::is_edge_node() {
            return Err(Error::new("this is not an edge node (node_role is not edge)"));
        }
        self.server
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| Error::new("this edge node takes no links: its HTTPS listener is off"))
    }

    fn start_dialer(&self, config: LocalConfig) -> Result<(), String> {
        let telemetry = self
            .factory
            .upgrade()
            .map(|factory| moqui_core::telemetry::for_factory(&factory));
        let dialer = Dialer::start(config, &self.runtime, self.shutdown.clone(), telemetry)?;
        // the dialer it replaces stops when dropped
        *lock(&self.dialer) = Some(dialer);
        Ok(())
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
        if moqui_core::edge::is_edge_node() {
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
        let set = [
            (NODE_URL.to_string(), url.trim().to_string()),
            (NODE_KEY.to_string(), paired.key.clone()),
            (NODE_PIN.to_string(), paired.pin.clone()),
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
        if !host.trim().is_empty() {
            local.node_name = host.trim().to_string();
        }
        self.start_dialer(local).map_err(Error::new)?;
        Ok(PairResult {
            edge: paired.edge,
            key_fingerprint: paired.key_fingerprint,
        })
    }

    fn unpair(&self) -> Result<(), Error> {
        let dialer = lock(&self.dialer).take();
        let told = match dialer {
            Some(dialer) => crate::edge::block_on(async move { dialer.unpair().await })?,
            None => Ok(()),
        };
        moqui_lib::env::update_dotenv_file(&self.env_path()?, &[], &[NODE_URL, NODE_KEY, NODE_PIN])?;
        for name in [NODE_URL, NODE_KEY, NODE_PIN] {
            moqui_lib::env::clear_property(name);
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

    fn ping_client(&self, paired_node_id: &str) -> Result<f64, Error> {
        let server = self.server()?;
        let node = paired_node_id.to_string();
        let rtt = crate::edge::block_on(async move { server.ping_local(&node).await })?.map_err(Error::new)?;
        Ok(rtt.as_secs_f64() * 1000.0)
    }
}
