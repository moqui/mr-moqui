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

//! The edge link as the screens and services see it (plans/Edge-Node.md §5.7): the link's status, pairing and unpairing
//! on a local node, and on an edge node its pairing codes and paired local nodes. Core has no network stack; a host
//! built with the `edge` feature installs an [`EdgeLink`] (`crates/moqui-host/src/edge_link/`). With none installed a
//! call fails with [`NOT_BUILT`]. `org.moqui.impl.EdgeServices` (`crate::service::edge`) calls these.

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::sync::InstallSlot;

/// The message when no edge link is installed.
pub const NOT_BUILT: &str = "edge link not built into this host";

/// The property that makes a node an edge node: `edge`, or empty for a normal node (plans/Edge-Node.md §5.6).
pub const NODE_ROLE: &str = "node_role";
/// A local node's link properties, in `runtime/.env`, written by pairing and removed by unpairing (§5.1).
pub const NODE_URL: &str = "edge_node_url";
pub const NODE_KEY: &str = "edge_node_key";
pub const NODE_PIN: &str = "edge_node_pin";
/// An edge node's long-lived link key (a generated secret), which its up and down connections present instead of the
/// web certificate, so a renewed web certificate does not break a link.
pub const LINK_KEY: &str = "edge_link_key";

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
