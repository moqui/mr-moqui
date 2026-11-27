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

//! The entries of the Comms stage (plans/Runtime-Config.md §8.1, RC0): call media (with its TURN server), SIP, XMPP and
//! SMTP, after the cluster so they register under its instance id, in that order. Each wraps today's start, which reads
//! the conf, records its own startup report rows (off, skipped or failed included) and answers none when it does not
//! run; a build without its feature has an `_off` start that records it as not built (SMTP's, here). None stops the
//! boot. They stop
//! as `ServeProcess` stopped them: XMPP, SIP, then call media before the factory (SIP's legs are in call media), and
//! SMTP first after it.

use std::net::SocketAddr;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::startup::State;
use moqui_lib::error::Error;

use super::{Ctx, Listener, Stage, StopPhase, Wanted, Wrapped};
use crate::port_check::Target;

/// The startup report's names, as each start records them.
const CALL_MEDIA: &str = "Call media";
const TURN: &str = "TURN";
const SIP: &str = "SIP";
const XMPP: &str = "XMPP";
const SMTP: &str = "SMTP";

/// Each wrapped start reads the conf and finds itself off.
fn on() -> Wanted<(), ()> {
    Wanted::On { bind: (), in_place: () }
}

/// Wrap what a start of today returned: none (it recorded why itself), or its handle with the row it recorded.
fn wrapped<T: Send>(
    factory: &ExecutionContextFactory,
    name: &str,
    started: Option<T>,
    address: impl FnOnce(&T) -> Option<SocketAddr>,
) -> Option<Wrapped<T>> {
    started.map(|handle| {
        let address = address(&handle);
        Wrapped::new(handle, super::recorded_state(factory, name), address)
    })
}

pub(crate) struct CallMedia;

impl Listener for CallMedia {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<crate::rtc::RtcNode>;

