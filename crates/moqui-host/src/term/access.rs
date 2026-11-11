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

//! Who may open a command or host session, and from where (plans/Terminal-Fancy.md §8). They are for a dev instance
//! (`instance_purpose` dev) and a user with the kind's permission: `SSH_SHELL` for the command line, `TERMINAL_HOST`
//! for a program on the server's operating system (D6). From the server's own machine (a loopback peer, no
//! forwarding header, the server's own loopback origin; or the Desktop app's node in process) nothing more is asked.
//! From anywhere else `terminal` `remote` must be on, the request secure, and the user's sign-in recent (D7). Both are
//! asked when a session is opened and when one is attached. A command or host session takes only the server's own
//! origin, whatever `allow-origins` says. A refusal tells nothing of why but a sign-in that is not recent.

use std::net::IpAddr;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::web::origin::{request_site, SiteOrigin};
use moqui_core::web::RequestVia;

use super::session::Kind;

/// The permission of the command line, the SSH prompt's own.
pub(crate) const COMMAND_PERMISSION: &str = "SSH_SHELL";
/// The permission of host sessions.
pub(crate) const HOST_PERMISSION: &str = "TERMINAL_HOST";
const REMOTE_REAUTH_MINUTES: usize = 60;
/// The headers a proxy in front sets: a request with one did not come straight from this machine.
const FORWARDING: [&str; 6] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
];

/// Where a request came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// The server's own machine, or the Desktop app's node in process.
    Local,
    /// Anywhere else: a LAN, a VPN, the edge node, a tunnel tool or a proxy on this machine.
    Remote {
        /// Over HTTPS: this server's TLS, a believed proxy's https, or the edge link's.
        secure: bool,
        /// Through the edge node.
        via_edge: bool,
    },
}

/// What a request says about where it came from.
#[derive(Clone, Debug, Default)]
pub(crate) struct Facts {
    /// The socket's peer.
    pub peer: Option<IpAddr>,
    pub via: RequestVia,
    /// A forwarding header is there.
    pub forwarded: bool,
    pub https: bool,
    /// The request's own origin: its scheme, its Host and its port.
    pub site: Option<SiteOrigin>,
    /// Its `Origin` header.
    pub origin: Option<String>,
    /// The client's address, for the audit.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub client_ip: String,
    /// An SSH connection: no origin to check, and encrypted.
    pub ssh: bool,
    /// Signed in by a device's login key (the Desktop and Mobile apps' host, §12), which sends no `Origin`: a page of
    /// another site cannot send the key.
    pub device_key: bool,
}

/// Whether `host` names this machine: `localhost`, a name under `.localhost`, or a loopback address.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.to_canonical().is_loopback())
}

impl Facts {
    /// The facts of a request, from its peer, how it came, its headers and whether it is https. `header` gives a
    /// header by its lowercase name; `client_header` is the conf's client address header, a forwarding header too.
    pub fn of<'a>(
        peer: Option<IpAddr>,
        via: RequestVia,
        https: bool,
        client_header: &str,
        client_ip: String,
        header: impl Fn(&str) -> Option<&'a str>,
    ) -> Self {
        let forwarded = FORWARDING.iter().any(|name| header(name).is_some())
            || (!client_header.is_empty() && header(&client_header.to_ascii_lowercase()).is_some());
        Self {
            peer,
            via,
            forwarded,
            https,
            // a local request has no proxy to believe; a remote one through a proxy is checked by its origin below
            site: request_site(https, false, header("host"), None, None),
            origin: header("origin").map(str::to_string),
            client_ip,
            ssh: false,
            device_key: header("login_key").is_some_and(|key| !key.is_empty()),
        }
    }

    /// The facts of an SSH connection from `peer`.
    #[cfg_attr(not(all(feature = "ssh", feature = "devtools", unix)), allow(dead_code))]
    pub fn over_ssh(peer: IpAddr) -> Self {
        Self {
            peer: Some(peer),
            https: true,
            client_ip: peer.to_string(),
            ssh: true,
            ..Self::default()
        }
    }

    /// The facts of the screen request on this thread, for a transition's service.
    pub fn of_current_request() -> Self {
        let Some(request) = moqui_core::web::current_request() else {
            return Self::default();
        };
        let addressing = moqui_core::web::client_addressing();
        Self::of(
            request.peer_ip.trim().parse().ok(),
            request.via,
            request.https,
            addressing.header(),
            moqui_core::web::client_ip(&request),
            |name| request.header(name),
        )
    }

    /// The facts of a same-origin GET, which carries no `Origin`: taken as the request's own. For what is offered only;
    /// opening and attaching carry their origin.
    pub fn listing(mut self) -> Self {
        if self.origin.is_none() {
            self.origin = self
                .site
                .as_ref()
                .map(|site| match site.host.contains(':') && !site.host.starts_with('[') {
                    true => format!("{}://[{}]:{}", site.scheme, site.host, site.port),
                    false => format!("{}://{}:{}", site.scheme, site.host, site.port),
                });
        }
        self
    }

    /// Whether the `Origin` is the request's own: the same scheme, host and port.
    pub fn own_origin(&self) -> bool {
        match (&self.origin, &self.site) {
            (Some(origin), Some(site)) => SiteOrigin::parse(origin).as_ref() == Some(site),
            _ => false,
        }
    }

    pub fn place(&self) -> Place {
        if self.via == RequestVia::InProcess {
            return Place::Local;
        }
        let loopback_peer = self.peer.is_some_and(|peer| peer.to_canonical().is_loopback());
        if self.ssh {
            return if loopback_peer {
                Place::Local
            } else {
                Place::Remote {
                    secure: true,
                    via_edge: false,
                }
            };
        }
        let loopback_site = self.site.as_ref().is_some_and(|site| is_loopback_host(&site.host));
        if loopback_peer && !self.forwarded && self.via == RequestVia::Direct && self.own_origin() && loopback_site {
            return Place::Local;
        }
        Place::Remote {
            secure: self.https,
            via_edge: self.via == RequestVia::Edge,
        }
    }
}

