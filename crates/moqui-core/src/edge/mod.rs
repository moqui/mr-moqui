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

//! The edge link as the screens and services see it (plans/Edge-Node.md §5.2, §5.3, §5.7): the link's status, pairing
//! and unpairing on a local node, and on an edge node its pairing codes, paired local nodes and their public host
//! names. Core has no network stack; a host built with the `edge` feature installs an [`EdgeLink`]
//! (`crates/moqui-host/src/edge_link/`). With none installed a call fails with [`NOT_BUILT`].
//! `org.moqui.impl.EdgeServices` (`crate::service::edge`) calls these.
//!
//! How work crosses the link (§5.2) is here too, as far as it needs no network: the `<edge>` conf ([`conf`]), the
//! service facade's hand-off of delegated calls ([`delegate`]), the queue's rows, receipts and handlers ([`queue`]),
//! and replication's change notes, batches and copies ([`replica`]). The host's link moves them. Attaching a local node
//! (§5.7, E10) is [`setup`]: the edge node's offer, the local node's Configure, the local rows Apply writes and the
//! Test step's round trips.

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::sync::InstallSlot;

pub mod conf;
pub mod delegate;
pub mod media;
pub mod queue;
pub mod replica;
pub mod setup;
pub mod signal;

pub use conf::{settings, EdgeSettings, ReplicaEntity, ReplicaSet};
pub use setup::{Configured, EdgeOffer, EdgeSetup};

/// The message when no edge link is installed.
pub const NOT_BUILT: &str = "edge link not built into this host";

/// The property that makes a node an edge node: `edge`, or empty for a normal node (plans/Edge-Node.md §5.6).
pub const NODE_ROLE: &str = "node_role";
/// A local node's link properties, in `runtime/.env`, written by pairing and removed by unpairing (§5.1).
pub const NODE_URL: &str = "edge_node_url";
pub const NODE_KEY: &str = "edge_node_key";
pub const NODE_PIN: &str = "edge_node_pin";
/// The edge node's instance id, pinned at pairing beside its key (E7): the node its calls' media is on, which a link
/// whose Hello names another id is refused for, so an edge node cannot take another media node's id.
pub const NODE_ID: &str = "edge_node_id";
/// An edge node's long-lived link key (a generated secret), which its up and down connections present instead of the
/// web certificate, so a renewed web certificate does not break a link.
pub const LINK_KEY: &str = "edge_link_key";

/// A host whose TLS the edge node passes through to the local node, which ends it with its own certificate (§5.3).
pub const HOST_PASSTHROUGH: &str = "EdgeHostPassthrough";
/// A host whose TLS the edge node ends, forwarding each request down the link as HTTP/2 (§5.3).
pub const HOST_TERMINATE: &str = "EdgeHostTerminate";

/// `node_role`, trimmed and lowercase.
pub fn node_role() -> String {
    moqui_lib::env::get_prop_or_env(NODE_ROLE).trim().to_ascii_lowercase()
}

/// Whether this node is an edge node (`node_role=edge`).
pub fn is_edge_node() -> bool {
    node_role() == "edge"
}

/// The state of a local node's link.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    /// No `edge_node_url`: not paired.
    #[default]
    Unpaired,
    /// Dialing, or between the TLS handshake and Hello.
    Connecting,
    /// Up and down connections both open.
    Linked,
    /// The last attempt failed; the next one waits for the backoff.
    Retrying,
    /// The edge node refused this node's key (revoked, or never paired), or the versions are too far apart.
    Refused,
}

impl LinkState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unpaired => "unpaired",
            Self::Connecting => "connecting",
            Self::Linked => "linked",
            Self::Retrying => "retrying",
            Self::Refused => "refused",
        }
    }
}

/// What an edge node says of itself (its Hello), with its certificate's fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeInfo {
    pub name: String,
    /// SHA-256 of the edge node's public key, hex: what pairing pins as `edge_node_pin`.
    pub fingerprint: String,
    pub binary_version: String,
    pub link_version: u32,
    pub features: Vec<String>,
    pub capabilities: Vec<String>,
    pub public_names: Vec<String>,
    /// The edge node's instance id: the node its calls' media is on (`CallMediaNode.instanceId`) and what
    /// [`signal`] routes a message for it by (E7).
    pub instance_id: String,
    /// The address browsers send the media of its calls to; empty when its media socket is off (E7).
    pub media_address: String,
    /// Its embedded TURN listeners as `turn:` and `turns:` URLs; empty without TURN (E7).
    pub turn_urls: Vec<String>,
    /// When its media socket started (epoch milliseconds), 0 when it is off (E7).
    pub media_started_ms: i64,
    /// The link versions are close enough to link.
    pub compatible: bool,
    /// Why not, when not compatible.
    pub message: String,
}

/// A local node's link, for the status screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkStatus {
    pub state: LinkState,
    pub url: String,
    pub generation: u64,
    /// The last round trip of a Ping on the up connection.
    pub rtt_ms: Option<f64>,
    /// When the link last came up (epoch milliseconds).
    pub last_connect: Option<i64>,
    pub last_error: String,
    /// The edge node, as its last Hello said, and the pinned fingerprint.
    pub peer: EdgeInfo,
    /// Links lost and dialed again since this process started.
    pub reconnects: u64,
    /// On an edge node, the fingerprint of its own link key (SHA-256 of the public key, hex), for the operator to
    /// compare with what a local node's check shows; empty on any other node.
    pub link_fingerprint: String,
}

/// What pairing pinned, for the operator to confirm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairResult {
    pub edge: EdgeInfo,
    /// The fingerprint of this node's new key, as the edge node pinned it.
    pub key_fingerprint: String,
}

