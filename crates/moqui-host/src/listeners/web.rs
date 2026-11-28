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

//! The web listeners' entries (plans/Runtime-Config.md §3.3, §8.3, RC2): HTTP, HTTPS and gRPC, started in the web
//! pass once `http::listen` has built the app ([`crate::http::WebContext`]), and live from then on: a change of their
//! switches, ports or address starts, stops or binds them again while the server runs, and the process stop stops
//! them together, first. Each reads the factory's running conf at each pass, so two servers in one process keep their
//! own settings; a pass reads them all from one reading of the conf. A failed bind is a Failed row and the others go
//! on, at boot as later (§6), except HTTP on an address its caller named (`start_server`, `moqui test --serve`), which
//! stops the start as before.
//!
//! HTTPS is wanted while its switch is on and either a certificate loads (it serves) or ACME wants it (it answers the
//! challenges alone until one is there); which of the two applies in place, with no new bind. gRPC builds its routes at
//! each start from the services that are on; the services' own switches (`telemetry/receiver@grpc-enabled`,
//! `pass-authority@grpc-enabled`) are read but wait for a restart (RC9), so they are not among its selectors.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::conf_env::{same_selector, switch_on};
use moqui_core::startup::State;
use moqui_lib::error::Error;
use tokio::net::TcpListener;

use super::{Ctx, EndNow, Listener, Now, Running, Stage, StopHow, StopPhase, Wanted};
use crate::http::{ServeTask, WebContext};
use crate::port_check::{Proto, Target};
use crate::ListenBind;

/// The startup report's names, as [`moqui_core::listeners::SWITCHES`] has them, so an Off row's reason and its upgrade
/// warning match ([`moqui_core::listeners::off_reason`]).
pub(crate) const HTTP: &str = moqui_core::listeners::HTTP_SWITCH.name;
pub(crate) const HTTPS: &str = moqui_core::listeners::HTTPS_SWITCH.name;
#[cfg(feature = "grpc")]
pub(crate) const GRPC: &str = moqui_core::listeners::GRPC_SWITCH.name;

/// The row of a web listener of a node reached in process only.
const IN_PROCESS: &str = "no port, reached in process only";

/// The web listeners' switches, ports and address (plans/Runtime-Config.md §3.2), from
/// [`moqui_core::listeners::SWITCHES`]: the Off rows name the switches.
const HTTP_ENABLED: &str = moqui_core::listeners::HTTP_SWITCH.enabled;
const HTTP_PORT: &str = moqui_core::listeners::HTTP_SWITCH.ports[0];
const HTTP_BIND: &str = "webapp-list/webapp[webroot]@http-bind";
const HTTPS_ENABLED: &str = moqui_core::listeners::HTTPS_SWITCH.enabled;
const HTTPS_PORT: &str = moqui_core::listeners::HTTPS_SWITCH.ports[0];
#[cfg(feature = "grpc")]
const GRPC_ENABLED: &str = moqui_core::listeners::GRPC_SWITCH.enabled;
#[cfg(feature = "grpc")]
const GRPC_PORT: &str = moqui_core::listeners::GRPC_SWITCH.ports[0];
#[cfg(feature = "grpc")]
const GRPC_BIND: &str = "webapp-list/webapp[webroot]@grpc-listen-bind";

/// How long a bind on the axum runtime may take before it is given up (`worker::on_runtime`).
const BIND_LIMIT: Duration = Duration::from_secs(30);

