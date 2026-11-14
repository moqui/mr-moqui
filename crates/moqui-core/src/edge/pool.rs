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

//! Pools of local nodes and failover to a standby (plans/Edge-Node2.md §4.4, §4.5), as far as they need no network.
//!
//! A pairing (`moqui.edge.EdgeClient`) is one local node or one cluster of them; every member links with the
//! pairing's key and names its own instance id. On a local node this is the member's state on its links
//! ([`member_state`], drained by [`set_drained`]) and its cluster's member list ([`cluster_members`]), which it
//! sends with its Hello, every Ping, and when the edge node asks. On the edge node it is what the host's registry
//! decides with: the member list checked as input ([`check_member_list`]), whether a member not linked is admitted
//! ([`admission`]), and the report of each pairing's members for System › Edge Clients ([`report`]). Nothing here is
//! stored on the edge node: the member list and the states are kept in memory and come back with the first link.

use moqui_lib::error::Error;

use crate::context::ExecutionContextFactory;

/// The most members a local node's list may name; a longer list is not taken.
pub const MAX_CLUSTER_MEMBERS: usize = 64;
/// The longest instance id: the `id` type of `moqui.server.ClusterMember.instanceId`.
const MAX_ID: usize = 40;
/// The longest version text taken from a member list.
const MAX_VERSION: usize = 40;

/// The refusal of a node with a pairing's key that its linked members do not list as a live member of their cluster.
pub const JOIN_FIRST: &str = "join the cluster first: this pairing's linked members do not list this node as a live \
                              member of their cluster (plans/Edge-Node2.md §4.4)";
/// The refusal of the node a takeover displaced (`EdgeClient.takeoverFromId`), until the operator clears it: so the
/// link never goes back and forth between two copies of the data (plans/Edge-Node2.md §4.5).
pub const DISPLACED: &str = "taken over: another node took this pairing over from this one; its operator clears the \
                             takeover on the edge node first (System › Edge Clients)";
/// The most member keys a pairing keeps pinned; the oldest goes first.
pub const MAX_PINNED: usize = 64;

/// The refusal of a new node taking over a pairing that holds new nodes for approval, until it is approved.
pub const HELD_FOR_APPROVAL: &str = "held for approval: this node would take over its pairing, which the edge node's \
                                     operator approves first (System › Edge Clients)";

/// A linked member's state (plans/Edge-Node2.md §4.4, health): told on each of its links with its Hello and every
/// Ping. Only a live member gets new work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemberState {
    /// Serving; a member that says nothing (before E17) is live.
    #[default]
    Live,
    /// Its server has not finished starting.
    Starting,
    /// Its operator drained it: it finishes what it has and gets nothing new.
    Draining,
    /// Its server is stopping.
    Stopping,
}

impl MemberState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Starting => "starting",
            Self::Draining => "draining",
            Self::Stopping => "stopping",
        }
    }

    /// The state a link names; empty is live (a member before E17), anything else unknown is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "" | "live" => Some(Self::Live),
            "starting" => Some(Self::Starting),
            "draining" => Some(Self::Draining),
            "stopping" => Some(Self::Stopping),
            _ => None,
        }
    }

    /// Whether the member gets new work.
    pub fn takes_work(self) -> bool {
        self == Self::Live
    }
}

/// One member of a local node's cluster, as its list on the link says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterMemberInfo {
    pub instance_id: String,
    /// How long ago its heartbeat was written, in milliseconds, by its cluster database's clock.
    pub heartbeat_age_ms: u64,
    /// Its heartbeat is within its cluster's liveness window.
    pub live: bool,
    pub version: String,
}

/// Whether `id` can be a member's instance id: not empty, at most 40 bytes, no whitespace, control characters or `@`
/// (as the host's paired node ids).
pub fn valid_member_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(|ch| ch.is_whitespace() || ch.is_control() || ch == '@')
}

