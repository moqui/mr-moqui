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

//! The SSH server's entry (plans/Runtime-Config.md §8.1, RC0): today's `ssh::server::start` wrapped, which reads the
//! conf itself, and the row the boot recorded for it before the registry. It starts before the cluster, whose member
//! row carries its address and host key; a failed start still stops the boot until RC3 makes it live.

use std::sync::Arc;

use moqui_core::startup::State;
use moqui_lib::error::Error;

use super::{Ctx, Listener, Stage, StopPhase, Wanted, Wrapped};
use crate::port_check::Target;
use crate::ssh::server::SshServer;

/// The startup report's name.
const NAME: &str = "SSH";

pub(crate) struct Ssh;

impl Listener for Ssh {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<SshServer>;

    fn name(&self) -> &'static str {
        NAME
    }

    fn stage(&self) -> Stage {
        Stage::BeforeCluster
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::AfterFactory
    }

    /// After SMTP and before the cluster port, as today.
    fn stop_rank(&self) -> u8 {
        1
    }

    fn fatal_at_boot(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &["ssh-server@enabled", "ssh-server@port", "ssh-server@host"]
    }

    /// The wrapped start reads the conf and finds it off itself.
    fn wanted(&self, _ctx: &Ctx) -> Wanted<(), ()> {
        Wanted::On { bind: (), in_place: () }
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Wrapped<SshServer>>, Error> {
        let started =
            crate::ssh::server::start(ctx.factory, &ctx.boot.runtime_dir, Arc::clone(&ctx.boot.cluster_posts));
        match started {
            Ok(Some(server)) => {
                let address = server.address();
                Ok(Some(Wrapped::new(
                    server,
                    State::On(format!("tcp {address}")),
                    Some(address),
                )))
            }
            Ok(None) => {
                moqui_core::startup::listener(ctx.factory, NAME, State::off("ssh-server@enabled is false"));
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}