/// A setting of the pass's reading of the running conf, trimmed; empty when the conf leaves it out.
fn setting(ctx: &Ctx, selector: &str) -> String {
    ctx.conf
        .value(selector)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// HTTP's switch, port and `http-bind` as the running conf has them.
fn http_settings(ctx: &Ctx) -> [String; 3] {
    [
        setting(ctx, HTTP_ENABLED),
        setting(ctx, HTTP_PORT),
        setting(ctx, HTTP_BIND),
    ]
}

/// Which of HTTP's switch, port and address the boot's bind still holds ([`crate::http::AddressHold`]): an address
/// bind holds each until it is set or a reload changes it, a property bind none. The boot's pass notes what the conf
/// says then.
fn held(ctx: &Ctx, web: &WebContext, settings: &[String; 3]) -> [bool; 3] {
    match ctx.boot.bind {
        ListenBind::Address(_) => {
            if *ctx.cause == moqui_core::listeners::Cause::Boot {
                web.held().note_boot(settings);
            }
            web.held().held()
        }
        ListenBind::Property | ListenBind::None => [false; 3],
    }
}

/// HTTP's switch, port and address, in [`http_settings`]' order, as a set names them.
const HELD_SELECTORS: [&str; 3] = [HTTP_ENABLED, HTTP_PORT, HTTP_BIND];

/// The selectors of a `SetValue` cause, as `listeners::control` joins them.
fn set_selectors(text: &str) -> impl Iterator<Item = &str> {
    text.split(", ").map(str::trim)
}

/// The HTTP settings a server started on an address still holds at that address's (none for another start), which a
/// change that sets one to the conf's value must still write so the hold is released (`ListenerControl::held`).
pub(super) fn held_now(web: &WebContext, bind: &ListenBind) -> Vec<String> {
    if !matches!(bind, ListenBind::Address(_)) {
        return Vec::new();
    }
    HELD_SELECTORS
        .iter()
        .zip(web.held().held())
        .filter(|(_, held)| *held)
        .map(|(selector, _)| (*selector).to_string())
        .collect()
}

/// Before a pass after the start: on a server started on an address, each of HTTP's settings a set of this pass's
/// `causes` names is the conf's from now on, whatever value it was set to, and so is each one a reload changed from
/// what the conf said at the first web pass ([`crate::http::AddressHold`]).
pub(super) fn release_held(ctx: &Ctx, causes: &[moqui_core::listeners::Cause]) {
    use moqui_core::listeners::Cause;
    let (Some(web), ListenBind::Address(_)) = (ctx.web, &ctx.boot.bind) else {
        return;
    };
    let release = |set: &str| {
        for (index, held) in HELD_SELECTORS.iter().enumerate() {
            if same_selector(set, held) {
                web.held().release(index);
            }
        }
    };
    for cause in causes {
        match cause {
            Cause::SetValue(selectors) => {
                for set in set_selectors(selectors) {
                    release(set);
                }
            }
            // a person's change through the listener services, which set its settings holding this pass's hook
            Cause::Request { selectors, .. } => {
                for set in selectors {
                    release(set);
                }
            }
            Cause::ConfReload => web.held().release_changed(&http_settings(ctx)),
            Cause::Boot | Cause::Shutdown | Cause::Signal(_) => {}
        }
    }
}

/// The address the web listeners bind for the boot's `bind`, `http-bind` (`conf_bind`) unless the bind's own address
/// still `holds` it; none for a node reached in process only.
pub(super) fn web_ip(bind: &ListenBind, holds: bool, conf_bind: &str) -> Result<Option<IpAddr>, String> {
    match bind {
        ListenBind::None => Ok(None),
        ListenBind::Address(address) if holds => Ok(Some(address.ip())),
        ListenBind::Address(_) | ListenBind::Property => {
            crate::web_bind_ip(conf_bind).map(Some).map_err(|err| err.to_string())
        }
    }
}

/// What HTTP wants for the boot's `bind`, from its switch, port and address (`settings`), each the bind's own while it
/// `held` it.
pub(super) fn http_wanted(bind: &ListenBind, held: [bool; 3], settings: &[String; 3]) -> Wanted<SocketAddr, ()> {
    let [enabled, port, address] = settings;
    let ip = match web_ip(bind, held[2], address) {
        Ok(Some(ip)) => ip,
        Ok(None) => return Wanted::Off(IN_PROCESS.to_string()),
        Err(err) => return Wanted::Off(err),
    };
    if !held[0] && !switch_on(enabled) {
        return Wanted::Off(format!("{HTTP_ENABLED} is false"));
    }
    let port = match bind {
        ListenBind::Address(address) if held[1] => address.port(),
        _ => match crate::web_listen_port("http-listen-port", port) {
            Ok(port) => port,
            Err(err) => return Wanted::Off(err.to_string()),
        },
    };
    Wanted::On {
        bind: SocketAddr::new(ip, port),
        in_place: (),
    }
}

/// Whether binding `new` would collide with `old` while it is bound: the same port, not one of the kernel's choosing,
/// on the same address or with either on every address. The old then stops before the new binds.
pub(super) fn web_overlaps(old: SocketAddr, new: SocketAddr) -> bool {
    old.port() == new.port()
        && old.port() != 0
        && (old.ip() == new.ip() || old.ip().is_unspecified() || new.ip().is_unspecified())
}

/// The port target of a web listener's `address`, for a failed bind's explanation and the pre-check.
fn target(listener: &'static str, property: &'static str, address: SocketAddr) -> Target {
    Target::optional(listener, property, address.ip(), address.port(), Proto::Tcp)
}

/// Bind `address` on the axum runtime from the pass's plain thread; a failure is explained as the port check explains
/// a busy port, with the processes holding it.
fn bind(web: &WebContext, address: SocketAddr, target: Target) -> Result<(TcpListener, SocketAddr), Error> {
    let bound = super::worker::on_runtime(
        web.handle(),
        BIND_LIMIT,
        async move { TcpListener::bind(address).await },
    )
    .map_err(Error::new)?
    .map_err(|err| Error::new(crate::port_check::explain(&target, &err)))?;
    let local = bound
        .local_addr()
        .map_err(|err| Error::new(format!("the bound listener has no address: {err}")))?;
    Ok((bound, local))
}

/// A web listener's loop, as the registry holds it.
pub(crate) struct WebRun {
    task: ServeTask,
    state: State,
    address: Option<SocketAddr>,
}

impl WebRun {
    /// Its stop's grace: `limits.shutdown` for the process stop, as before the registry, a live stop's own, or none
    /// for "end connections now" (plans/Runtime-Config.md §8.3); a later "end connections now" cuts a grace short.
    /// With the process's stop it has already begun.
    fn begin(&mut self, how: StopHow) {
        self.task.begin_stop(how);
    }
}

impl Running<()> for WebRun {
    fn state(&self) -> State {
        self.state.clone()
    }

    fn address(&self) -> Option<SocketAddr> {
        self.address
    }

    fn begin_stop(&mut self, how: StopHow) {
        self.begin(how);
    }

    fn end_now(&self) -> Option<EndNow> {
        Some(self.task.end_now_handle())
    }

    fn released(&mut self, wait: Duration) -> bool {
        self.task.released(wait)
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.task.finished(wait)
    }

    fn connections(&self) -> Option<usize> {
        Some(self.task.connections())
    }
}

/// HTTP: `named` when the server was started on an address its caller named, whose failed bind stops the start.
pub(crate) struct Http {
    pub(super) named: bool,
}

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

    fn live(&self) -> bool {
        true
    }

    fn fatal_at_boot(&self) -> bool {
        self.named
    }

    fn selectors(&self) -> &'static [&'static str] {
        &HELD_SELECTORS
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<SocketAddr, ()> {
        let Some(web) = ctx.web else {
            return Wanted::NotYet;
        };
        let settings = http_settings(ctx);
        http_wanted(&ctx.boot.bind, held(ctx, web, &settings), &settings)
    }

    fn start(&self, ctx: &Ctx, address: &SocketAddr, _in_place: &()) -> Result<Option<WebRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        let (listener, local) = bind(web, *address, target(HTTP, "http-listen-port", *address))?;
        Ok(Some(WebRun {
            task: web.serve(listener, false, HTTP),
            state: State::On(format!("tcp {local}")),
            address: Some(local),
        }))
    }

    fn overlaps(&self, old: &SocketAddr, new: &SocketAddr) -> bool {
        web_overlaps(*old, *new)
    }

    fn target(&self, address: &SocketAddr) -> Option<Target> {
        Some(target(HTTP, "http-listen-port", *address))
    }
}