/// Check a member list a linked node sent, as input from any client (plans/Edge-Node2.md §4.11): at most
/// [`MAX_CLUSTER_MEMBERS`], each id a member id and named once, each version short and printable. A list that fails is
/// not taken whole; the reason names what failed.
pub fn check_member_list(members: &[ClusterMemberInfo]) -> Result<(), String> {
    if members.len() > MAX_CLUSTER_MEMBERS {
        return Err(format!(
            "the member list names {} members, more than {MAX_CLUSTER_MEMBERS}",
            members.len()
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(members.len());
    for member in members {
        if !valid_member_id(&member.instance_id) {
            return Err(format!(
                "the member list names an instance id that is empty, too long or has spaces, control characters or \
                 @: {}",
                member
                    .instance_id
                    .chars()
                    .take(MAX_ID)
                    .collect::<String>()
                    .escape_debug()
            ));
        }
        if !seen.insert(member.instance_id.as_str()) {
            return Err(format!("the member list names {} twice", member.instance_id));
        }
        if member.version.len() > MAX_VERSION || member.version.chars().any(char::is_control) {
            return Err(format!(
                "the version of member {} is too long or not text",
                member.instance_id
            ));
        }
    }
    Ok(())
}

/// What the edge node does with a link of a pairing's key from a member that is not linked (plans/Edge-Node2.md §4.4
/// Admission, §4.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// A member of the linked pool, or the pairing's own node coming back.
    Admit,
    /// A new node taking over the pairing with none linked: admitted, logged, shown and told to the administrators.
    /// `from` is the member that was last linked; when it is the member itself, this is that member with a new key (its
    /// key file made again, or forgotten by the operator), which displaces no one.
    TakeOver { from: String },
    /// Held until the operator approves it ([`HELD_FOR_APPROVAL`]).
    Hold { from: String },
    /// Refused, with what to tell it.
    Refuse(String),
}

/// What admission decides from.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmissionFacts<'a> {
    /// The member asking.
    pub member: &'a str,
    /// The pairing's members linked now.
    pub linked: &'a [String],
    /// With members linked, the list one of them gave when asked now (`None` when none answered).
    pub listed: Option<&'a [ClusterMemberInfo]>,
    /// The latest list the edge node kept of the pairing, from before its members went down (empty after a restart).
    pub known: &'a [ClusterMemberInfo],
    /// The member last linked (`EdgeClient.lastMemberId`), else the node that paired.
    pub last_member: &'a str,
    /// The pairing holds a new node for approval (`EdgeClient.holdNewMember`).
    pub hold: bool,
    /// The member the operator approved (`EdgeClient.approvedMemberId`).
    pub approved: &'a str,
    /// The member's key against the one pinned for its id (`EdgeClient.memberKeys`): `Some(true)` the same,
    /// `Some(false)` another (so another node naming that id), `None` none pinned yet.
    pub pinned: Option<bool>,
    /// The node the last takeover displaced (`EdgeClient.takeoverFromId`), until the operator clears it.
    pub displaced: &'a str,
    /// The operator forgot the key pinned for this id ([`forget_key`]): its next link is a new node's, never the last
    /// member or a member of the kept list coming back.
    pub forgotten: bool,
}

/// Admission (plans/Edge-Node2.md §4.4, §4.11). A member id is its own only with the key pinned for it: one named
/// with another key is a new node. A member linked already reconnects (the caller checks its old link no longer
/// answers). The node a takeover displaced is refused ([`DISPLACED`]) until its operator clears it. With members
/// linked, a new one is admitted only when the list a linked member gave has it as a live member, else refused with
/// [`JOIN_FIRST`]; with none linked the key admits it. A node with none linked that is neither a pinned member, nor
/// (with no key pinned for its id) the member last linked or a live member of the kept list, takes the pairing over
/// (§4.5): held while the pairing holds new nodes and it is not the one approved.
pub fn admission(facts: &AdmissionFacts<'_>) -> Admission {
    let member = facts.member;
    let other_key = facts.pinned == Some(false);
    if facts.linked.iter().any(|linked| linked == member) {
        return if other_key {
            Admission::Refuse(JOIN_FIRST.to_string())
        } else {
            Admission::Admit
        };
    }
    if !facts.displaced.is_empty() && facts.displaced == member {
        return Admission::Refuse(DISPLACED.to_string());
    }
    if !facts.linked.is_empty() {
        let listed = facts
            .listed
            .is_some_and(|list| list.iter().any(|row| row.instance_id == member && row.live));
        return if listed && !other_key {
            Admission::Admit
        } else {
            Admission::Refuse(JOIN_FIRST.to_string())
        };
    }
    let returning = match facts.pinned {
        Some(same) => same,
        None => {
            !facts.forgotten
                && (facts.last_member.is_empty()
                    || facts.last_member == member
                    || facts.known.iter().any(|row| row.instance_id == member && row.live))
        }
    };
    if returning {
        return Admission::Admit;
    }
    let from = facts.last_member.to_string();
    if facts.hold && facts.approved != member {
        Admission::Hold { from }
    } else {
        Admission::TakeOver { from }
    }
}

/// The member keys pinned for a pairing, oldest first, from `EdgeClient.memberKeys` (a JSON array of
/// `[instanceId, keyFingerprint]`); text that is not that is no pins, logged by the caller's read.
pub fn pinned_keys(json: &str) -> Vec<(String, String)> {
    let parsed: Vec<(String, String)> = serde_json::from_str(json).unwrap_or_default();
    parsed
}

/// The fingerprint kept for a member whose key the operator forgot ([`forget_key`]).
const FORGOTTEN: &str = "forgotten";

/// Whether `member`'s key `fingerprint` is the one pinned for it: `None` when none is, or it was forgotten.
pub fn key_matches(pins: &[(String, String)], member: &str, fingerprint: &str) -> Option<bool> {
    pins.iter()
        .find(|(id, pinned)| id == member && pinned != FORGOTTEN)
        .map(|(_, pinned)| pinned == fingerprint)
}

/// Whether the operator forgot `member`'s key ([`forget_key`]) and it has not linked since.
pub fn forgotten(pins: &[(String, String)], member: &str) -> bool {
    pins.iter().any(|(id, pinned)| id == member && pinned == FORGOTTEN)
}

/// `pins` with `member`'s key forgotten (`forget#EdgeMember`): its next link pins the key it brings, and is a new
/// node's, admitted only by its linked members' list or, with none linked, as a takeover. The JSON to keep.
pub fn forget_key(pins: &[(String, String)], member: &str) -> String {
    pin_key(pins, member, FORGOTTEN)
}

/// Pin `fingerprint` for `member` (trust on its first link, or again after a takeover), newest last, at most
/// [`MAX_PINNED`]: the JSON to keep. See [`pin_key_keeping`].
pub fn pin_key(pins: &[(String, String)], member: &str, fingerprint: &str) -> String {
    pin_key_keeping(pins, member, fingerprint, &[])
}

/// Pin `fingerprint` for `member`, newest last (an admitted link moves its member to the end, so recency is the last
/// link), at most [`MAX_PINNED`]. Past the cap a forgotten marker goes first, then the oldest member not in `keep`
/// (the members linked now, the member last linked and the kept list's: plans/Edge-Node2.md §4.11, so container churn
/// never evicts a stable member, whose id a stolen `.env` could then take quietly), then the oldest. The JSON to keep.
pub fn pin_key_keeping(pins: &[(String, String)], member: &str, fingerprint: &str, keep: &[String]) -> String {
    let mut pins: Vec<(String, String)> = pins.iter().filter(|(id, _)| id != member).cloned().collect();
    pins.push((member.to_string(), fingerprint.to_string()));
    while pins.len() > MAX_PINNED {
        let evicted = pins
            .iter()
            .position(|(id, pinned)| pinned == FORGOTTEN && id != member)
            .or_else(|| {
                pins.iter()
                    .position(|(id, _)| id != member && !keep.iter().any(|kept| kept == id))
            })
            .unwrap_or(0);
        pins.remove(evicted);
    }
    serde_json::to_string(&pins).unwrap_or_else(|_| "[]".to_string())
}

/// What claiming `member`'s key on its link does (plans/Edge-Node2.md §4.11): pin it when none is (or it was
/// forgotten), keep it when it matches, re-pin it on a decided takeover; otherwise the link is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyClaim {
    /// The JSON to keep: the key pinned, or moved to newest.
    Pin(String),
    /// Another key is pinned for the id and no takeover was decided.
    Refuse,
}

