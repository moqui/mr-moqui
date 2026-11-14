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

//! The members of the edge cluster a local node links to (plans/Edge-Node2.md §4.6), as far as they need no network.
//!
//! Every edge member's Hello answer, and each answer to a Ping, lists the cluster's live members with their own link
//! addresses (`edge_link_url`). The local node dials each one ([`plan`]), checks that a link dialed for a member reaches
//! that member, and keeps the list and the ids it learned ([`Known`]) in `runtime/db/edge_members`: this machine's, as
//! `runtime/db/instance_id` and `runtime/db/edge_member_key` are, since each member of a local cluster learns the list
//! from its own links and `runtime/.env` is what people copy between the members. A restart then finds a member when
//! the one this node paired with is gone. The list is read again with each Hello and Ping; a member gone from it is
//! not dialed again.

use std::path::{Path, PathBuf};

use super::proto;

/// The most members an edge cluster's list may name; a longer list is not taken.
pub(crate) const MAX_EDGE_MEMBERS: usize = 64;
/// The longest link address taken from a list.
const MAX_URL: usize = moqui_core::edge::cluster_list::MAX_LINK_URL;
/// The attempts every member's link fails in a row, with none linked, before the URL this node paired with is dialed
/// too: a list whose addresses all fail (members renamed while this node was away) does not strand the node.
pub(crate) const PAIRED_URL_AFTER: u32 = 2;
/// The file under the runtime directory that keeps the list.
const FILE: &str = "db/edge_members";

/// One member of an edge cluster as its list names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EdgeMember {
    pub instance_id: String,
    /// Its `edge_link_url`; empty when it has none, so it is reached only at the URL this node paired with.
    pub link_url: String,
}

pub(crate) fn from_proto(list: &[proto::EdgeMember]) -> Vec<EdgeMember> {
    list.iter()
        .map(|member| EdgeMember {
            instance_id: member.instance_id.clone(),
            link_url: member.link_url.trim().to_string(),
        })
        .collect()
}

pub(crate) fn to_proto(list: &[EdgeMember]) -> Vec<proto::EdgeMember> {
    list.iter()
        .map(|member| proto::EdgeMember {
            instance_id: member.instance_id.clone(),
            link_url: member.link_url.clone(),
        })
        .collect()
}