/// How HTTPS serves, which changes in place: with a certificate, or with none, answering only ACME's challenges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HttpsMode {
    Serving,
    AcmeOnly,
}

/// What HTTPS wants: off for a node reached in process only (no `ip`), or while its switch is off (`listen`); on at
/// `ip` and its port while a certificate loads (`certificate`, none when its TLS material did not build) or ACME
/// wants it; otherwise off, standing by.
pub(super) fn https_wanted(
    ip: Result<Option<IpAddr>, String>,
    listen: &crate::tls::Listen,
    certificate: Option<bool>,
    acme_wanted: bool,
) -> Wanted<SocketAddr, HttpsMode> {
    let ip = match ip {
        Ok(Some(ip)) => ip,
        Ok(None) => return Wanted::Off(IN_PROCESS.to_string()),
        Err(err) => return Wanted::Off(err),
    };
    let port = match listen.port() {
        Ok(Some(port)) => port,
        Ok(None) => return Wanted::Off(format!("{HTTPS_ENABLED} is false")),
        Err(err) => return Wanted::Off(err.to_string()),
    };
    let mode = match certificate {
        None => return Wanted::Off("its TLS settings did not build at the start (the log says why)".to_string()),
        Some(true) => HttpsMode::Serving,
        Some(false) if acme_wanted => HttpsMode::AcmeOnly,
        Some(false) => return Wanted::Off(crate::http::STANDBY.to_string()),
    };
    Wanted::On {
        bind: SocketAddr::new(ip, port),
        in_place: mode,
    }
}