/// Claim `fingerprint` for `member` against the pins read in the same transaction that writes the answer (a compare and
/// set, so two first links of one id cannot both pin): see [`KeyClaim`]. `keep` as [`pin_key_keeping`].
pub fn claim_key(
    pins: &[(String, String)],
    member: &str,
    fingerprint: &str,
    takeover: bool,
    keep: &[String],
) -> KeyClaim {
    match key_matches(pins, member, fingerprint) {
        Some(false) if !takeover => KeyClaim::Refuse,
        _ => KeyClaim::Pin(pin_key_keeping(pins, member, fingerprint, keep)),
    }
}

/// `pins` without `member`'s: the JSON to keep.
pub fn unpin_key(pins: &[(String, String)], member: &str) -> String {
    let kept: Vec<&(String, String)> = pins.iter().filter(|(id, _)| id != member).collect();
    serde_json::to_string(&kept).unwrap_or_else(|_| "[]".to_string())
}

/// How a member of a pairing stands, for Clients (plans/Edge-Node2.md §4.4, "the edge node reports on the cluster").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberCondition {
    /// Live in its cluster and linked.
    Ok,
    /// Live in its cluster and not linked: a fault to show.
    NotLinked,
    /// Linked, with its heartbeat stale in its cluster's list, or not in it.
    StaleHeartbeat,
    /// On another version than most of its pool.
    OtherVersion,
    /// In its cluster's list with a stale heartbeat, and not linked: a member that stopped.
    Stopped,
    /// Held for approval as a new node taking over the pairing.
    Held,
    /// Refused lately: not in its linked members' cluster.
    Refused,
}