/// Why a command or host session is not opened or attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not here, for this user, from this place: nothing tells which.
    NotAvailable,
    /// Sign in again first.
    ReauthRequired,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotAvailable => super::services::NOT_AVAILABLE,
            Self::ReauthRequired => "reauthRequired",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::NotAvailable => "That terminal is not available here",
            Self::ReauthRequired => "Confirm it is you: sign in again to open this terminal",
        }
    }
}

/// The conf's say on command and host sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Switches {
    pub command: bool,
    pub host: bool,
    /// `terminal` `remote`: from anywhere but this machine.
    pub remote: bool,
    /// How recent a remote session's sign-in must be.
    pub reauth_millis: i64,
}

impl Switches {
    pub fn of(factory: &ExecutionContextFactory) -> Self {
        let minutes = super::terminal_number(factory, "remote-reauth-minutes", REMOTE_REAUTH_MINUTES);
        Self {
            command: switched_on(factory, "command-line"),
            host: switched_on(factory, "host-sessions"),
            remote: factory.conf_flag("terminal", "remote", false),
            reauth_millis: i64::try_from(minutes).unwrap_or(60) * 60_000,
        }
    }
}

/// A switch of the conf: `dev` (the default) on a dev instance only, `false` off, anything else on.
fn switched_on(factory: &ExecutionContextFactory, name: &str) -> bool {
    match factory.conf_setting_if_set("terminal", name).as_deref().map(str::trim) {
        None | Some("dev") => moqui_core::devtools::is_dev_instance(),
        Some("false") => false,
        Some(_) => true,
    }
}

/// Whether this build carries `kind` (D15): the command line and host sessions only with the dev tools, and a host
/// session only on a Unix server.
fn built_with(kind: Kind) -> bool {
    match kind {
        Kind::Apps => true,
        Kind::Command => cfg!(feature = "devtools"),
        Kind::Host => cfg!(all(feature = "devtools", unix)),
    }
}

/// The kinds this user may open from where `facts` say: what get#TerminalOptions offers.
pub(crate) fn offered(switches: Switches, facts: &Facts, permitted: &dyn Fn(&str) -> bool) -> Vec<Kind> {
    [Kind::Apps, Kind::Command, Kind::Host]
        .into_iter()
        // a sign-in that is not recent is asked for when the session is opened, so it does not hide the kind
        .filter(|kind| {
            !matches!(
                check(switches, *kind, facts, permitted, &|_| true),
                Err(Refusal::NotAvailable)
            )
        })
        .collect()
}

/// May a session of `kind` be opened or attached, from where `facts` say? `permitted` answers a permission of the
/// user, and `recent` whether their sign-in is within the millis given.
pub(crate) fn check(
    switches: Switches,
    kind: Kind,
    facts: &Facts,
    permitted: &dyn Fn(&str) -> bool,
    recent: &dyn Fn(i64) -> bool,
) -> Result<(), Refusal> {
    let (permission, on) = match kind {
        Kind::Apps => return Ok(()),
        Kind::Command => (COMMAND_PERMISSION, switches.command),
        Kind::Host => (HOST_PERMISSION, switches.host),
    };
    if !built_with(kind) || !on || !permitted(permission) {
        return Err(Refusal::NotAvailable);
    }
    // the server's own origin only, whatever allow-origins says (§8.3); the node in process has its own
    let keyed = facts.device_key && facts.origin.is_none();
    if !facts.ssh && !keyed && facts.via != RequestVia::InProcess && !facts.own_origin() {
        return Err(Refusal::NotAvailable);
    }
    match facts.place() {
        Place::Local => Ok(()),
        Place::Remote { secure, .. } => {
            if !switches.remote || !secure {
                return Err(Refusal::NotAvailable);
            }
            if !recent(switches.reauth_millis) {
                return Err(Refusal::ReauthRequired);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
