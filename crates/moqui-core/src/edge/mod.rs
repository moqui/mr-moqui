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

//! The edge link as the screens and services see it (plans/archive/Edge-Node.md §5.2, §5.3, §5.7): the link's status, pairing
//! and unpairing on a local node, and on an edge node its pairing codes, paired local nodes and their public host
//! names. Core has no network stack; a host built with the `edge` feature installs an [`EdgeLink`]
//! (`crates/moqui-host/src/edge_link/`). With none installed a call fails with [`NOT_BUILT`].
//! `org.moqui.edge.EdgeServices` (`crate::service::edge`) calls these.
//!
//! How work crosses the link (§5.2) is here too, as far as it needs no network: the `<edge>` conf ([`conf`]), the
//! service facade's hand-off of delegated calls ([`delegate`]), the queue's rows, receipts and handlers ([`queue`]),
//! and replication's change notes, batches and copies ([`replica`]). The host's link moves them. Attaching a local node
//! (§5.7, E10) is [`setup`]: the edge node's offer, the local node's Configure, the local rows Apply writes and the
//! Test step's round trips; its calls area, on the edge node as media node, is [`calls`].

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::sync::InstallSlot;

pub mod calls;
pub mod claims;
pub mod conf;
pub mod delegate;
pub mod hosts;
pub mod media;
pub mod owner;
pub mod pool;
pub mod ports;
pub mod queue;
pub mod replica;
pub mod setup;
pub mod signal;
pub mod sip;

pub use conf::{settings, EdgeSettings, ReplicaEntity, ReplicaSet};
pub use pool::{BalanceMemberInfo, BalancingInfo, ClusterMemberInfo, MemberState, PoolMemberInfo};
pub use setup::{Configured, EdgeOffer, EdgeSetup};

/// The message when no edge link is installed.
pub const NOT_BUILT: &str = "edge link not built into this host";

/// The property that sets a node's role ([`NodeRole`]): `front`, `front_edge`, `edge`, `all-in-one`, or empty for a
/// normal node (plans/Edge-Node2.md §4.3).
pub const NODE_ROLE: &str = "node_role";
/// A local node's link properties, in `runtime/.env`, written by pairing and removed by unpairing (§5.1).
pub const NODE_URL: &str = "edge_node_url";
pub const NODE_KEY: &str = "edge_node_key";
pub const NODE_PIN: &str = "edge_node_pin";
/// The edge node's instance id, pinned at pairing beside its key (E7): the node its calls' media is on, which a link
/// whose Hello names another id is refused for, so an edge node cannot take another media node's id.
pub const NODE_ID: &str = "edge_node_id";
/// The pairing this node and the members of its cluster are known by on the edge node (plans/Edge-Node2.md §4.4): the
/// instance id of the member that paired, written beside the key at pairing. The edge node keys its rows by it and
/// the local node its queue rows and receipts, so every member of a pool, and a standby that takes over, is the same
/// pairing. A node paired before E17 has none, and is its own instance id.
pub const NODE_PAIRING: &str = "edge_node_pairing";
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

/// A node's role (`node_role`, plans/Edge-Node2.md §4.3, decision 16): which of the two option sets it has. The
/// **front** set faces browsers: links taken, web access by host name, TCP ports, balancing, web protection and
/// certificates. The **edge** set is the rest of an edge node: delegate, queue and replicate, the mail edge, chat
/// webhooks, the media node and TURN, SIP, mr-pass's sites. Every role but a normal node's takes links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeRole {
    /// A normal node (`node_role` empty): its own data, and a local node when paired with an edge node.
    #[default]
    Normal,
    /// A front node (`node_role=front`, E19 finishes it): the front set alone. Its Hello offers only web access and
    /// ports.
    Front,
    /// An edge node as built before E17 (`node_role=front_edge`): both sets, so it balances a local cluster with no
    /// other node. No business data of its own.
    FrontEdge,
    /// An edge node behind a front node (`node_role=edge`): the edge set alone. It takes its local nodes' links for
    /// the queue, delegate, replicate, comms and PASS, and forwards no web access itself; its sites and ports are
    /// reached through the front.
    Edge,
    /// An all-in-one node (`node_role=all-in-one`, E12): a full server with both sets, which also takes the links of
    /// paired local nodes and routes each item by its owner ([`owner`]).
    AllInOne,
}