impl MemberCondition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NotLinked => "notLinked",
            Self::StaleHeartbeat => "staleHeartbeat",
            Self::OtherVersion => "otherVersion",
            Self::Stopped => "stopped",
            Self::Held => "held",
            Self::Refused => "refused",
        }
    }
}

/// A member linked now, as the edge node's registry has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkedMember {
    pub instance_id: String,
    pub generation: u64,
    /// Its down connection is open too.
    pub down: bool,
    pub remote_address: String,
    pub state: MemberState,
    /// The binary version its Hello said.
    pub binary_version: String,
    /// When its last Hello or Ping came (epoch milliseconds).
    pub last_seen: Option<i64>,
}

/// A node with the pairing's key that was refused or held lately (kept in memory, a few per pairing).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefusedMember {
    pub instance_id: String,
    pub remote_address: String,
    /// Epoch milliseconds.
    pub at: i64,
    pub message: String,
    /// Held for approval, not refused.
    pub held: bool,
}

/// One member of a pairing as balancing sees it (plans/Edge-Node2.md §4.4, §4.10 Balancing): its traffic and whether
/// it is set aside, kept in memory on the edge node from its first request to the next restart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalanceMemberInfo {
    pub paired_node_id: String,
    pub instance_id: String,
    /// Linked now with its down connection open.
    pub linked: bool,
    /// Its [`MemberState`] while linked; empty when not.
    pub state: String,
    /// Whether it gets new work now: linked, live and not set aside.
    pub takes_work: bool,
    /// Requests, calls and tunnel opens sent to it and not answered yet.
    pub in_flight: u64,
    /// Sent to it since the edge node started.
    pub requests: u64,
    /// Failed or timed out in a row, up to now.
    pub failures_in_row: u32,
    /// How many times it was set aside.
    pub set_asides: u64,
    /// Epoch milliseconds until which it is set aside; `None` when it is not.
    pub set_aside_until: Option<i64>,
}

/// Balancing on an edge node (plans/Edge-Node2.md §4.10 Balancing): the defaults from the `<edge>` conf, and each
/// member that is linked or was sent work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalancingInfo {
    /// The cookie a terminate host's requests are sticky by where the host names none.
    pub sticky_cookie: String,
    pub set_aside_failures: u32,
    pub set_aside_seconds: u64,
    pub members: Vec<BalanceMemberInfo>,
}

/// One member of a pairing for Clients: from the links, the latest member list and the refusals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolMemberInfo {
    pub paired_node_id: String,
    pub instance_id: String,
    pub linked: bool,
    pub down: bool,
    pub generation: u64,
    pub remote_address: String,
    /// Its [`MemberState`] while linked; empty when not.
    pub state: String,
    /// What its Hello said.
    pub binary_version: String,
    /// In the latest member list, with what it says.
    pub in_cluster: bool,
    pub live_in_cluster: bool,
    pub heartbeat_age_ms: Option<u64>,
    pub version: String,
    /// A [`MemberCondition`].
    pub condition: String,
    /// A refusal's text.
    pub message: String,
    /// Epoch milliseconds: its last Hello or Ping, or its last refusal.
    pub last_seen: Option<i64>,
}

