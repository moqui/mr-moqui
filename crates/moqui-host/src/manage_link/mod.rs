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

//! The manager link (plans/archive/Vault-Instances.md §4.4, M2): a managed instance dials its manager's HTTPS port and keeps one
//! connection open, gRPC on HTTP/2, as the edge link does (`crate::link` has what the two share).
//!
//! - **TLS.** ALPN [`ALPN`] (`moqui-manage/1`) on the manager's HTTPS listener ([`accept::Acceptor`]), on a self-signed
//!   certificate made from the manager's link key; the instance presents its own. Both keys are pinned by SHA-256 of the
//!   public key: the instance pins the manager's from `manage@pin` (a provision sets it, and a pairing code is shown with
//!   it), the manager pins the instance's on its `AppInstance` row at pairing. A key not pinned is refused.
//! - **Pairing** ([`ROLE_PAIR`]). The one-time token a provision gave the new container as a file
//!   (`manage@token-file`), or a code made in System › Instances for an instance the manager did not make: it works
//!   once, within 15 minutes, for that instance only (`moqui_instance::link::pair`). The instance also gives the X25519
//!   key its bootstrap bundle is sealed to, and takes and pins its environment's approvers' signing keys; a changed set
//!   reaches it only signed by an approver it pinned (`moqui_vault::approvers`).
//! - **The link** ([`ROLE_UP`]). The instance is the HTTP/2 client of `Manager` (`proto/manage.proto`): Hello, Status,
//!   Bootstrap, Unseal and Event; the manager's calls of the instance go down the Serve stream. Each new link of an
//!   instance has a higher generation and closes the one before.
//! - **Sealed.** While the instance is sealed it dials out and opens no listener (Sealed-Secrets §3.3), and only the
//!   calls of [`Call::sealed_ok`] are answered: Hello and status, the bootstrap bundle on a first start, and unseal (by
//!   transit, or by recovery shares sealed to the instance's unseal key). Every other call needs the instance unsealed,
//!   on both ends.
//! - **Unsealed, as the manager takes it** (M6). The manager runs a command that needs an unsealed instance (all but
//!   status and seal) only on a link it unsealed by transit, or on which the instance proved it holds its root key
//!   ([`unseal`], `moqui_instance::transit`); a claim of unsealed without either is taken as sealed.
//! - **Recovery shares** ([`shares`], M6): the instance seals each share to one holder whose key an approver it pinned
//!   checked, sends it as a delivery signed with its link key, and checks each holder's confirmation itself.
//! - **Commands** ([`commands`]). The manager's calls run on the instance as one system identity that runs only the
//!   commands of [`CommandKind`]; nothing on the link runs an arbitrary service. A command that is none of them is
//!   refused.
//! - **The bootstrap bundle** ([`bundle`]): the instance's database password and the other secrets it needs before its
//!   database opens, sealed to its bundle key (HPKE), kept with the seal's bootstrap API in `seal.json`; never `.env`.
//! - **The link key** ([`key`]) works while the instance is sealed, so it is not under the root key: a file in
//!   `runtime/db` of mode 0600, or a `systemd-creds` credential bound to the host's TPM (`manage@key-store`). Without a
//!   TPM it is a usable key on disk, the exception the plan names (§4.4, the limit of that key).

use std::time::Duration;

pub mod bundle;
pub(crate) mod commands;
pub mod dial;
pub(crate) mod key;
pub mod proto;
pub(crate) mod services;
pub(crate) mod shares;
pub(crate) mod state;
pub mod unseal;

#[cfg(feature = "instances")]
pub(crate) mod accept;
#[cfg(not(feature = "instances"))]
#[path = "accept_off.rs"]
pub(crate) mod accept;

pub use crate::link::Backoff;
pub use commands::Managed;
pub use dial::{pair, Dialer, LinkPhase, LinkStatus, ManageConfig};
pub use key::{KeyStore, LinkKey};

