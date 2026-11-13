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

//! The `<edge>` conf (plans/archive/Edge-Node.md §5.2): the delegated services, the queue's kinds and limits and the
//! replicated sets, read once per factory with this node's role, so two servers in one process (a test's edge node and
//! local node) each keep their own.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::node::MNode;

use super::NodeRole;
use crate::context::ExecutionContextFactory;

/// One entity of a replicated set: its full name and the fields copied besides the primary key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplicaEntity {
    pub name: String,
    pub fields: Vec<String>,
}

/// A replicated set (`<replicate set="..">`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplicaSet {
    pub name: String,
    pub entities: Vec<ReplicaEntity>,
}

/// A delegated service (`<delegate service=".." allow-anonymous=".." timeout-seconds=".."/>`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delegate {
    /// A caller with no user may call it on the edge node when the edge node has no definition of the service to
    /// check (a webhook of a comms engine). With a definition, the definition's `authenticate` decides.
    pub allow_anonymous: bool,
    /// How long the edge node waits for the local node's answer, when shorter than the link's answer timeout (60 s):
    /// for a caller that cannot wait that long, as the SIP engine's routing of a call while it rings (E8).
    pub timeout: Option<Duration>,
}

/// The `<edge>` conf of one factory.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeSettings {
    /// This node's role (`node_role`), as the property was when the settings were first read. Read it through
    /// [`EdgeSettings::edge_node`], [`EdgeSettings::all_in_one`] and [`EdgeSettings::takes_links`].
    pub role: NodeRole,
    /// The services an edge node runs on its local node, and the only ones a local node runs for it, by canonical
    /// name (`create#moqui.X` for `create-moqui.X`, as the service facade names them).
    pub delegates: BTreeMap<String, Delegate>,
    /// The handler service of each queue kind.
    pub kinds: BTreeMap<String, String>,
    /// The service a sender calls for an item of the kind that failed for good (`queue-kind` `failed-service`), so a
    /// failure is not silent: after its last refusal, past the retention period or the size cap, or at unpairing.
    pub failed: BTreeMap<String, String>,
    pub sets: Vec<ReplicaSet>,
    pub retention_days: u32,
    pub max_bytes: u64,
    pub max_attempts: u32,
    /// The wait after the first refusal, doubled after each up to [`MAX_RETRY`].
    pub retry_first: Duration,
}

/// The longest wait between a receiver's refusals of an item.
pub const MAX_RETRY: Duration = Duration::from_secs(600);

impl Default for EdgeSettings {
    fn default() -> Self {
        Self {
            role: NodeRole::Normal,
            delegates: BTreeMap::new(),
            kinds: BTreeMap::new(),
            failed: BTreeMap::new(),
            sets: Vec::new(),
            retention_days: 14,
            max_bytes: 1024 * 1024 * 1024,
            max_attempts: 10,
            retry_first: Duration::from_secs(5),
        }
    }
}

impl EdgeSettings {
    /// A dedicated edge node (`node_role=edge`): no business data of its own, so a path it switches as a whole goes
    /// the edge way for every item.
    pub fn edge_node(&self) -> bool {
        self.role.edge_node()
    }

    /// An all-in-one node (`node_role=all-in-one`, E12): a full server that decides per item by its owner
    /// ([`super::owner`]).
    pub fn all_in_one(&self) -> bool {
        self.role.all_in_one()
    }

    /// The edge end of a link, a dedicated edge node or an all-in-one node: it accepts links, and its queue and
    /// delegated calls go to its paired node.
    pub fn takes_links(&self) -> bool {
        self.role.takes_links()
    }