/// Check a list an edge member sent, as input (plans/Edge-Node2.md §4.11): at most [`MAX_EDGE_MEMBERS`], each id a
/// node id and named once, each address empty or an `https` URL with a host. A list that fails is not taken.
pub(crate) fn check_list(list: &[EdgeMember]) -> Result<(), String> {
    if list.len() > MAX_EDGE_MEMBERS {
        return Err(format!(
            "the edge cluster's list names {} members, more than {MAX_EDGE_MEMBERS}",
            list.len()
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(list.len());
    for member in list {
        if !super::valid_node_id(&member.instance_id) {
            return Err(format!(
                "the edge cluster's list names an instance id that is empty, too long or has spaces, control \
                 characters or @: {}",
                member.instance_id.chars().take(40).collect::<String>().escape_debug()
            ));
        }
        if !seen.insert(member.instance_id.as_str()) {
            return Err(format!("the edge cluster's list names {} twice", member.instance_id));
        }
        if !member.link_url.is_empty() {
            if member.link_url.len() > MAX_URL || member.link_url.chars().any(char::is_control) {
                return Err(format!(
                    "the link address of edge member {} is too long or not text",
                    member.instance_id
                ));
            }
            if !member.link_url.starts_with("https://") {
                return Err(format!(
                    "the link address of edge member {} is not https",
                    member.instance_id
                ));
            }
            super::pair::parse_url(&member.link_url)
                .map_err(|err| format!("the link address of edge member {}: {err}", member.instance_id))?;
        }
    }
    Ok(())
}

/// What a local node keeps of its edge cluster: the latest list and the member ids it learned (`edge_node_id`, each
/// on a link whose key is pinned), for the edge node it paired with: kept with that URL and pin, so a list from before
/// pairing with another edge node is not used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Known {
    url: String,
    pin: String,
    pub members: Vec<EdgeMember>,
    pub ids: Vec<String>,
}

impl Known {
    /// Nothing known yet, for the edge node at `url` with link key `pin`.
    pub(crate) fn new(url: &str, pin: &str) -> Self {
        Self {
            url: url.trim().to_string(),
            pin: pin.trim().to_string(),
            ..Self::default()
        }
    }

    /// What `path` keeps for the edge node at `url` with link key `pin`: nothing when it has none, keeps another
    /// pairing's, or cannot be read (logged; the list comes back with the first link).
    pub(crate) fn load(path: Option<&Path>, url: &str, pin: &str) -> Self {
        let fresh = Self::new(url, pin);
        let Some(path) = path else {
            return fresh;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return fresh,
            Err(err) => {
                tracing::error!(
                    "Edge link: {} was not read ({err}); the edge cluster's list comes with the first link",
                    path.display()
                );
                return fresh;
            }
        };
        match Self::from_json(&text) {
            Ok(kept) if kept.url == fresh.url && kept.pin == fresh.pin => {
                let mut kept = kept;
                // input from a file: checked as a list from the link is, and dropped whole when it fails
                if let Err(why) = check_list(&kept.members) {
                    tracing::error!("Edge link: the list in {} is not used: {why}", path.display());
                    kept.members.clear();
                }
                kept.ids.retain(|id| super::valid_node_id(id));
                kept.ids.truncate(MAX_EDGE_MEMBERS * 4);
                kept
            }
            Ok(_) => fresh,
            Err(err) => {
                tracing::error!(
                    "Edge link: {} is not a list of edge members ({err}); it is written again",
                    path.display()
                );
                fresh
            }
        }
    }

    /// Write it to `path` (a new file put in place), logged when it fails: the link goes on, and the list comes back
    /// with the first link after a restart.
    pub(crate) fn save(&self, path: Option<&Path>) {
        let Some(path) = path else {
            return;
        };
        let written = serde_json::to_vec_pretty(&self.to_json())
            .map_err(std::io::Error::other)
            .and_then(|json| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let next = path.with_extension("new");
                std::fs::write(&next, json)?;
                std::fs::rename(&next, path)
            });
        if let Err(err) = written {
            tracing::error!(
                "Edge link: the edge cluster's list was not written to {}: {err}",
                path.display()
            );
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "url": self.url,
            "pin": self.pin,
            "members": self.members.iter().map(|member| serde_json::json!({
                "instanceId": member.instance_id,
                "linkUrl": member.link_url,
            })).collect::<Vec<_>>(),
            "ids": self.ids,
        })
    }

    fn from_json(text: &str) -> Result<Self, String> {
        let json: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let text_of = |value: &serde_json::Value, name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let list = |name: &str| {
            json.get(name)
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        if !json.is_object() {
            return Err("not an object".to_string());
        }
        Ok(Self {
            url: text_of(&json, "url"),
            pin: text_of(&json, "pin"),
            members: list("members")
                .iter()
                .map(|member| EdgeMember {
                    instance_id: text_of(member, "instanceId"),
                    link_url: text_of(member, "linkUrl"),
                })
                .collect(),
            ids: list("ids")
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect(),
        })
    }

    /// Take `list` as the latest: whether it differs from the one kept.
    pub(crate) fn set_members(&mut self, mut list: Vec<EdgeMember>) -> bool {
        // in instance id order: the members answer in their own order, and the same members are no change
        list.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        if self.members == list {
            return false;
        }
        self.members = list;
        true
    }

    /// Learn member id `id`: whether it is new. The oldest goes first past a bound, so the list cannot grow without
    /// end.
    pub(crate) fn learn(&mut self, id: &str) -> bool {
        if id.is_empty() || self.ids.iter().any(|known| known == id) {
            return false;
        }
        self.ids.push(id.to_string());
        let over = self.ids.len().saturating_sub(MAX_EDGE_MEMBERS * 4);
        self.ids.drain(..over);
        true
    }

    /// The list's link address of member `id`: its own, else `paired_url`.
    pub(crate) fn url_of(&self, id: &str, paired_url: &str) -> String {
        self.members
            .iter()
            .find(|member| member.instance_id == id && !member.link_url.is_empty())
            .map_or_else(|| paired_url.to_string(), |member| member.link_url.clone())
    }
}

/// Where a server keeps the list: `runtime/db/edge_members`.
pub(crate) fn file(runtime: &Path) -> PathBuf {
    runtime.join(FILE)
}

/// Remove the list kept under `runtime`, at unpairing; logged when it fails.
pub(crate) fn forget(runtime: &Path) {
    let path = file(runtime);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::error!("Unpaired, but {} was not removed: {err}", path.display()),
    }
}

/// One of a dialer's links as [`plan`] sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkView {
    /// The member it is for; `None` for the link to the URL this node paired with before its member is known.
    pub member: Option<String>,
    /// Where it dials now.
    pub url: String,
    pub linked: bool,
    /// Its attempts that failed in a row since it was last linked.
    pub failures: u32,
}