/// HTTPS's loop: a web loop whose row follows its mode, and ACME's record of its address.
pub(crate) struct HttpsRun {
    run: WebRun,
    mode: HttpsMode,
    acme: Arc<crate::acme::Acme>,
    /// What its row adds while it serves: the gRPC services it carries, and on an edge node that takes its links only
    /// after a restart, that.
    serving_note: String,
}

impl HttpsRun {
    fn state_of(local: Option<SocketAddr>, mode: HttpsMode, serving_note: &str) -> State {
        let shown = local.map_or_else(String::new, |local| local.to_string());
        match mode {
            HttpsMode::Serving => State::On(format!("tcp {shown} TLS{serving_note}")),
            HttpsMode::AcmeOnly => State::On(format!("tcp {shown} TLS, started for ACME")),
        }
    }
}

impl Running<HttpsMode> for HttpsRun {
    fn state(&self) -> State {
        Self::state_of(self.run.address, self.mode, &self.serving_note)
    }

    fn address(&self) -> Option<SocketAddr> {
        self.run.address
    }

    /// A certificate came, or the last one went: the same socket serves, or answers ACME's challenges alone.
    fn apply(&mut self, mode: &HttpsMode) {
        self.mode = *mode;
        self.run.state = self.state();
    }

    fn begin_stop(&mut self, how: StopHow) {
        self.run.begin(how);
    }

    fn end_now(&self) -> Option<EndNow> {
        Some(self.run.task.end_now_handle())
    }

    fn released(&mut self, wait: Duration) -> bool {
        self.run.task.released(wait)
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.run.task.finished(wait)
    }

    fn connections(&self) -> Option<usize> {
        Some(self.run.task.connections())
    }
}

/// ACME's record of the listener's address goes with it, unless a new bind has taken its place already.
impl Drop for HttpsRun {
    fn drop(&mut self) {
        if self.acme.listening() == self.run.address {
            self.acme.set_listening(None);
        }
    }
}

pub(crate) struct Https;

impl Listener for Https {
    type Bind = SocketAddr;
    type InPlace = HttpsMode;
    type Run = HttpsRun;

