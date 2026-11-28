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

//! The SSH server's entry (plans/Runtime-Config.md §3.4, §8.4, RC3), live: its switch, host and port start, stop and
//! bind it again while the server runs, and its access mode, limits, git settings, SFTP to the drives and sub-service
//! flags apply in place ([`SshLive`]). It starts before the cluster, whose member row carries its address and host
//! key; a start or a new bind sets them again and rewrites the row. A failed bind is a Failed row and the boot goes on
//! (§6). The edge node's SSH tunnel and the cluster's SSH transport depend on it: turning it off ends their sessions
//! too, and a pass that does so logs them ([`Listener::dependents`]).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::conf_env::switch_on;
use moqui_core::startup::State;
use moqui_lib::error::Error;

use super::{Ctx, EndNow, Listener, Now, Running, Stage, StopHow, StopPhase, Wanted};
use crate::port_check::{Proto, Target};
use crate::ssh::intake::{Ending, SshListener};
use crate::ssh::live::SshLive;

/// The startup report's name, as [`moqui_core::listeners::SWITCHES`] has it.
const NAME: &str = "SSH";

/// Its switch, port and host (plans/Runtime-Config.md §3.2).
const ENABLED: &str = "ssh-server@enabled";
const PORT: &str = "ssh-server@port";
const HOST: &str = "ssh-server@host";

/// The settings the entry reads: what needs a new bind, then what applies in place, SFTP to the drives last.
const SELECTORS: [&str; 17] = [
    ENABLED,
    PORT,
    HOST,
    "ssh-server@access",
    "ssh-server@max-sessions",
    "ssh-server@max-sessions-per-user",
    "ssh-server@max-sessions-per-source",
    "ssh-server@max-dialog-sessions",
    "ssh-server@max-dialog-sessions-per-source",
    "ssh-server@sftp-max-file",
    "ssh-server@git-password-auth",
    "ssh-server@git-any-account",
    "ssh-server@tui-enabled",
    "ssh-server@sftp-enabled",
    "ssh-server@git-enabled",
    "ssh-server@shell-enabled",
    "dav-server@sftp-drives",
];

/// How long a bind on the edge runtime may take before it is given up.
const BIND_LIMIT: Duration = Duration::from_secs(30);

/// Where the SSH server binds: the host key is read again at each bind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SshBind {
    host: String,
    port: u16,
}

/// What the SSH server wants of the running conf `setting` reads (by selector, trimmed): off while its switch is off
/// or its port is not one, else its bind and its settings in place.
pub(super) fn ssh_wanted(setting: &dyn Fn(&str) -> Option<String>) -> Wanted<SshBind, SshLive> {
    let text = |selector: &str| {
        setting(selector)
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    };
    if !switch_on(&text(ENABLED)) {
        return Wanted::Off(format!("{ENABLED} is false"));
    }
    match crate::ssh::server::listen_target_of(&text(PORT), &text(HOST)) {
        Ok((host, port)) => Wanted::On {
            bind: SshBind { host, port },
            in_place: SshLive::from_conf(setting),
        },
        Err(err) => Wanted::Off(err.message().to_string()),
    }
}

/// Whether binding `new` would collide with `old` while it is bound: the same port, not one of the kernel's choosing,
/// on the same host or with either on every address; a host that is a name is taken to collide.
pub(super) fn ssh_overlaps(old: &SshBind, new: &SshBind) -> bool {
    if old.port != new.port || old.port == 0 {
        return false;
    }
    match (old.host.parse::<IpAddr>(), new.host.parse::<IpAddr>()) {
        (Ok(old), Ok(new)) => old == new || old.is_unspecified() || new.is_unspecified(),
        _ => true,
    }
}

/// The port target of `bind`, for a failed bind's explanation and the pre-check.
fn target_of(bind: &SshBind) -> Target {
    Target::optional(NAME, PORT, &bind.host, bind.port, Proto::Tcp)
}

/// The SSH server's row text: its address and what applies in place ([`SshLive::summary`]).
fn state_of(address: SocketAddr, live: &SshLive) -> State {
    State::On(format!("tcp {address}, {}", live.summary()))
}

/// How a stop the registry begins ends the bind's sessions: the process stop at once, as before the registry.
fn ending(how: StopHow) -> Ending {
    match how {
        StopHow::Process | StopHow::Now => Ending::Now,
        StopHow::Grace(grace) => Ending::After(grace),
    }
}

