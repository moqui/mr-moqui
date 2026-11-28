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

//! The cluster port's entry (plans/Runtime-Config.md §3.5, RC6), live: `cluster-member@listen` opens it always (`on`),
//! never (`off`) or, by default, only while this member is in a cluster (`auto`): while the member table has another
//! live member with this member's session key and certificate fingerprints, from [`cluster::start`]'s read before the
//! boot's pass, so a member that starts into a cluster opens it before it reports ready, until the last other member's
//! row is deleted as dead. Under `auto` a member that peers reach elsewhere (`pg-notify:`, its SSH server, a base URL
//! that is not `https://`) opens none. Core's cluster module tells [`follow_membership`]'s hook when the cluster changes
//! (the heartbeat, a lone check every `cluster-member@lone-check-seconds` while alone, a send's read), and the hook asks
//! for a pass. Its port and address bind again while the server runs, and each start rewrites this member's row, whose
//! default base URL names the port. A port that does not bind, at the start or when it opens later, is a Failed row and
//! the server goes on (§6); a node reached in process only has none.
//!
//! [`cluster::start`]: moqui_core::cluster::start

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::listeners::{Cause, ReconcileRequest};
use moqui_core::startup::State;
use moqui_lib::error::Error;

use super::{Ctx, Listener, Registry, Running, Stage, StopHow, StopPhase, Wanted};
use crate::cluster_edge::ClusterEdge;
use crate::port_check::{Proto, Target};
use crate::ListenBind;

/// The startup report's name, which `crate::Server::cluster_address` asks the registry's addresses by.
const NAME: &str = "Cluster";

/// Its switch, port and address (plans/Runtime-Config.md §3.2, §3.5).
const LISTEN: &str = "cluster-member@listen";
const PORT: &str = "cluster-member@https-port";
const BIND_ADDRESS: &str = "cluster-member@bind-address";
/// How peers reach this member, read for `auto` and for the row: both wait for a restart, so they are not selectors.
const BASE_URL: &str = "cluster-member@base-url";
const TRANSPORT: &str = "cluster-member@transport";

/// The settings the entry reads.
const SELECTORS: [&str; 3] = [LISTEN, PORT, BIND_ADDRESS];

/// Its row under `auto` while this member is alone, as the startup report says it (plans/Runtime-Config.md §3.5).
const NO_OTHER_MEMBER: &str = "no other member";

/// The cause of the pass a change of this member's cluster asks for.
const MEMBERSHIP: &str = "cluster member";

/// What the cluster port wants for the boot's `bind`, from the running conf `setting` reads (by selector) and whether
/// this member is in a cluster now (`in_cluster`, [`moqui_core::cluster::cluster_detected`]): off for a node reached
/// in process only, while `cluster-member@listen` is `off` (or none of the XSD's three values, named on the row), and
/// under `auto` for a member peers reach elsewhere or while there is no other member; else its address, unless the
/// port or the address is not one (named on the row). A conf that leaves `listen` out means `auto`, its XSD default.
pub(super) fn cluster_wanted(
    bind: &ListenBind,
    setting: &dyn Fn(&str) -> Option<String>,
    in_cluster: bool,
) -> Wanted<SocketAddr, ()> {
    if matches!(bind, ListenBind::None) {
        return Wanted::Off("this node is reached in process only".to_string());
    }
    let text = |selector: &str| {
        setting(selector)
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    };
    // the XSD's values exactly (an xs:token, so the spaces around one are not part of it)
    match setting(LISTEN).as_deref().map(str::trim) {
        Some("on") => {}
        None | Some("auto") => {
            if let Some(elsewhere) = reached_elsewhere(&text) {
                return Wanted::Off(elsewhere);
            }
            if !in_cluster {
                return Wanted::Off(NO_OTHER_MEMBER.to_string());
            }
        }
        Some("off") => return Wanted::Off(format!("{LISTEN} is off")),
        Some(other) => return Wanted::Off(format!("{LISTEN} \"{other}\" is not auto, on or off")),
    }
    let address = crate::cluster_edge::bind_address(&text(BIND_ADDRESS))
        .and_then(|ip| crate::cluster_edge::listen_port(&text(PORT)).map(|port| SocketAddr::new(ip, port)));
    match address {
        Ok(address) => Wanted::On {
            bind: address,
            in_place: (),
        },
        Err(err) => Wanted::Off(err.message().to_string()),
    }
}

/// Why peers do not reach this member on its cluster port, when they reach it elsewhere: a `cluster-member@base-url`
/// that is not `https://`, or with none, `cluster-member@transport` `notify` (Postgres NOTIFY) or `ssh` (its SSH
/// server), as core's cluster module makes the base URL. None for the cluster port.
fn reached_elsewhere(text: &dyn Fn(&str) -> String) -> Option<String> {
    let base = text(BASE_URL);
    if !base.is_empty() {
        return (!base.starts_with("https://"))
            .then(|| format!("peers reach this member at {BASE_URL} {base}, not on its cluster port"));
    }
    let transport = text(TRANSPORT);
    matches!(transport.as_str(), "notify" | "ssh")
        .then(|| format!("{TRANSPORT} is {transport}: peers do not reach this member on its cluster port"))
}