    fn name(&self) -> &'static str {
        CALL_MEDIA
    }

    fn stage(&self) -> Stage {
        Stage::Comms
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::BeforeFactory
    }

    /// Last before the factory: its last act deregisters the node, and SIP's legs are in it.
    fn stop_rank(&self) -> u8 {
        2
    }

    fn extra_rows(&self) -> &'static [&'static str] {
        &[TURN]
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[
            "media-server@enabled",
            "media-server@port",
            "media-server@tcp-port",
            "media-server/turn-server@enabled",
            "media-server/turn-server@port",
        ]
    }

    fn wanted(&self, _ctx: &Ctx) -> Wanted<(), ()> {
        on()
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Self::Run>, Error> {
        Ok(wrapped(ctx.factory, CALL_MEDIA, crate::rtc::start(ctx.factory), |_| {
            rtc_address(ctx.factory)
        }))
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}

#[cfg(feature = "rtc")]
fn rtc_address(factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    *moqui_lib::sync::lock(&factory.extension(|_| crate::rtc::RtcAddress::default()).0)
}

#[cfg(not(feature = "rtc"))]
fn rtc_address(_factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    None
}

pub(crate) struct Sip;

impl Listener for Sip {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<crate::sip::SipNode>;

    fn name(&self) -> &'static str {
        SIP
    }

    fn stage(&self) -> Stage {
        Stage::Comms
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::BeforeFactory
    }

    /// After XMPP, and finished before call media begins to stop.
    fn stop_rank(&self) -> u8 {
        1
    }

    fn selectors(&self) -> &'static [&'static str] {
        &["sip-server@enabled", "sip-server@port", "sip-server@tls-port"]
    }

    fn wanted(&self, _ctx: &Ctx) -> Wanted<(), ()> {
        on()
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Self::Run>, Error> {
        Ok(wrapped(ctx.factory, SIP, crate::sip::start(ctx.factory), |_| {
            sip_address(ctx.factory)
        }))
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}

#[cfg(feature = "sip")]
fn sip_address(factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    *moqui_lib::sync::lock(&factory.extension(|_| crate::sip::SipAddress::default()).0)
}

#[cfg(not(feature = "sip"))]
fn sip_address(_factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    None
}

pub(crate) struct Xmpp;

impl Listener for Xmpp {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<crate::xmpp::XmppNode>;

    fn name(&self) -> &'static str {
        XMPP
    }

    fn stage(&self) -> Stage {
        Stage::Comms
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::BeforeFactory
    }

    /// First before the factory, as today.
    fn stop_rank(&self) -> u8 {
        0
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[
            "xmpp-server@enabled",
            "xmpp-server@domain",
            "xmpp-server@port",
            "xmpp-server@tls-port",
        ]
    }

    fn wanted(&self, _ctx: &Ctx) -> Wanted<(), ()> {
        on()
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Self::Run>, Error> {
        Ok(wrapped(ctx.factory, XMPP, crate::xmpp::start(ctx.factory), |_| {
            xmpp_address(ctx.factory)
        }))
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}

/// The STARTTLS listener's address, else the direct TLS one's.
#[cfg(feature = "xmpp")]
fn xmpp_address(factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    let (starttls, tls) = *moqui_lib::sync::lock(&factory.extension(|_| crate::xmpp::XmppSlot::default()).addresses);
    starttls.or(tls)
}

#[cfg(not(feature = "xmpp"))]
fn xmpp_address(_factory: &ExecutionContextFactory) -> Option<SocketAddr> {
    None
}

pub(crate) struct Smtp;

impl Listener for Smtp {
    type Bind = ();
    type InPlace = ();
    type Run = Wrapped<SmtpServer>;

    fn name(&self) -> &'static str {
        SMTP
    }

    fn stage(&self) -> Stage {
        Stage::Comms
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::AfterFactory
    }

    /// First after the factory, before SSH and the cluster port, as today.
    fn stop_rank(&self) -> u8 {
        0
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[
            "smtp-server@enabled",
            "smtp-server@listen-port",
            "smtp-server@allowed-addresses",
        ]
    }

    fn wanted(&self, _ctx: &Ctx) -> Wanted<(), ()> {
        on()
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<Self::Run>, Error> {
        Ok(wrapped(ctx.factory, SMTP, start_smtp(ctx.factory), smtp_address))
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}

#[cfg(feature = "inbound-smtp")]
type SmtpServer = crate::smtp::SmtpServer;

/// No SMTP listener in this build: never made.
#[cfg(not(feature = "inbound-smtp"))]
type SmtpServer = std::convert::Infallible;

#[cfg(feature = "inbound-smtp")]
fn smtp_address(server: &SmtpServer) -> Option<SocketAddr> {
    Some(server.address())
}

#[cfg(not(feature = "inbound-smtp"))]
fn smtp_address(server: &SmtpServer) -> Option<SocketAddr> {
    match *server {}
}

/// The inbound SMTP listener while `smtp-server@enabled` is true. A listener that cannot start is logged and the server
/// starts without it; an empty or bad `smtp-server@allowed-addresses` never starts one.
#[cfg(feature = "inbound-smtp")]
fn start_smtp(factory: &Arc<ExecutionContextFactory>) -> Option<SmtpServer> {
    use moqui_core::startup::listener;
    if let Some(busy) = crate::port_check::busy(crate::smtp::LISTENER) {
        listener(factory, crate::smtp::LISTENER, State::Skipped(busy));
        return None;
    }
    // a front node's port 25 is a TCP port to its pool, never this listener (plans/Edge-Node2.md §4.7)
    let configured = moqui_core::context::conf_env::switch_on(&moqui_core::context::conf_env::setting_on(
        factory,
        "smtp-server@enabled",
    ));
    if moqui_core::edge::front_refuses(factory, "smtp-server@enabled", configured) {
        listener(factory, crate::smtp::LISTENER, State::off(moqui_core::edge::FRONT_ONLY));
        return None;
    }
    match crate::smtp::start(factory) {
        Ok(Some(server)) => {
            listener(
                factory,
                crate::smtp::LISTENER,
                State::On(format!("tcp {}", server.address())),
            );
            Some(server)
        }
        Ok(None) => {
            listener(
                factory,
                crate::smtp::LISTENER,
                State::off("smtp-server@enabled is false"),
            );
            None
        }
        Err(err) => {
            tracing::error!("The SMTP listener did not start, the server starts without it: {err}");
            listener(factory, crate::smtp::LISTENER, State::failed(err.message()));
            None
        }
    }
}

/// No SMTP listener in this build: recorded as not built, as the other comms' `_off` starts record theirs.
#[cfg(not(feature = "inbound-smtp"))]
fn start_smtp(factory: &Arc<ExecutionContextFactory>) -> Option<SmtpServer> {
    if moqui_core::context::conf_env::switch_on(&moqui_core::context::conf_env::setting_on(
        factory,
        "smtp-server@enabled",
    )) {
        tracing::error!("smtp-server@enabled is true but this build has no SMTP listener (the inbound-smtp feature)");
    }
    moqui_core::startup::listener(factory, SMTP, State::NotBuilt);
    None
}