/// The members of pairing `paired` (plans/Edge-Node2.md §4.4): each linked member, each member of the latest list
/// that is not linked (live: [`MemberCondition::NotLinked`], a fault), and each node refused or held lately that is not
/// linked now, in that order and each by id. A member whose version differs from most of the pool's is
/// [`MemberCondition::OtherVersion`].
pub fn report(
    paired: &str,
    linked: &[LinkedMember],
    list: Option<&[ClusterMemberInfo]>,
    refused: &[RefusedMember],
) -> Vec<PoolMemberInfo> {
    let in_list = |id: &str| list.and_then(|list| list.iter().find(|row| row.instance_id == id));
    let mut out: Vec<PoolMemberInfo> = Vec::new();
    let mut linked: Vec<&LinkedMember> = linked.iter().collect();
    linked.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    for member in linked {
        let row = in_list(&member.instance_id);
        let stale = list.is_some() && !row.is_some_and(|row| row.live);
        out.push(PoolMemberInfo {
            paired_node_id: paired.to_string(),
            instance_id: member.instance_id.clone(),
            linked: true,
            down: member.down,
            generation: member.generation,
            remote_address: member.remote_address.clone(),
            state: member.state.as_str().to_string(),
            binary_version: member.binary_version.clone(),
            in_cluster: row.is_some(),
            live_in_cluster: row.is_some_and(|row| row.live),
            heartbeat_age_ms: row.map(|row| row.heartbeat_age_ms),
            version: row.map_or_else(|| member.binary_version.clone(), |row| row.version.clone()),
            condition: if stale {
                MemberCondition::StaleHeartbeat
            } else {
                MemberCondition::Ok
            }
            .as_str()
            .to_string(),
            message: String::new(),
            last_seen: member.last_seen,
        });
    }
    let mut listed: Vec<&ClusterMemberInfo> = list.map(|list| list.iter().collect()).unwrap_or_default();
    listed.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    for row in listed {
        if out.iter().any(|member| member.instance_id == row.instance_id) {
            continue;
        }
        out.push(PoolMemberInfo {
            paired_node_id: paired.to_string(),
            instance_id: row.instance_id.clone(),
            in_cluster: true,
            live_in_cluster: row.live,
            heartbeat_age_ms: Some(row.heartbeat_age_ms),
            version: row.version.clone(),
            condition: if row.live {
                MemberCondition::NotLinked
            } else {
                MemberCondition::Stopped
            }
            .as_str()
            .to_string(),
            ..PoolMemberInfo::default()
        });
    }
    let mut attempts: Vec<&RefusedMember> = refused.iter().collect();
    attempts.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    for attempt in attempts {
        if let Some(member) = out
            .iter_mut()
            .find(|member| member.instance_id == attempt.instance_id && !member.linked)
        {
            // a live member of the list that is refused: its refusal says why it is not linked
            member.message.clone_from(&attempt.message);
            member.last_seen = Some(attempt.at);
            continue;
        }
        if out.iter().any(|member| member.instance_id == attempt.instance_id) {
            continue;
        }
        out.push(PoolMemberInfo {
            paired_node_id: paired.to_string(),
            instance_id: attempt.instance_id.clone(),
            remote_address: attempt.remote_address.clone(),
            condition: if attempt.held {
                MemberCondition::Held
            } else {
                MemberCondition::Refused
            }
            .as_str()
            .to_string(),
            message: attempt.message.clone(),
            last_seen: Some(attempt.at),
            ..PoolMemberInfo::default()
        });
    }
    // on another version: of the members linked or live in the list, the version most of them run
    if let Some(common) = common_version(&out) {
        for member in &mut out {
            let counted = member.linked || member.live_in_cluster;
            if counted && !member.version.is_empty() && member.version != common && member.condition == "ok" {
                member.condition = MemberCondition::OtherVersion.as_str().to_string();
            }
        }
    }
    out
}

/// The version most of the pool's linked or live members run (ties: the greatest), when there is one.
fn common_version(members: &[PoolMemberInfo]) -> Option<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for member in members {
        if (member.linked || member.live_in_cluster) && !member.version.is_empty() {
            *counts.entry(member.version.as_str()).or_default() += 1;
        }
    }
    let most = counts.values().copied().max()?;
    counts
        .into_iter()
        .rev()
        .find(|(_, count)| *count == most)
        .map(|(version, _)| version.to_string())
}

/// This member's own instance id: as its cluster names it, else as its telemetry does.
pub fn own_id(factory: &ExecutionContextFactory) -> String {
    crate::cluster::instance_id(factory).unwrap_or_else(|| crate::drain::instance_id(factory))
}

/// This member's state on its links (a local node's): draining while its operator drains it (the `InstanceDrain` row
/// of `crate::drain`, which `/health` answers 503 for too), starting until its server's startup report is done, else
/// live. Stopping is the host's, as its listeners stop.
pub fn member_state(factory: &ExecutionContextFactory) -> MemberState {
    if crate::drain::is_draining(factory) {
        MemberState::Draining
    } else if crate::startup::report(factory).ready().is_none() {
        MemberState::Starting
    } else {
        MemberState::Live
    }
}