/// What a dialer does with its links after a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Links to start: the member and its address.
    pub start: Vec<(String, String)>,
    /// Links whose address is now another: the member and the address.
    pub dial_at: Vec<(String, String)>,
    /// Links of members gone from the list: not dialed again (one that is linked goes on until it ends).
    pub retire: Vec<String>,
    /// Whether the link to the URL this node paired with should run.
    pub paired_url: bool,
}

/// The links a local node keeps for `list` (plans/Edge-Node2.md §4.6): one per member, at its own address, else at
/// `paired_url`. A member with no address and no link is reached only through the URL this node paired with, which
/// runs while no member's link does: with no list yet (a first link, an edge node before E18), or when none is linked,
/// no member without an address of its own has a link (that link dials the URL), and every link failed
/// [`PAIRED_URL_AFTER`] times in a row (a list whose addresses are stale, or reach other members). The link it makes
/// is the member's once its Hello names one. An empty list is not told: it changes nothing.
pub(crate) fn plan(list: &[EdgeMember], paired_url: &str, links: &[LinkView]) -> Plan {
    let mut plan = Plan::default();
    let url_of = |id: &str| {
        list.iter()
            .find(|member| member.instance_id == id && !member.link_url.is_empty())
            .map_or_else(|| paired_url.to_string(), |member| member.link_url.clone())
    };
    let keyed: Vec<&LinkView> = links.iter().filter(|link| link.member.is_some()).collect();
    for link in &keyed {
        let id = link.member.as_deref().unwrap_or_default();
        if !list.is_empty() && !list.iter().any(|member| member.instance_id == id) {
            plan.retire.push(id.to_string());
            continue;
        }
        let url = if list.is_empty() { link.url.clone() } else { url_of(id) };
        if url != link.url {
            plan.dial_at.push((id.to_string(), url));
        }
    }
    for member in list {
        if member.link_url.is_empty()
            || keyed
                .iter()
                .any(|link| link.member.as_deref() == Some(&member.instance_id))
        {
            continue;
        }
        plan.start.push((member.instance_id.clone(), member.link_url.clone()));
    }
    let kept: Vec<&LinkView> = keyed
        .iter()
        .copied()
        .filter(|link| !plan.retire.iter().any(|id| link.member.as_deref() == Some(id)))
        .collect();
    let any_linked = links.iter().any(|link| link.linked);
    // a member with no address of its own is reached at the paired URL by its own link: no second link to it
    let dials_paired = kept.iter().any(|link| {
        let id = link.member.as_deref().unwrap_or_default();
        !list
            .iter()
            .any(|member| member.instance_id == id && !member.link_url.is_empty())
    });
    let all_failed = plan.start.is_empty() && kept.iter().all(|link| link.failures >= PAIRED_URL_AFTER);
    plan.paired_url = !any_linked && !dials_paired && all_failed;
    plan
}

/// The list an edge member's Hello and Ping answers carry (plans/Edge-Node2.md §4.6): its cluster's live members by
/// their rows, in instance id order, at most [`MAX_EDGE_MEMBERS`], itself with its own address (`own_url`). Every member
/// answers from the same rows by the database's clock, so their lists agree: the answering member is in it only while
/// its own row is live too (or it has none, a node with no cluster row yet), never by its own say. An address that is
/// not an `https` URL with a host goes as none, so the rest of the list is still taken.
pub(crate) fn of_cluster(own_id: &str, own_url: &str, rows: &[moqui_core::cluster::MemberReport]) -> Vec<EdgeMember> {
    let usable = |url: &str| {
        moqui_core::edge::cluster_list::usable_link_url(url)
            .unwrap_or_default()
            .to_string()
    };
    let own_live = rows
        .iter()
        .find(|row| row.instance_id == own_id)
        .is_none_or(|row| row.live);
    let mut list: Vec<EdgeMember> = Vec::new();
    if own_live && super::valid_node_id(own_id) {
        list.push(EdgeMember {
            instance_id: own_id.to_string(),
            link_url: usable(own_url),
        });
    }
    for row in rows {
        if !row.live || row.instance_id == own_id || !super::valid_node_id(&row.instance_id) {
            continue;
        }
        list.push(EdgeMember {
            instance_id: row.instance_id.clone(),
            link_url: usable(&row.edge_link_url),
        });
    }
    list.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    list.truncate(MAX_EDGE_MEMBERS);
    list
}

#[cfg(test)]
#[path = "members_tests.rs"]
mod tests;
