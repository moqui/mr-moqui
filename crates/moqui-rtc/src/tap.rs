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

//! Media taken out of and put into a room (plans/Comms-Framework.md §5.1, §6.2): the seam the SIP mixer, recording and
//! captions use. A [`MediaTap`] set on a room sees every frame a participant publishes, after depacketizing and before
//! forwarding. An injected source ([`crate::Sfu::add_injected_source`]) is a participant with no browser: the frames
//! its owner writes ([`crate::Sfu::inject`]) are forwarded to every subscriber like a published track.
//!
//! A tap runs inside the media loop, so it must not block: it copies what it needs and hands it to its own bounded
//! queue (§6.4, "Queues for captions and recording").

use crate::sfu::Source;

/// The kind of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Audio,
    Video,
}

/// A frame as a tap sees it.
#[derive(Debug)]
pub struct TapFrame<'a> {
    /// The publishing leg.
    pub leg: &'a str,
    pub kind: Kind,
    pub source: Source,
    /// The simulcast layer, for video sent in layers.
    pub rid: Option<&'a str>,
    /// The codec name, for example `opus` or `vp8`.
    pub codec: &'a str,
    /// The RTP media time: `rtp_time / clock_rate` seconds.
    pub rtp_time: u64,
    pub clock_rate: u32,
    /// The encoded frame.
    pub data: &'a [u8],
    pub keyframe: bool,
    /// The audio level header extension, in negative dBov (0 is loudest, -127 silence).
    pub audio_level: Option<i8>,
}

/// Sees every frame published in a room.
pub trait MediaTap: Send {
    fn on_frame(&mut self, frame: &TapFrame<'_>);
}