    fn name(&self) -> &'static str {
        HTTPS
    }

    fn stage(&self) -> Stage {
        Stage::Web
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::Web
    }

    fn live(&self) -> bool {
        true
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[HTTPS_ENABLED, HTTPS_PORT, HTTP_BIND]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<SocketAddr, HttpsMode> {
        let Some(web) = ctx.web else {
            return Wanted::NotYet;
        };
        let settings = http_settings(ctx);
        let holds = held(ctx, web, &settings)[2];
        let ip = web_ip(&ctx.boot.bind, holds, &settings[2]);
        let listen = crate::tls::Listen::of(&|name: &str| setting(ctx, &format!("webapp-list/webapp[webroot]@{name}")));
        let acme_wanted = *crate::acme::Acme::of(ctx.factory).wanted().borrow();
        https_wanted(ip, &listen, web.has_certificate(), acme_wanted)
    }

    fn start(&self, ctx: &Ctx, address: &SocketAddr, mode: &HttpsMode) -> Result<Option<HttpsRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        let (listener, local) = bind(web, *address, target(HTTPS, "https-listen-port", *address))?;
        let acme = crate::acme::Acme::of(ctx.factory);
        acme.set_listening(Some(local));
        let mut serving_note = web
            .https_grpc()
            .map(|names| format!(", gRPC over HTTP/2: {names}"))
            .unwrap_or_default();
        if web.edge_links_wait() {
            serving_note.push_str("; edge links are taken after a restart");
        }
        let run = WebRun {
            task: web.serve(listener, true, HTTPS),
            state: HttpsRun::state_of(Some(local), *mode, &serving_note),
            address: Some(local),
        };
        Ok(Some(HttpsRun {
            run,
            mode: *mode,
            acme,
            serving_note,
        }))
    }

    fn overlaps(&self, old: &SocketAddr, new: &SocketAddr) -> bool {
        web_overlaps(*old, *new)
    }

    fn target(&self, address: &SocketAddr) -> Option<Target> {
        Some(target(HTTPS, "https-listen-port", *address))
    }

    /// ACME may take a name while HTTPS answers its challenges: bound, or waiting for it with no certificate yet.
    fn published(&self, ctx: &Ctx, now: Now<'_, HttpsMode>) {
        use crate::acme::HttpsState;
        let state = match now {
            Now::Running(HttpsMode::Serving) => HttpsState::Serving,
            Now::Running(HttpsMode::AcmeOnly) => HttpsState::AcmeOnly,
            Now::Off(reason) if reason == crate::http::STANDBY => HttpsState::StandingBy,
            Now::Off(_) | Now::NotYet => HttpsState::Off,
            Now::Failed => HttpsState::Failed,
        };
        crate::acme::Acme::of(ctx.factory).set_https(state);
    }
}

/// Where the gRPC listener binds and what it serves: with the HTTPS listener's TLS while a certificate loads, the
/// OTLP receiver's routes and the PASS authority's, each when on (`pass` only where its secrets are safe: TLS, a
/// loopback address, or `grpc-insecure`). A change of any binds it again with its routes built anew.
#[cfg(feature = "grpc")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GrpcBind {
    address: SocketAddr,
    tls: bool,
    otlp: bool,
    pass: bool,
}

/// gRPC: the services' switches as the boot read them, which wait for a restart, and its TLS, which follows HTTPS's
/// switch and a certificate but changes only with its own settings or that switch, so a certificate coming or going
/// does not bind it again and drop its h2c clients.
#[cfg(feature = "grpc")]
#[derive(Default)]
pub(crate) struct Grpc {
    services: std::sync::OnceLock<GrpcServices>,
    tls: std::sync::Mutex<Option<(GrpcSettings, bool)>>,
}

/// Which of gRPC's services are on, and whether the PASS authority may serve in clear, as the boot read their
/// switches (restart-class, RC9).
#[cfg(feature = "grpc")]
#[derive(Clone, Copy, Debug)]
struct GrpcServices {
    otlp: bool,
    pass: bool,
    /// `pass-authority@grpc-insecure`.
    pass_insecure: bool,
}

/// gRPC's own settings and HTTPS's switch, as a pass read them: its TLS is decided again only when they change.
#[cfg(feature = "grpc")]
type GrpcSettings = [String; 5];