/// The ALPN protocol of the manager link on the manager's HTTPS port.
pub const ALPN: &[u8] = b"moqui-manage/1";
/// The link protocol version. Protobuf changes are additive only; the two ends refuse a link more than one apart.
pub const LINK_VERSION: u32 = 1;
/// The role byte of a pairing connection: Pair alone is served.
pub const ROLE_PAIR: u8 = b'P';
/// The role byte of a link connection: the instance's key must be pinned on the manager.
pub const ROLE_UP: u8 = b'U';

/// The `:authority` of the link's gRPC requests: the connection is already the right one.
const ORIGIN: &str = "https://moqui-manage.link";

/// The largest gRPC message either end sends: a seal bundle or a deploy's report.
pub(crate) const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// The file of the link key in the runtime's `db` directory.
pub const KEY_FILE: &str = "manage_link_key";
/// The file of what the instance pinned at pairing, in the runtime's `db` directory.
pub const STATE_FILE: &str = "manage_link.json";

/// How the link reads its settings: from the merged conf at the sealed gate, before a factory runs
/// ([`gate_settings`]), else as a running server does ([`server_setting`]).
pub type Settings<'a> = &'a dyn Fn(&str) -> String;

/// A setting of the running server.
#[must_use]
pub fn server_setting(selector: &str) -> String {
    moqui_core::context::conf_env::setting(selector).trim().to_string()
}

/// The settings of the merged conf the gate is given, the environment names applied over it.
pub(crate) fn gate_settings(conf: &moqui_lib::node::MNode) -> impl Fn(&str) -> String + '_ {
    move |selector: &str| moqui_core::context::conf_env::conf_attribute(conf, selector)
}

/// The calls of `Manager`, the instance's of the manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Pair,
    Hello,
    Status,
    Bootstrap,
    Unseal,
    SealedShares,
    Holders,
    Shares,
    ShareConfirmations,
    RotateProof,
    Event,
    Serve,
}

impl Call {
    /// Whether the call is answered while the instance is sealed (§4.4): Hello and status, the bootstrap bundle on a
    /// first start, and unseal, by transit or by recovery shares. Pair is a pairing connection's alone.
    #[must_use]
    pub fn sealed_ok(self) -> bool {
        matches!(
            self,
            Self::Hello | Self::Status | Self::Bootstrap | Self::Unseal | Self::SealedShares
        )
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pair => "Pair",
            Self::Hello => "Hello",
            Self::Status => "Status",
            Self::Bootstrap => "Bootstrap",
            Self::Unseal => "Unseal",
            Self::SealedShares => "SealedShares",
            Self::Holders => "Holders",
            Self::Shares => "Shares",
            Self::ShareConfirmations => "ShareConfirmations",
            Self::RotateProof => "RotateProof",
            Self::Event => "Event",
            Self::Serve => "Serve",
        }
    }
}

/// The commands the manager may run on an instance (§4.4, Manager to instance), and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Status,
    Seal,
    Restart,
    Drain,
    Undrain,
    PutSecret,
    DeployComponent,
    ExportSeal,
    SurfaceReport,
    Approvers,
}

impl CommandKind {
    /// The command a message carries, or none: an empty one, or one of a later version that this binary does not
    /// know, which is refused.
    #[must_use]
    pub fn of(command: &proto::Command) -> Option<Self> {
        use proto::command::Body;
        Some(match command.body.as_ref()? {
            Body::Status(_) => Self::Status,
            Body::Seal(_) => Self::Seal,
            Body::Restart(_) => Self::Restart,
            Body::Drain(drain) if drain.drain => Self::Drain,
            Body::Drain(_) => Self::Undrain,
            Body::PutSecret(_) => Self::PutSecret,
            Body::DeployComponent(_) => Self::DeployComponent,
            Body::ExportSeal(_) => Self::ExportSeal,
            Body::SurfaceReport(_) => Self::SurfaceReport,
            Body::Approvers(_) => Self::Approvers,
        })
    }

