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

//! Calls: the WebRTC endpoint and the selective forwarding unit (SFU) every call goes through
//! (plans/Comms-Framework.md §6.1, §6.2, D-W1, D-W2). It is written on str0m, a sans-IO WebRTC library: ICE-lite, DTLS-SRTP,
//! simulcast and TWCC bandwidth estimation, with every frame exposed to the caller.
//!
//! [`Sfu`] holds the rooms of one media node. Its caller owns the sockets and the clock: it hands in datagrams
//! ([`Sfu::handle_receive`]) and the time ([`Sfu::handle_timeout`]), and takes out datagrams to send, signaling events and
//! the next timeout ([`Sfu::poll_output`]). The caller also passes in the signaling commands: a leg joining with its
//! offer ([`Sfu::join`]), answers and offers in either direction, candidates, subscriptions and leaving. Nothing here
//! decides who may join: moqui-host checks the call, leg and connection generation with `org.moqui.impl.CallServices`
//! before a command reaches the SFU, and the generation passed with each command fences an older connection of the same
//! leg.
//!
//! The forwarding is the SFU's own, not str0m's, so [`tap`] can take each frame out (for the SIP mixer, recording and
//! captions) and put frames in (a SIP leg's audio, as one more participant). [`audio`] decodes, mixes and encodes that
//! audio off the media thread: the codecs, resampling, the per-leg jitter buffer and the mix-minus mixer.

pub mod audio;
pub mod sfu;
pub mod tap;
pub mod turn;

pub use sfu::{Layer, LegKey, Proto, Sfu, SfuConfig, SfuError, SfuEvent, SfuOutput, Source, TapId, Transmit};
