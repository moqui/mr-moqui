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

//! Phone calls over SIP (plans/Comms-Framework.md §5.1, §5.2; "The V1 gate" in §6.2).
//!
//! Every call is one `CallSession` whatever its transport; a phone number through a trunk is one more leg of it (§5.1).
//! This crate is that leg: its signaling toward the trunk provider and its media. It runs no business logic and knows
//! no ExecutionContext: moqui-host starts it under its `sip` feature, owns its sockets and asks the voice services
//! (`org.moqui.impl.VoiceServices`) what a call may do.
//!
//! - [`ua`]: the user agent and B2BUA on rsipstack (Tokio): the listeners, trunk registration, outbound calls with
//!   CANCEL and forked answers, inbound calls, hold by re-INVITE, and the per-source block of failed digests.
//! - [`digest`]: SIP Digest (RFC 3261, RFC 8760): answering a trunk's challenge only for its realm and algorithms, and
//!   challenging a request with nonces that expire and nonce counts that cannot be replayed.
//! - [`guard`]: failed digests counted and blocked per source address, never per account (§5.2 "Fraud").
//! - [`dial`]: numbers in E.164 and their class: emergency, international, premium or domestic.
//! - [`sdp`]: the offer and answer for PCMU, PCMA and Opus with telephone-event, and SDES keys.
//! - [`rtp`], [`srtp`] (AES_CM_128_HMAC_SHA1_80 on str0m's crypto provider) and [`dtmf`] (RFC 4733).
//! - [`media`]: a phone leg's audio, sans-IO: its RTP to 48 kHz frames for the call's mixer, and the mix-minus back
//!   to its codec every 20 ms (moqui-rtc's audio core, the M0 gate).
//! - [`race`]: the outcome of a leg's offers when more than one answers or the caller gives up: the first answer wins.
//! - [`registrar`] (V4): the bindings of desk and soft phones, sans-IO; the user agent's side of the registrar
//!   (digest against stored verifiers, the flows, forking a call to a device's bindings, a device's own calls) is
//!   `ua_phones.rs`.
//! - [`stun`]: the STUN Binding answer to a UDP phone's keepalive on the SIP socket (RFC 5626).

pub mod dial;
pub mod digest;
pub mod dtmf;
/// The per-source block of failed digests and the request caps, shared with the XMPP server (moqui-lib).
pub use moqui_lib::source_guard as guard;
pub mod media;
pub mod race;
pub mod registrar;
pub mod rtp;
pub mod sdp;
pub mod srtp;
pub mod stun;
pub mod ua;

/// The attribute [`ua::DeviceDirectory`] is implemented with, so the host needs no dependency of its own for it.
pub use async_trait::async_trait;

/// Makes aws-lc-rs the process's default rustls provider, as the rest of the host uses it. rsipstack's TLS builds its
/// configs from the process default, and its `ring` feature puts a second provider in rustls, so without this the
/// first TLS connection would panic. Safe to call more than once.
pub fn install_crypto() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
