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

//! The web listeners' entries (plans/Runtime-Config.md §3.3, §8.1, RC0-C): HTTP, HTTPS (or its standby for ACME) and
//! gRPC, started in the web pass once `http::listen` has built the app ([`crate::http::WebContext`]) and stopped
//! together, first, at the process stop. Their rows read as `http::listen` wrote them before the registry. A failed
//! HTTP or HTTPS bind still stops the start until RC2 makes them live; gRPC's records its own failure and the others go
//! on. The HTTPS plan is the boot's (`tls::plan`, which makes a dev certificate); the HTTP bind is read again from the
//! conf at each pass, as the boot read it.

use std::net::SocketAddr;
use std::time::Duration;

use moqui_core::context::conf_env::webroot_setting;
use moqui_core::startup::State;
use moqui_lib::error::Error;
use tokio::net::TcpListener;

use super::{Ctx, Listener, Running, Stage, StopHow, StopPhase, Wanted};
use crate::http::{HttpsPlan, ServeTask, WebContext};
use crate::port_check::{Proto, Target};

/// The startup report's names.
pub(crate) const HTTP: &str = "HTTP";
pub(crate) const HTTPS: &str = "HTTPS";
#[cfg(feature = "grpc")]
const GRPC: &str = "gRPC";

/// The row of a web listener of a node reached in process only.
const IN_PROCESS: &str = "no port, reached in process only";

/// How long a bind on the axum runtime may take before it is given up (`worker::on_runtime`).
const BIND_LIMIT: Duration = Duration::from_secs(30);

/// A web listener's loop, as the registry holds it.
pub(crate) struct WebRun {
    task: ServeTask,
    state: State,
    address: Option<SocketAddr>,
    /// HTTPS standing by records its own row as it binds and closes for ACME.
    own_row: bool,
}

impl<P> Running<P> for WebRun {
    fn state(&self) -> State {
        self.state.clone()
    }

    fn address(&self) -> Option<SocketAddr> {
        self.address
    }

    /// The loop's own stop; with the process's stop it has already begun. The grace is `limits.shutdown` as before
    /// the registry; a live stop's comes in RC2.
    fn begin_stop(&mut self, _how: StopHow) {
        self.task.begin_stop();
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.task.finished(wait)
    }

    fn records_own_row(&self) -> bool {
        self.own_row
    }
}

/// Bind `bind` on the axum runtime from the pass's plain thread, with the text `http::listen` gave a failed bind.
fn bind(web: &WebContext, bind: SocketAddr, unaddressed: &str) -> Result<(TcpListener, SocketAddr), Error> {
    let bound = super::worker::on_runtime(web.handle(), BIND_LIMIT, async move { TcpListener::bind(bind).await })
        .map_err(Error::new)?
        .map_err(|err| Error::new(format!("could not bind {bind}: {err}")))?;
    let local = bound
        .local_addr()
        .map_err(|err| Error::new(format!("{unaddressed}: {err}")))?;
    Ok((bound, local))
}

pub(crate) struct Http;

impl Listener for Http {
    type Bind = SocketAddr;
    type InPlace = ();
    type Run = WebRun;

    fn name(&self) -> &'static str {
        HTTP
    }

    fn stage(&self) -> Stage {
        Stage::Web
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::Web
    }

    fn fatal_at_boot(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[
            "webapp-list/webapp[webroot]@http-listen-port",
            "webapp-list/webapp[webroot]@http-port",
            "webapp-list/webapp[webroot]@http-bind",
        ]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<SocketAddr, ()> {
        if ctx.web.is_none() {
            return Wanted::NotYet;
        }
        http_wanted(&ctx.boot.bind, &webroot_setting)
    }

    fn start(&self, ctx: &Ctx, address: &SocketAddr, _in_place: &()) -> Result<Option<WebRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        let (listener, local) = bind(web, *address, "bound listener has no address")?;
        Ok(Some(WebRun {
            task: web.serve(listener, false),
            state: State::On(format!("tcp {local}")),
            address: Some(local),
            own_row: false,
        }))
    }

    fn overlaps(&self, old: &SocketAddr, new: &SocketAddr) -> bool {
        old.port() == new.port()
    }

    fn target(&self, address: &SocketAddr) -> Option<Target> {
        Some(Target::optional(
            HTTP,
            "http-listen-port",
            address.ip(),
            address.port(),
            Proto::Tcp,
        ))
    }
}

/// What the conf wants of HTTP for the boot's `bind`, its webroot settings read through `webroot`.
pub(super) fn http_wanted(bind: &crate::ListenBind, webroot: &dyn Fn(&str) -> String) -> Wanted<SocketAddr, ()> {
    match crate::listen_address(bind, webroot) {
        Ok(None) => Wanted::Off(IN_PROCESS.to_string()),
        Ok(Some(crate::WebBind { http_port: None, .. })) => Wanted::Off("http-listen-port".to_string()),
        Ok(Some(crate::WebBind {
            ip,
            http_port: Some(port),
        })) => Wanted::On {
            bind: SocketAddr::new(ip, port),
            in_place: (),
        },
        Err(err) => Wanted::Off(err.to_string()),
    }
}

/// Where HTTPS binds, and whether it stands by for ACME with no certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HttpsBind {
    pub(super) address: SocketAddr,
    pub(super) standby: bool,
}

/// What the boot's HTTPS `plan` gives for the boot's `bind`.
pub(super) fn https_wanted(bind: &crate::ListenBind, plan: Option<HttpsPlan>) -> Wanted<HttpsBind, ()> {
    if matches!(bind, crate::ListenBind::None) {
        return Wanted::Off(IN_PROCESS.to_string());
    }
    match plan {
        Some(plan) => Wanted::On {
            bind: HttpsBind {
                address: plan.bind,
                standby: plan.standby,
            },
            in_place: (),
        },
        None => {
            Wanted::Off("no certificate in runtime/cert, or the webroot webapp's https-listen-port is off".to_string())
        }
    }
}

