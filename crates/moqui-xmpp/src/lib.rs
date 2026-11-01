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

//! XMPP for local users (plans/Comms-Framework.md §4.3 "XMPP server (C3)", C3a; docs/interfaces/XMPP.md).
//!
//! - [`framing`] reads a client's stream (TCP: one XML document with a stanza per child; WebSocket, RFC 7395: one
//!   element per frame) into [`minidom::Element`]s, and writes them.
//! - [`scram`] and [`fast`] are the sign-in mechanisms: SCRAM-SHA-256 with and without tls-exporter channel binding
//!   (RFC 5802, RFC 7677, RFC 9266, XEP-0440), and FAST tokens (XEP-0484).
//! - [`session`] is one client's stream: features, SASL and SASL2 (XEP-0388) with Bind 2 (XEP-0386), stream management
//!   (XEP-0198), and the stanzas, which it maps onto Moqui chat through a [`backend::Backend`] by service name.
//! - [`registry`] holds this node's sessions by user, routes the notification bus's chat pushes to them, and keeps a
//!   detached session for resumption.
//! - [`conn`] runs a session over a TCP connection (STARTTLS or direct TLS) or WebSocket frames.
//!
//! What is advertised is one list ([`features`]), so service discovery and the docs' XEP-0479 claims cannot drift
//! apart from what the session answers.

pub mod backend;
pub mod conn;
pub mod fast;
pub mod features;
pub mod framing;
pub mod jids;
pub mod ns;
pub mod registry;
pub mod scram;
pub mod session;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::source_guard::{GuardPolicy, SourceGuard};

/// The settings of the server, from the host's properties.
#[derive(Debug, Clone)]
pub struct Config {
    /// The domain of user JIDs, `user@domain` (`xmpp_domain`).
    pub domain: String,
    /// The MUC service, `rooms.<domain>` by default.
    pub rooms_domain: String,
    /// The HTTP upload service (XEP-0363), `upload.<domain>` by default.
    pub upload_domain: String,
    /// The HTTPS base of upload slots, such as `https://chat.example.com` (no trailing slash).
    pub upload_base_url: String,
    /// The largest upload, in bytes.
    pub upload_max_bytes: u64,
    /// How long a detached session waits for resumption.
    pub resume_timeout: Duration,
    /// The longest stanza a client may send, in bytes.
    pub max_stanza_bytes: usize,
    /// How long a FAST token lasts.
    pub fast_token_lifetime: Duration,
    /// The version answered to XEP-0092 software version queries; empty answers none.
    pub software_version: String,
}

impl Config {
    /// The defaults for `domain`: rooms and upload as subdomains, and an upload base on the domain over HTTPS.
    pub fn for_domain(domain: &str) -> Config {
        let domain = domain.trim().to_lowercase();
        Config {
            rooms_domain: format!("rooms.{domain}"),
            upload_domain: format!("upload.{domain}"),
            upload_base_url: format!("https://{domain}"),
            upload_max_bytes: 20 * 1024 * 1024,
            resume_timeout: Duration::from_secs(300),
            max_stanza_bytes: 256 * 1024,
            fast_token_lifetime: Duration::from_secs(14 * 24 * 3600),
            software_version: String::new(),
            domain,
        }
    }
}

/// What every session of the server shares.
pub struct Server {
    pub config: Config,
    pub backend: Arc<dyn backend::Backend>,
    pub registry: registry::Registry,
    /// Failed sign-ins per source address (moqui-lib's guard, as the SIP engine counts failed digests). An account is
    /// never locked by them.
    pub guard: Mutex<SourceGuard>,
    /// Stanzas to other servers (push notifications to app servers).
    pub outbound: Box<dyn Outbound>,
}

impl Server {
    pub fn new(config: Config, backend: Arc<dyn backend::Backend>) -> Arc<Server> {
        Server::with_outbound(config, backend, Box::new(NoFederation))
    }

    pub fn with_outbound(
        config: Config,
        backend: Arc<dyn backend::Backend>,
        outbound: Box<dyn Outbound>,
    ) -> Arc<Server> {
        Arc::new(Server {
            config,
            backend,
            registry: registry::Registry::default(),
            guard: Mutex::new(SourceGuard::new(GuardPolicy::default())),
            outbound,
        })
    }
}

/// Where a stanza for another server goes. Server-to-server is C3b; until it is built nothing leaves.
pub trait Outbound: Send + Sync {
    fn send(&self, stanza: minidom::Element) -> Result<(), String>;
}

/// No federation: a stanza for another server is not sent.
pub struct NoFederation;

impl Outbound for NoFederation {
    fn send(&self, _stanza: minidom::Element) -> Result<(), String> {
        Err("server-to-server (federation, C3b) is not built".to_string())
    }
}