/// This member's cluster as its links tell the edge node (plans/Edge-Node2.md §4.4): the `moqui.server.ClusterMember`
/// rows, at most [`MAX_CLUSTER_MEMBERS`], with this member always in it (a node with no cluster has no row). Rows that
/// cannot be read are logged, and the list is this member alone.
pub fn cluster_members(factory: &ExecutionContextFactory) -> Vec<ClusterMemberInfo> {
    let own = own_id(factory);
    let rows = match factory.entity() {
        Some(facade) => crate::cluster::member_report(factory, &facade).unwrap_or_else(|err| {
            tracing::error!("Edge link: this cluster's members were not read for the edge node: {err}");
            Vec::new()
        }),
        None => Vec::new(),
    };
    let mut members: Vec<ClusterMemberInfo> = rows
        .into_iter()
        .filter(|row| valid_member_id(&row.instance_id))
        .map(|row| ClusterMemberInfo {
            instance_id: row.instance_id,
            heartbeat_age_ms: row.heartbeat_age_ms,
            live: row.live,
            version: row.version.chars().take(MAX_VERSION).collect(),
        })
        .collect();
    if !own.is_empty() && !members.iter().any(|member| member.instance_id == own) {
        members.push(ClusterMemberInfo {
            instance_id: own,
            heartbeat_age_ms: 0,
            live: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        });
    }
    members.truncate(MAX_CLUSTER_MEMBERS);
    members
}

/// Drain this member, or bring it back (plans/Edge-Node2.md §4.4): its own `InstanceDrain` row, so `/health` answers
/// 503 too and the state holds across its links and a restart. The state after.
///
/// # Errors
///
/// When the row cannot be written.
pub fn set_drained(factory: &ExecutionContextFactory, drained: bool) -> Result<MemberState, Error> {
    let own = crate::drain::instance_id(factory);
    let reason = if drained {
        "drained from the edge link (System › Edge Node or the edge node's Edge Clients)"
    } else {
        "resumed from the edge link"
    };
    crate::drain::set_drained(factory, &own, drained, reason, None)?;
    Ok(member_state(factory))
}

/// The notification topic of a takeover (`framework/data/EdgeSetupData.xml`).
pub const TAKEOVER_TOPIC: &str = "EdgeMemberTakeover";
/// Who is told of a takeover: the edge node's administrators.
const ADMINISTRATORS: &str = "ADMIN";

/// Tell the edge node's administrators that `to` took pairing `paired` over from `from` with none of its members
/// linked (plans/Edge-Node2.md §4.5), or, `held`, that it is held for their approval. In the current context.
///
/// # Errors
///
/// When the notification cannot be sent.
pub fn notify_takeover(paired: &str, from: &str, to: &str, remote: &str, held: bool) -> Result<(), Error> {
    let mut message = moqui_lib::object::ValueMap::default();
    for (name, value) in [
        ("pairedNodeId", paired),
        ("fromInstanceId", from),
        ("toInstanceId", to),
        ("remoteAddress", remote),
    ] {
        message.insert(name.to_string(), moqui_lib::object::Value::string(value));
    }
    message.insert("held".to_string(), moqui_lib::object::Value::Bool(held));
    // plain text: an id holds no template
    let clean = |text: &str| text.replace(['$', '{', '}'], "");
    let title = if from == to {
        // the member last linked with a new key: no one is displaced, but its key is not the one pinned
        format!(
            "Edge pairing {}: member {} came back with a new key{}",
            clean(paired),
            clean(to),
            if held { " and is held for approval" } else { "" }
        )
    } else if held {
        format!(
            "Edge pairing {}: new node {} is held for approval",
            clean(paired),
            clean(to)
        )
    } else {
        format!(
            "Edge pairing {}: new node {} took over from {}",
            clean(paired),
            clean(to),
            if from.is_empty() {
                "none".to_string()
            } else {
                clean(from)
            }
        )
    };
    crate::notify::NotificationMessage::new()
        .topic(TAKEOVER_TOPIC)
        .user_group_id(ADMINISTRATORS)
        .notification_type("warning")
        .message_map(message)
        .title(title)
        .send()
        .map_err(|err| Error::new(err.message().to_string()))
}

#[cfg(test)]
#[path = "pool_tests.rs"]
mod tests;