/// A bind of the SSH server, as the registry holds it.
pub(crate) struct SshRun {
    listener: SshListener,
    state: State,
}

impl Running<SshLive> for SshRun {
    fn state(&self) -> State {
        self.state.clone()
    }

    fn address(&self) -> Option<SocketAddr> {
        Some(self.listener.address())
    }

    /// New connections, logins and channels read the new settings; the sessions at the sign-in dialog end when the
    /// access mode leaves `open`.
    fn apply(&mut self, live: &SshLive) {
        for problem in &live.problems {
            tracing::error!("The SSH server's settings: {problem}");
        }
        self.listener.apply(live);
        self.state = state_of(self.listener.address(), live);
    }

    fn begin_stop(&mut self, how: StopHow) {
        self.listener.begin_stop(ending(how));
    }

    fn end_now(&self) -> Option<EndNow> {
        Some(self.listener.end_now_handle())
    }

    fn released(&mut self, wait: Duration) -> bool {
        self.listener.released(wait)
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.listener.finished(wait)
    }

    fn connections(&self) -> Option<usize> {
        Some(self.listener.sessions())
    }
}

pub(crate) struct Ssh;

impl Listener for Ssh {
    type Bind = SshBind;
    type InPlace = SshLive;
    type Run = SshRun;

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

    fn live(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &SELECTORS
    }

    /// From the pass's reading of the running conf, with `dav-server@sftp-drives` beside the `ssh-server` settings.
    fn wanted(&self, ctx: &Ctx) -> Wanted<SshBind, SshLive> {
        ssh_wanted(&|selector: &str| ctx.conf.value(selector))
    }

    fn start(&self, ctx: &Ctx, bind: &SshBind, live: &SshLive) -> Result<Option<SshRun>, Error> {
        let target = target_of(bind);
        // host and port apart, so an IPv6 address needs no brackets
        let address = (bind.host.clone(), bind.port);
        let bound = super::worker::on_runtime(&crate::edge::handle(), BIND_LIMIT, async move {
            tokio::net::TcpListener::bind(address).await
        })
        .map_err(Error::new)?
        .map_err(|err| Error::new(crate::port_check::explain(&target, &err)))?;
        for problem in &live.problems {
            tracing::error!("The SSH server's settings: {problem}");
        }
        let listener = crate::ssh::intake::serve(
            ctx.factory,
            &ctx.boot.runtime_dir,
            Arc::clone(&ctx.boot.cluster_posts),
            bound,
            live,
        )?;
        // this member's row carries the address and host key, after the boot's pass, which comes before the cluster
        // starts and writes the row itself; written off the pass
        crate::ssh::intake::rewrite_member_row(ctx.factory);
        Ok(Some(SshRun {
            state: state_of(listener.address(), live),
            listener,
        }))
    }

    fn overlaps(&self, old: &SshBind, new: &SshBind) -> bool {
        ssh_overlaps(old, new)
    }

    /// The cluster's SSH transport, whose peers post over it, and the edge node's SSH tunnel to this node: turned off,
    /// SSH ends their sessions too (plans/Runtime-Config.md §3.4).
    fn dependents(&self, ctx: &Ctx) -> Vec<String> {
        let mut dependents = Vec::new();
        let transport = ctx.conf.value("cluster-member@transport").unwrap_or_default();
        if transport.trim().eq_ignore_ascii_case("ssh") {
            dependents.push("the cluster's SSH transport (cluster-member@transport=\"ssh\")".to_string());
        }
        if moqui_core::edge::setup::ssh_address(ctx.factory).is_some() {
            dependents.push("the edge node's SSH tunnel to this node".to_string());
        }
        dependents
    }

    fn target(&self, bind: &SshBind) -> Option<Target> {
        Some(target_of(bind))
    }

    /// The edge node's tunnel streams are taken while a bind runs; off, they end and the address and the cluster row's
    /// properties go.
    fn published(&self, ctx: &Ctx, now: Now<'_, SshLive>) {
        match now {
            Now::Running(_) => crate::ssh::intake::now_running(ctx.factory),
            Now::Off(_) | Now::Failed | Now::NotYet => crate::ssh::intake::now_off(ctx.factory),
        }
    }
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