impl NodeRole {
    /// The role of a `node_role` value, trimmed and in any case. An unknown value is logged as an error (once per
    /// process) and means a normal node.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "normal" => Self::Normal,
            "front" => Self::Front,
            "front_edge" => Self::FrontEdge,
            "edge" => Self::Edge,
            "all-in-one" => Self::AllInOne,
            other => {
                static LOGGED: std::sync::Once = std::sync::Once::new();
                LOGGED.call_once(|| {
                    tracing::error!(
                        "{NODE_ROLE}={other} is not a node role (front, front_edge, edge, all-in-one, or empty); this \
                         is a normal node"
                    );
                });
                Self::Normal
            }
        }
    }

    /// The `node_role` value of the role: empty for a normal node.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Front => "front",
            Self::FrontEdge => "front_edge",
            Self::Edge => "edge",
            Self::AllInOne => "all-in-one",
        }
    }

    /// A dedicated edge node of any set (`front`, `front_edge` or `edge`): no business data of its own, so every name,
    /// address, number and bridge not its own is a paired node's, and some paths switch as a whole.
    pub fn edge_node(self) -> bool {
        matches!(self, Self::Front | Self::FrontEdge | Self::Edge)
    }

    /// An all-in-one node: a full server that also takes a paired node's link.
    pub fn all_in_one(self) -> bool {
        self == Self::AllInOne
    }

    /// The front set: web access by host name, TCP ports, balancing (plans/Edge-Node2.md §4.3).
    pub fn has_front(self) -> bool {
        matches!(self, Self::Front | Self::FrontEdge | Self::AllInOne)
    }

    /// The edge set: delegate, queue and replicate, mail, chat, calls, SIP and PASS (plans/Edge-Node2.md §4.3).
    pub fn has_edge(self) -> bool {
        matches!(self, Self::FrontEdge | Self::Edge | Self::AllInOne)
    }

    /// The edge end of a link: every role but a normal node's. It accepts links, runs the [`queue`]'s receiving side
    /// and sends the listed services' calls of a paired node's items down the link.
    pub fn takes_links(self) -> bool {
        self != Self::Normal
    }
}

/// This node's role, from `node_role` as it is now. A factory keeps the role it started with in
/// [`EdgeSettings::role`]; code with a factory reads that.
pub fn node_role_kind() -> NodeRole {
    NodeRole::parse(&node_role())
}

/// Whether this node is a dedicated edge node (`front`, `front_edge` or `edge`).
pub fn is_edge_node() -> bool {
    node_role_kind().edge_node()
}

/// Whether this node takes links: any role but a normal node's.
pub fn takes_links() -> bool {
    node_role_kind().takes_links()
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
    /// Its SIP engine's address for UDP and TCP, and for TLS; empty when off (E8). The local node keeps the edge
    /// node's `SipNode` row from them.
    pub sip_address: String,
    pub sip_tls_address: String,
    /// Its registrar's realm, and whether its registrar is on (E8): the local node's phones' verifiers are for that
    /// realm, and its phones ring only with the registrar on.
    pub sip_domain: String,
    pub sip_registrar: bool,
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
    /// Its members linked now (plans/Edge-Node2.md §4.4): each links with the pairing's key; `generation` is the
    /// newest of theirs.
    pub members_linked: u32,
    /// The member last linked (`lastMemberId`), the node that paired before any.
    pub last_member_id: String,
    /// A new node taking the pairing over is held for approval (`holdNewMember`, §4.5), and the one approved.
    pub hold_new_member: bool,
    pub approved_member_id: String,
    /// The last takeover by a new node (§4.5): when, from which member, by which.
    pub takeover_date: Option<i64>,
    pub takeover_from_id: String,
    pub takeover_to_id: String,
    /// Its own size cap on the items waiting for it, in megabytes (`queueMaxMegabytes`); `None` for the conf's.
    pub queue_max_megabytes: Option<i64>,
}

/// A public host name an edge node answers for a paired local node (`moqui.edge.EdgeHost`, §5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeHostInfo {
    /// Lower case, with no port or trailing dot; unique across the paired nodes.
    pub host_name: String,
    pub paired_node_id: String,
    /// [`HOST_PASSTHROUGH`] or [`HOST_TERMINATE`].
    pub mode_enum_id: String,
    /// The cookie a terminate host's requests are sticky by (plans/Edge-Node2.md §4.4), a cookie name; empty for the
    /// conf's `<edge sticky-cookie>`. On a save, `None` leaves the row's as it is (a Configure's host names).
    pub sticky_cookie: Option<String>,
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
    /// How long the edge node waits for the answer, when its conf entry sets one shorter than the link's (E8).
    pub timeout: Option<std::time::Duration>,
    /// The paired node to run it on (its `pairedNodeId`); `None` for the only linked one, as a dedicated edge node
    /// sends. An all-in-one node names the owner of the item ([`owner::Owner::Paired`], E12). Not sent down the link.
    pub node: Option<String>,
}