pub(crate) struct Https;

impl Listener for Https {
    type Bind = HttpsBind;
    type InPlace = ();
    type Run = WebRun;

    fn name(&self) -> &'static str {
        HTTPS
    }

    fn stage(&self) -> Stage {
        Stage::Web
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::Web
    }

    /// A bind of the plan that serves stops the start; standing by binds nothing at the start.
    fn fatal_at_boot(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[
            "webapp-list/webapp[webroot]@https-listen-port",
            "webapp-list/webapp[webroot]@http-bind",
        ]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<HttpsBind, ()> {
        let Some(web) = ctx.web else {
            return Wanted::NotYet;
        };
        https_wanted(&ctx.boot.bind, web.https_plan())
    }

    fn start(&self, ctx: &Ctx, bind: &HttpsBind, _in_place: &()) -> Result<Option<WebRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        if bind.standby {
            // its row first, as before the registry: standing by records its own from then on, as ACME wants it
            moqui_core::startup::listener(ctx.factory, HTTPS, State::off(crate::http::STANDBY));
            return Ok(Some(WebRun {
                task: web.stand_by(bind.address),
                state: State::off(crate::http::STANDBY),
                address: None,
                own_row: true,
            }));
        }
        let (listener, local) = bind_https(web, bind.address)?;
        crate::acme::Acme::of(ctx.factory).set_listening(Some(local));
        let grpc = web
            .https_grpc()
            .map(|names| format!(", gRPC over HTTP/2: {names}"))
            .unwrap_or_default();
        Ok(Some(WebRun {
            task: web.serve(listener, true),
            state: State::On(format!("tcp {local} TLS{grpc}")),
            address: Some(local),
            own_row: false,
        }))
    }

    fn overlaps(&self, old: &HttpsBind, new: &HttpsBind) -> bool {
        old.address.port() == new.address.port()
    }

    fn target(&self, bind: &HttpsBind) -> Option<Target> {
        Some(Target::optional(
            HTTPS,
            "https-listen-port",
            bind.address.ip(),
            bind.address.port(),
            Proto::Tcp,
        ))
    }
}

fn bind_https(web: &WebContext, address: SocketAddr) -> Result<(TcpListener, SocketAddr), Error> {
    bind(web, address, "bound HTTPS listener has no address")
}

/// Where the gRPC listener binds, and whether it has the HTTPS listener's TLS.
#[cfg(feature = "grpc")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GrpcBind {
    address: SocketAddr,
    tls: bool,
}

#[cfg(feature = "grpc")]
pub(crate) struct Grpc;

#[cfg(feature = "grpc")]
impl Listener for Grpc {
    type Bind = GrpcBind;
    type InPlace = ();
    type Run = WebRun;

    fn name(&self) -> &'static str {
        GRPC
    }

    fn stage(&self) -> Stage {
        Stage::Web
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::Web
    }

    fn selectors(&self) -> &'static [&'static str] {
        &["webapp-list/webapp[webroot]@grpc-listen-port"]
    }

    /// The PASS authority's h2c refusal is logged by the boot's reading only, so it is logged once.
    fn wanted(&self, ctx: &Ctx) -> Wanted<GrpcBind, ()> {
        let Some(web) = ctx.web else {
            return Wanted::NotYet;
        };
        let off = || Wanted::Off("grpc-listen-port is off or no dedicated gRPC service is enabled".to_string());
        let Ok(Some(web_bind)) = crate::listen_address(&ctx.boot.bind, &webroot_setting) else {
            return off();
        };
        let tls = web.grpc_tls().is_some();
        let address = if *ctx.cause == moqui_core::listeners::Cause::Boot {
            crate::grpc_listen::bind_address(ctx.factory, web_bind.ip, tls)
        } else {
            let setting = |path: &str, name: &str| ctx.factory.conf_setting(path, name);
            crate::grpc_listen::bind_address_by(&setting, &webroot_setting, web_bind.ip, tls, false)
        };
        match address {
            Some(address) => Wanted::On {
                bind: GrpcBind { address, tls },
                in_place: (),
            },
            None => off(),
        }
    }

    /// `grpc_listen::open` records its row, a failed bind or a busy port included, and answers none for those.
    fn start(&self, ctx: &Ctx, bind: &GrpcBind, _in_place: &()) -> Result<Option<WebRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        let factory = std::sync::Arc::clone(ctx.factory);
        let (address, tls) = (bind.address, web.grpc_tls().filter(|_| bind.tls));
        let opened = super::worker::on_runtime(web.handle(), BIND_LIMIT, async move {
            crate::grpc_listen::open(&factory, address, tls).await
        })
        .map_err(Error::new)?;
        let Some(bound) = opened else {
            return Ok(None);
        };
        let local = *moqui_lib::sync::lock(&ctx.factory.extension(|_| crate::grpc_listen::GrpcAddress::default()).0);
        Ok(Some(WebRun {
            task: web.serve_grpc(bound),
            state: super::recorded_state(ctx.factory, GRPC),
            address: local,
            own_row: false,
        }))
    }

    fn overlaps(&self, old: &GrpcBind, new: &GrpcBind) -> bool {
        old.address.port() == new.address.port()
    }

    fn target(&self, bind: &GrpcBind) -> Option<Target> {
        Some(Target::optional(
            GRPC,
            "grpc-listen-port",
            bind.address.ip(),
            bind.address.port(),
            Proto::Tcp,
        ))
    }
}