#[cfg(feature = "grpc")]
impl Grpc {
    /// Whether gRPC serves TLS: with the HTTPS listener's settings while HTTPS's switch is on and a certificate loads,
    /// as decided when `settings` last changed.
    fn tls(&self, settings: GrpcSettings, https_on: bool, web: &WebContext) -> bool {
        let mut decided = moqui_lib::sync::lock(&self.tls);
        if let Some((seen, tls)) = decided.as_ref() {
            if *seen == settings {
                return *tls;
            }
        }
        let tls = https_on && web.grpc_tls().is_some() && web.has_certificate() == Some(true);
        *decided = Some((settings, tls));
        tls
    }
}

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

    fn live(&self) -> bool {
        true
    }

    /// Its switch, port and address, and `http-bind`, whose address it shares when it names none; the services' own
    /// switches wait for a restart (RC9).
    fn selectors(&self) -> &'static [&'static str] {
        &[GRPC_ENABLED, GRPC_PORT, GRPC_BIND, HTTP_BIND]
    }

    /// The PASS authority's h2c refusal is logged by the boot's pass only, so it is logged once; a port busy at the
    /// start check is skipped at boot, and tried again by a later pass.
    fn wanted(&self, ctx: &Ctx) -> Wanted<GrpcBind, ()> {
        let Some(web) = ctx.web else {
            return Wanted::NotYet;
        };
        let boot = *ctx.cause == moqui_core::listeners::Cause::Boot;
        let settings = http_settings(ctx);
        let holds = held(ctx, web, &settings)[2];
        let web_ip = match web_ip(&ctx.boot.bind, holds, &settings[2]) {
            Ok(Some(ip)) => ip,
            Ok(None) => return Wanted::Off(IN_PROCESS.to_string()),
            Err(err) => return Wanted::Off(err),
        };
        if !switch_on(&setting(ctx, GRPC_ENABLED)) {
            return Wanted::Off(format!("{GRPC_ENABLED} is false"));
        }
        let port: u16 = match setting(ctx, GRPC_PORT).parse() {
            Ok(port) => port,
            Err(_) => {
                return Wanted::Off(format!(
                    "{GRPC_PORT} \"{}\" is not a port (set {GRPC_ENABLED}=\"false\" to keep it closed)",
                    setting(ctx, GRPC_PORT)
                ))
            }
        };
        let ip = match setting(ctx, GRPC_BIND).as_str() {
            "" => web_ip,
            text => match text.parse() {
                Ok(ip) => ip,
                Err(_) => return Wanted::Off(format!("{GRPC_BIND} {text} is not an IP address")),
            },
        };
        let address = SocketAddr::new(ip, port);
        let https_switch = setting(ctx, HTTPS_ENABLED);
        let https_on = switch_on(&https_switch);
        let tls = self.tls(
            [
                setting(ctx, GRPC_ENABLED),
                setting(ctx, GRPC_PORT),
                setting(ctx, GRPC_BIND),
                settings[2].clone(),
                https_switch,
            ],
            https_on,
            web,
        );
        let services = *self.services.get_or_init(|| GrpcServices {
            otlp: crate::monitor::grpc::enabled(ctx.factory),
            pass: crate::pass_authority::direct_enabled(),
            pass_insecure: setting(ctx, crate::pass_authority::INSECURE_PROPERTY) == "true",
        });
        let otlp = services.otlp;
        let pass = services.pass && crate::grpc_listen::pass_allowed(address, tls, services.pass_insecure, boot);
        if !otlp && !pass {
            return Wanted::Off(
                "no dedicated gRPC service is enabled (telemetry/receiver@grpc-enabled, pass-authority@grpc-enabled)"
                    .to_string(),
            );
        }
        if boot {
            if let Some(busy) = crate::port_check::busy(GRPC) {
                return Wanted::Skipped(busy);
            }
        }
        Wanted::On {
            bind: GrpcBind {
                address,
                tls,
                otlp,
                pass,
            },
            in_place: (),
        }
    }

    fn start(&self, ctx: &Ctx, bind: &GrpcBind, _in_place: &()) -> Result<Option<WebRun>, Error> {
        let Some(web) = ctx.web else {
            return Ok(None);
        };
        let factory = Arc::clone(ctx.factory);
        let (address, services) = (bind.address, (bind.otlp, bind.pass));
        let tls = web.grpc_tls().filter(|_| bind.tls);
        let encrypted = tls.is_some();
        let (bound, local) = super::worker::on_runtime(web.handle(), BIND_LIMIT, async move {
            crate::grpc_listen::open(&factory, address, tls, services).await
        })
        .map_err(Error::new)?
        .map_err(Error::new)?;
        let state = State::on(format!(
            "tcp {local}{}, {}",
            if encrypted { " TLS" } else { "" },
            bound.label()
        ));
        Ok(Some(WebRun {
            task: web.serve_grpc(bound),
            state,
            address: Some(local),
        }))
    }

    fn overlaps(&self, old: &GrpcBind, new: &GrpcBind) -> bool {
        web_overlaps(old.address, new.address)
    }

    fn target(&self, bind: &GrpcBind) -> Option<Target> {
        Some(target(GRPC, "grpc-listen-port", bind.address))
    }
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