    /// The settings of a merged conf's `<edge>`, for a node of the given role. A bad number keeps its default, logged.
    pub fn parse(conf: &MNode, role: NodeRole) -> Self {
        let mut settings = Self {
            role,
            ..Self::default()
        };
        let Some(edge) = conf.first_named("edge") else {
            return settings;
        };
        let number = |name: &str, default: u64| -> u64 {
            let raw = edge.attribute(name).unwrap_or_default();
            let raw = raw.trim();
            if raw.is_empty() {
                return default;
            }
            let parsed: Result<u64, _> = raw.parse();
            parsed.unwrap_or_else(|_| {
                tracing::error!("edge {name}=\"{raw}\" is not a whole number; {default} is used");
                default
            })
        };
        settings.retention_days = u32::try_from(number("queue-retention-days", 14)).unwrap_or(14);
        settings.max_bytes = number("queue-max-megabytes", 1024).saturating_mul(1024 * 1024);
        settings.max_attempts = u32::try_from(number("queue-max-attempts", 10)).unwrap_or(10).max(1);
        let retry = edge.attribute("queue-retry-seconds").unwrap_or_default();
        if !retry.trim().is_empty() {
            let seconds: Result<f64, _> = retry.trim().parse();
            match seconds {
                Ok(seconds) if seconds.is_finite() && seconds > 0.0 => {
                    settings.retry_first = Duration::from_secs_f64(seconds).min(MAX_RETRY);
                }
                _ => tracing::error!("edge queue-retry-seconds=\"{retry}\" is not a positive number; 5 is used"),
            }
        }
        for delegate in edge.named_iter("delegate") {
            let service = delegate.attribute("service").unwrap_or_default();
            let service = service.trim();
            if service.is_empty() {
                continue;
            }
            let allow_anonymous = delegate
                .attribute("allow-anonymous")
                .is_some_and(|flag| flag.trim() == "true");
            let timeout = delegate
                .attribute("timeout-seconds")
                .and_then(|raw| delegate_timeout(service, raw.trim()));
            settings.delegates.insert(
                crate::service::call::canonical(service),
                Delegate {
                    allow_anonymous,
                    timeout,
                },
            );
        }
        for kind in edge.named_iter("queue-kind") {
            let name = kind.attribute("name").unwrap_or_default().trim().to_string();
            let service = kind.attribute("service").unwrap_or_default().trim().to_string();
            if name.is_empty() || service.is_empty() {
                tracing::error!("an edge queue-kind needs a name and a service; one without is left out");
                continue;
            }
            let failed = kind.attribute("failed-service").unwrap_or_default().trim().to_string();
            if !failed.is_empty() {
                settings.failed.insert(name.clone(), failed);
            }
            settings.kinds.insert(name, service);
        }
        for set in edge.named_iter("replicate") {
            let name = set.attribute("set").unwrap_or_default().trim().to_string();
            if name.is_empty() {
                tracing::error!("an edge replicate needs a set name; one without is left out");
                continue;
            }
            let entities: Vec<ReplicaEntity> = set
                .named_iter("entity")
                .filter_map(|entity| {
                    let entity_name = entity.attribute("name").unwrap_or_default().trim().to_string();
                    let fields = entity
                        .attribute("fields")
                        .unwrap_or_default()
                        .split(',')
                        .map(str::trim)
                        .filter(|field| !field.is_empty())
                        .map(str::to_string)
                        .collect();
                    (!entity_name.is_empty()).then_some(ReplicaEntity {
                        name: entity_name,
                        fields,
                    })
                })
                .collect();
            settings.sets.push(ReplicaSet { name, entities });
        }
        settings
    }

    /// Whether an edge node runs `service` (a canonical name) on its local node.
    pub fn delegates(&self, service: &str) -> bool {
        self.delegates.contains_key(service)
    }

    /// The delegated service `service` (a canonical name), if the conf lists it.
    pub fn delegate(&self, service: &str) -> Option<&Delegate> {
        self.delegates.get(service)
    }

    /// The wait after the `attempt`th refusal (1 is the first).
    pub fn retry_after(&self, attempt: u32) -> Duration {
        let doubled = self
            .retry_first
            .saturating_mul(1_u32 << attempt.saturating_sub(1).min(16));
        doubled.min(MAX_RETRY)
    }
}

/// A delegate's `timeout-seconds`: none when empty; one that is not a positive number is logged and left out.
fn delegate_timeout(service: &str, raw: &str) -> Option<Duration> {
    if raw.is_empty() {
        return None;
    }
    let seconds: Result<f64, _> = raw.parse();
    match seconds {
        Ok(seconds) if seconds.is_finite() && seconds > 0.0 => Some(Duration::from_secs_f64(seconds)),
        _ => {
            tracing::error!(
                "edge delegate {service} timeout-seconds=\"{raw}\" is not a positive number; it is left out"
            );
            None
        }
    }
}

/// Set once some factory of this process takes links (an edge or all-in-one node) with delegated services, so a service call on any other node
/// does not look its factory's settings up.
static DELEGATING: AtomicBool = AtomicBool::new(false);

/// Whether a factory of this process may delegate service calls.
pub(crate) fn delegating() -> bool {
    DELEGATING.load(Ordering::Acquire)
}

/// The `<edge>` conf of `factory`, read the first time and kept: at boot on an edge node ([`init`]), else when the
/// host's listeners start, so the role is the one the node started with.
pub fn settings(factory: &ExecutionContextFactory) -> Arc<EdgeSettings> {
    settings_as(factory, super::node_role_kind())
}

/// At boot, on a node that takes links: read the `<edge>` conf now, so a service it delegates goes to the link from
/// the first call, before the link is installed or with no listener at all (then failing as offline), and never runs
/// here.
pub(crate) fn init(factory: &ExecutionContextFactory) {
    let role = super::node_role_kind();
    if role.takes_links() {
        settings_as(factory, role);
    }
}

/// [`settings`], read for a node of the given role the first time.
pub(super) fn settings_as(factory: &ExecutionContextFactory, role: NodeRole) -> Arc<EdgeSettings> {
    factory.extension(|factory| {
        let settings = match MNode::parse_shared("moqui-conf", &factory.conf_text()) {
            Ok(conf) => EdgeSettings::parse(&conf, role),
            Err(err) => {
                tracing::error!("the edge conf could not be read ({err}); nothing is delegated, queued or replicated");
                EdgeSettings {
                    role,
                    ..EdgeSettings::default()
                }
            }
        };
        if settings.takes_links() && !settings.delegates.is_empty() {
            DELEGATING.store(true, Ordering::Release);
        }
        settings
    })
}

#[cfg(test)]
#[path = "conf_tests.rs"]
mod tests;
