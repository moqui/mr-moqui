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

//! The cluster port's entry (plans/Runtime-Config.md §8.1, RC0): today's `cluster_edge::listen` wrapped, after
//! `cluster::start` writes this member's row. A node reached in process only has none. Its row, which
//! `http::record_web_listeners` wrote before the registry, says the same here; a failed bind still stops the boot until
//! RC6 makes the port optional.

use std::sync::Arc;

use moqui_core::startup::State;
use moqui_lib::error::Error;

use super::{Ctx, Listener, Stage, StopPhase, Wanted, Wrapped};
use crate::cluster_edge::ClusterEdge;
use crate::port_check::Target;
use crate::ListenBind;

/// The startup report's name, which `crate::Server::cluster_address` asks the registry by.
pub(crate) const NAME: &str = "Cluster";

pub(crate) struct Cluster;

impl Listener for Cluster {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<ClusterEdge>;

    fn name(&self) -> &'static str {
        NAME
    }

    fn stage(&self) -> Stage {
        Stage::ClusterPort
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::AfterFactory
    }

    /// Last, after SMTP and SSH, as today.
    fn stop_rank(&self) -> u8 {
        2
    }

    fn fatal_at_boot(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &["cluster-member@https-port"]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<(), ()> {
        match ctx.boot.bind {
            ListenBind::None => Wanted::Off("this node is reached in process only".to_string()),
            ListenBind::Address(_) | ListenBind::Property => Wanted::On { bind: (), in_place: () },
        }
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Wrapped<ClusterEdge>>, Error> {
        let edge = crate::cluster_edge::listen(Arc::clone(ctx.factory), Arc::clone(&ctx.boot.cluster_posts))?;
        let address = edge.address();
        Ok(Some(Wrapped::new(
            edge,
            State::On(format!("tcp {address} mTLS")),
            Some(address),
        )))
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}