/// The local node's answer to a [`DelegateCall`]: the service's result, with the errors it added (a refusal's
/// `reasonCode` comes back beside its error, as a call on the node itself returns both).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DelegateAnswer {
    pub result: serde_json::Value,
    pub errors: Vec<String>,
}

/// The edge link of this process, installed by the host.
pub trait EdgeLink: Send + Sync {
    /// Whether it serves links now: false on a node that takes links whose HTTPS listener is off (the link is
    /// installed, with no server), so the screens show the rows and the defaults in place of what is in memory.
    fn serving(&self) -> bool {
        true
    }
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
    /// On an edge node: the members of each pairing, of one when `paired_node_id` is given (plans/Edge-Node2.md §4.4),
    /// from the links, the latest member lists and the nodes refused or held lately.
    fn members(&self, paired_node_id: Option<&str>) -> Result<Vec<PoolMemberInfo>, Error> {
        let _ = paired_node_id;
        Err(Error::new(NOT_BUILT))
    }
    /// On an edge node: balancing's defaults and each member's traffic and set-aside (plans/Edge-Node2.md §4.10
    /// Balancing), of one pairing when `paired_node_id` is given; in memory only.
    fn balancing(&self, paired_node_id: Option<&str>) -> Result<BalancingInfo, Error> {
        let _ = paired_node_id;
        Err(Error::new(NOT_BUILT))
    }
    /// On an edge node: ask a linked member down its link to drain (`drain`) or to come back; its state after. The
    /// state is the member's: nothing is stored here.
    fn drain_member(&self, paired_node_id: &str, instance_id: &str, drain: bool) -> Result<MemberState, Error> {
        let _ = (paired_node_id, instance_id, drain);
        Err(Error::new(NOT_BUILT))
    }
    /// On an edge node: a Ping down the down connection of member `instance_id` of pairing `paired_node_id`, in
    /// milliseconds (Clients' Ping on a member row, plans/Edge-Node2.md §4.10).
    fn ping_member(&self, paired_node_id: &str, instance_id: &str) -> Result<f64, Error> {
        let _ = (paired_node_id, instance_id);
        Err(Error::new(NOT_BUILT))
    }
    /// On a local node: its state changed (drained or resumed here), so its links tell the edge node now rather than
    /// at the next Ping.
    fn state_changed(&self) {}
    /// On an edge node: the key pinned for member `instance_id` of pairing `paired_node_id` was forgotten, so close its
    /// link: its next one is admitted afresh (plans/Edge-Node2.md §4.11).
    fn member_forgotten(&self, paired_node_id: &str, instance_id: &str) {
        let _ = (paired_node_id, instance_id);
    }
    /// On a node that takes links: the host name rows ([`hosts`]) changed, so load the table in memory again. Nothing
    /// while this node takes no links; a table that does not load is logged.
    fn hosts_changed(&self) {}
    /// On an edge node: run `call` on its local node ([`DelegateCall::node`], else the only one linked) and wait for its
    /// answer; [`OFFLINE`] at once while that node's link is down. The caller checks the allowlist, and the local node
    /// checks its own again.
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
    /// On a local node, the Test step's TURN check (§5.7, E10 calls): an allocation on the edge node's TURN at `url`
    /// (a `turn:` URL over UDP) with `username` and `credential`, as a browser's, then a permission for the IP of `peer` (its media
    /// address), and the allocation deleted; what was allocated, or why not.
    fn turn_probe(&self, url: &str, username: &str, credential: &str, peer: &str) -> Result<String, Error> {
        let _ = (url, username, credential, peer);
        Err(Error::new(NOT_BUILT))
    }
}

static LINK: InstallSlot<dyn EdgeLink> = InstallSlot::new();

/// Use `link` for the edge link in this process.
pub fn install_edge_link(link: Arc<dyn EdgeLink>) {
    LINK.install(link);
}

/// The installed link while it serves links ([`EdgeLink::serving`]); `None` with none installed or its listener off.
pub fn serving_link() -> Option<Arc<dyn EdgeLink>> {
    link().ok().filter(|link| link.serving())
}

/// Remove the installed link.
pub fn clear_edge_link() {
    LINK.clear();
}

/// The installed link, or [`NOT_BUILT`].
pub fn link() -> Result<Arc<dyn EdgeLink>, Error> {
    LINK.get().ok_or_else(|| Error::new(NOT_BUILT))
}

#[cfg(test)]
mod tests;