/// A new pairing code; only its hash is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairingCode {
    pub pairing_code_id: String,
    pub code: String,
    pub expire_date: i64,
}

/// A paired local node, on the edge node (`moqui.edge.EdgeClient`, with its live link).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeClientInfo {
    pub paired_node_id: String,
    pub key_fingerprint: String,
    pub status_id: String,
    pub linked: bool,
    /// The down connection is open too.
    pub down: bool,
    pub generation: u64,
    pub last_connect: Option<i64>,
    pub remote_address: String,
    pub binary_version: String,
    pub link_version: u32,
    /// What the node's last Configure set besides its host names (a JSON object: mailDomains, chatBridgeIds,
    /// passSites), empty before one.
    pub setup_json: String,
    /// When its last Configure came.
    pub setup_date: Option<i64>,
}

/// A public host name an edge node answers for a paired local node (`moqui.edge.EdgeHost`, §5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeHostInfo {
    /// Lower case, with no port or trailing dot; unique across the paired nodes.
    pub host_name: String,
    pub paired_node_id: String,
    /// [`HOST_PASSTHROUGH`] or [`HOST_TERMINATE`].
    pub mode_enum_id: String,
}

/// The error of a delegated call while the link to the local node is down.
pub const OFFLINE: &str = "local node offline";

/// A service call an edge node sends to its local node (§5.2): the parameters and the caller's identity (username and
/// userId) as JSON data, never a login.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DelegateCall {
    pub service: String,
    pub parameters: serde_json::Value,
    pub caller: serde_json::Value,
    /// An id for the call, in both nodes' logs.
    pub operation_id: String,
}

/// The local node's answer to a [`DelegateCall`]: the service's result, or the errors it added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DelegateAnswer {
    pub result: serde_json::Value,
    pub errors: Vec<String>,
}

/// The edge link of this process, installed by the host.
pub trait EdgeLink: Send + Sync {
    /// The local node's link (unpaired on a node with no `edge_node_url`).
    fn status(&self) -> LinkStatus;
    /// Connect to the edge node at `url`, check its certificate by WebPKI, and return its Hello and fingerprint,
    /// without pairing.
    fn check(&self, url: &str) -> Result<EdgeInfo, Error>;
    /// Pair with the edge node at `url` using its one-use `code`: make this node's key, have the edge node pin it, pin
    /// the edge node's (refused when `expect_fingerprint` is given and differs), write `edge_node_*` to `runtime/.env`
    /// and bring the link up.
    fn pair(&self, url: &str, code: &str, expect_fingerprint: Option<&str>) -> Result<PairResult, Error>;
    /// Tell the edge node to revoke this node's key (when it can be reached), close the link and remove `edge_node_*`.
    fn unpair(&self) -> Result<(), Error>;
    /// On an edge node: a one-use pairing code good for `valid_minutes`.
    fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, Error>;
    /// On an edge node: the paired local nodes.
    fn clients(&self) -> Result<Vec<EdgeClientInfo>, Error>;
    /// On an edge node: revoke a local node's key and close its link. False when there was no such node.
    fn revoke(&self, paired_node_id: &str) -> Result<bool, Error>;
    /// On an edge node: a Ping down the local node's down connection, in milliseconds.
    fn ping_client(&self, paired_node_id: &str) -> Result<f64, Error>;
    /// On an edge node: the public host names of the paired nodes, of one node when `paired_node_id` is given.
    fn hosts(&self, paired_node_id: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error>;
    /// On an edge node: add a host name for a paired node, or change the mode of one it has. A name another node has
    /// is refused. The answer has the name as it was saved.
    fn add_host(&self, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error>;
    /// On an edge node: remove a host name. False when there was none.
    fn remove_host(&self, host_name: &str) -> Result<bool, Error>;
    /// On an edge node: run `call` on the local node (the only one linked) and wait for its answer; [`OFFLINE`] at once
    /// while the link is down. The caller checks the allowlist, and the local node checks its own again.
    fn delegate(&self, call: &DelegateCall) -> Result<DelegateAnswer, Error> {
        let _ = call;
        Err(Error::new(OFFLINE))
    }
    /// On a local node: what its edge node offers it now, asked on the up connection (§5.7, Discover).
    fn offer(&self) -> Result<EdgeOffer, Error> {
        Err(Error::new(NOT_BUILT))
    }
    /// On a local node: send the edge node its part of the setup (the link's Configure, §5.7 Apply); the answer has
    /// the offer after it and each host name's result.
    fn configure(&self, setup: &EdgeSetup) -> Result<Configured, Error> {
        let _ = setup;
        Err(Error::new(NOT_BUILT))
    }
    /// On a local node, the Test step's XMPP check (§5.7, E10): this node's XMPP server takes tunneled connections of
    /// the kind (direct TLS when `direct`, else STARTTLS), and a client stream opened to the edge node's tunnel `port`
    /// reaches it; what was reached, or why not. No credentials are sent.
    fn xmpp_probe(&self, port: u16, direct: bool) -> Result<String, Error> {
        let _ = (port, direct);
        Err(Error::new(NOT_BUILT))
    }
}

static LINK: InstallSlot<dyn EdgeLink> = InstallSlot::new();

/// Use `link` for the edge link in this process.
pub fn install_edge_link(link: Arc<dyn EdgeLink>) {
    LINK.install(link);
}

/// Remove the installed link.
pub fn clear_edge_link() {
    LINK.clear();
}

/// The installed link, or [`NOT_BUILT`].
pub fn link() -> Result<Arc<dyn EdgeLink>, Error> {
    LINK.get().ok_or_else(|| Error::new(NOT_BUILT))
}