/// What the row says of an explicit `https://` `cluster-member@base-url` whose port is not the one the port is bound to
/// now (`bound`): peers post there, and the base URL waits for a restart. None when it names that port, names none, or
/// is not set (the default names the bound port once the row is rewritten).
pub(super) fn base_url_note(text: &dyn Fn(&str) -> String, bound: u16) -> Option<String> {
    let base = text(BASE_URL);
    if !base.starts_with("https://") {
        return None;
    }
    let (_, port) = crate::cluster_edge::url_host(&base)?;
    (port != 0 && port != bound).then(|| {
        format!(
            "{BASE_URL} {base} names port {port}, not {bound} where this member listens: peers post there until it \
             is changed, which takes a restart"
        )
    })
}

/// Rewrite this member's row on a thread of its own after the port starts (`moqui_core::cluster::refresh_member`,
/// which writes only what changed, one rewrite at a time), so its default base URL names the port bound now, and a
/// database that is slow or down holds no listener change behind the registry's lock.
fn rewrite_member_row(factory: &Arc<moqui_core::context::ExecutionContextFactory>) {
    let factory = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-cluster-member".to_string())
        .spawn(moqui_core::context::framework_thread(move || {
            let Some(factory) = factory.upgrade() else {
                return;
            };
            let context = factory.new_context();
            let _entered = context.enter();
            moqui_core::cluster::refresh_member(&factory);
        }));
    if let Err(err) = spawned {
        tracing::error!("This member's cluster row keeps its old base URL: no thread to rewrite it: {err}");
    }
}

/// The port target of `bind`, for a failed bind's explanation and the pre-check.
fn target_of(bind: SocketAddr) -> Target {
    Target::optional(NAME, PORT, bind.ip(), bind.port(), Proto::Tcp)
}

/// Ask `registry` for a pass each time this member joins a cluster or is left alone (plans/Runtime-Config.md §3.5):
/// core's cluster module tells the hook installed here, on the thread that read the member table, which only queues
/// the request. The hook holds the registry weakly, so it neither keeps the listeners nor makes a cycle with the
/// factory the registry holds; installed before `cluster::start`, so the start's read is told too.
pub(crate) fn follow_membership(registry: &Registry) {
    let weak = Arc::downgrade(&registry.0);
    moqui_core::cluster::on_membership(
        &registry.0.factory,
        Arc::new(move |_in_cluster| {
            if let Some(inner) = weak.upgrade() {
                // nobody waits for the answer: the pass records the row
                drop(Registry(inner).request(ReconcileRequest::new(Cause::Signal(MEMBERSHIP)), Duration::ZERO));
            }
        }),
    );
}

/// A bind of the cluster port, as the registry holds it.
pub(crate) struct ClusterRun {
    edge: ClusterEdge,
    state: State,
}

impl Running<()> for ClusterRun {
    fn state(&self) -> State {
        self.state.clone()
    }

    fn address(&self) -> Option<SocketAddr> {
        Some(self.edge.address())
    }

    fn begin_stop(&mut self, how: StopHow) {
        match how {
            StopHow::Now => self.edge.end_now(),
            StopHow::Process | StopHow::Grace(_) => self.edge.begin_stop(),
        }
    }

    fn released(&mut self, wait: Duration) -> bool {
        self.edge.released(wait)
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.edge.finished(wait)
    }
}

pub(crate) struct Cluster;

impl Listener for Cluster {
    type Bind = SocketAddr;
    type InPlace = ();
    type Run = ClusterRun;

    fn name(&self) -> &'static str {
        NAME
    }

    fn stage(&self) -> Stage {
        Stage::ClusterPort
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::AfterFactory
    }

    /// Last, after SMTP and SSH, as before the registry.
    fn stop_rank(&self) -> u8 {
        2
    }

    fn live(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &SELECTORS
    }

    /// From the pass's reading of the running conf, and whether this member is in a cluster as the member table was
    /// last read.
    fn wanted(&self, ctx: &Ctx) -> Wanted<SocketAddr, ()> {
        cluster_wanted(
            &ctx.boot.bind,
            &|selector: &str| ctx.conf.value(selector),
            moqui_core::cluster::cluster_detected(ctx.factory),
        )
    }

    fn start(&self, ctx: &Ctx, bind: &SocketAddr, _in_place: &()) -> Result<Option<ClusterRun>, Error> {
        let listener = std::net::TcpListener::bind(*bind)
            .map_err(|err| Error::new(crate::port_check::explain(&target_of(*bind), &err)))?;
        let edge = crate::cluster_edge::listen(Arc::clone(ctx.factory), Arc::clone(&ctx.boot.cluster_posts), listener)?;
        let address = edge.address();
        let mut detail = format!("tcp {address} mTLS");
        let text = |selector: &str| {
            ctx.conf
                .value(selector)
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        if let Some(note) = base_url_note(&text, address.port()) {
            tracing::warn!("The cluster port: {note}");
            detail = format!("{detail}; {note}");
        }
        rewrite_member_row(ctx.factory);
        Ok(Some(ClusterRun {
            state: State::On(detail),
            edge,
        }))
    }

    fn overlaps(&self, old: &SocketAddr, new: &SocketAddr) -> bool {
        super::web::web_overlaps(*old, *new)
    }

    fn target(&self, bind: &SocketAddr) -> Option<Target> {
        Some(target_of(*bind))
    }
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