    /// The command by the name a service call gives it.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "status" => Self::Status,
            "seal" => Self::Seal,
            "restart" => Self::Restart,
            "drain" => Self::Drain,
            "undrain" => Self::Undrain,
            "put-secret" => Self::PutSecret,
            "deploy-component" => Self::DeployComponent,
            "export-seal" => Self::ExportSeal,
            "surface-report" => Self::SurfaceReport,
            "approvers" => Self::Approvers,
            _ => return None,
        })
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Seal => "seal",
            Self::Restart => "restart",
            Self::Drain => "drain",
            Self::Undrain => "undrain",
            Self::PutSecret => "put-secret",
            Self::DeployComponent => "deploy-component",
            Self::ExportSeal => "export-seal",
            Self::SurfaceReport => "surface-report",
            Self::Approvers => "approvers",
        }
    }

    /// Whether the command runs while the instance is sealed: status alone. Seal is moot then, and the others need the
    /// root key or the database.
    #[must_use]
    pub fn sealed_ok(self) -> bool {
        self == Self::Status
    }

    /// Whether the manager runs it only on a link whose "unsealed" it verified (M6): all but status and seal, which
    /// give nothing to a holder of the link key alone.
    #[must_use]
    pub fn needs_verified_unseal(self) -> bool {
        !matches!(self, Self::Status | Self::Seal)
    }

    /// How long the manager waits for the command's answer.
    #[must_use]
    pub fn timeout(self) -> Duration {
        match self {
            Self::DeployComponent => Duration::from_secs(600),
            _ => Duration::from_secs(60),
        }
    }
}

/// Ok when `theirs` is at most one link protocol version from `ours`, else what to tell the operator.
pub fn check_versions(ours: u32, theirs: u32, peer: &str) -> Result<(), String> {
    if theirs == 0 {
        return Err(format!("the {peer} sent no manager link protocol version"));
    }
    if ours.abs_diff(theirs) > 1 {
        return Err(format!(
            "the {peer} speaks manager link protocol version {theirs} and this server {ours}: more than one version apart"
        ));
    }
    Ok(())
}

/// The `:authority` of the link's gRPC calls.
pub(crate) fn origin() -> http::Uri {
    http::Uri::from_static(ORIGIN)
}

/// The instance's client of `Manager` on a connection, with the link's message limits.
pub(crate) fn manager_client(send: hyper::client::conn::http2::SendRequest<tonic::body::Body>) -> ManagerClient {
    proto::manager_client::ManagerClient::with_origin(crate::link::H2Channel(send), origin())
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE)
}

pub(crate) type ManagerClient = proto::manager_client::ManagerClient<crate::link::H2Channel>;

/// Why a link could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The manager refused this instance: its key is not pinned or was revoked, its pairing secret is unknown, used or
    /// expired, or the manager's key is not the one pinned. Dialing again does not help until an operator acts.
    Refused(String),
    /// Anything else: the network, a timeout, the manager down.
    Failed(String),
}

impl LinkError {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Refused(message) | Self::Failed(message) => message,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// The manager's address as `host` and `port`: `https://host[:port]`, 443 by default.
pub(crate) fn parse_url(url: &str) -> Result<(String, u16), String> {
    let text = url.trim();
    let uri: http::Uri = text
        .parse()
        .map_err(|_| format!("the manager URL {text} is not a URL"))?;
    if uri.scheme_str() != Some("https") {
        return Err(format!("the manager URL {text} is not https"));
    }
    let host = uri
        .host()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']').to_string())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| format!("the manager URL {text} has no host"))?;
    Ok((host, uri.port_u16().unwrap_or(443)))
}

/// What [`start`] gives the HTTPS listener: the manager's hand-off of `moqui-manage/1`, on a manager.
#[derive(Default)]
pub(crate) struct Started {
    pub acceptor: Option<std::sync::Arc<accept::Acceptor>>,
}

/// At the HTTP server's start: on a manager (the `instances` feature) the link's acceptor and the manager's address for
/// the provisions; on a managed instance (`manage@url` set, or a pairing token there) the dialer, unless the sealed
/// gate started it already.
pub(crate) fn start(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    edge_node: bool,
) -> Started {
    dial::start_for_server(
        factory.runtime_dir(),
        tokio::runtime::Handle::current(),
        shutdown.clone(),
    );
    Started {
        acceptor: accept::start(factory, shutdown, edge_node),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
