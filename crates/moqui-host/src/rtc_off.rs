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

//! Call media when this build has no `rtc` feature: nothing starts, a set `rtc_media_port` is an error in the log, and
//! the media hook points answer `notBuilt`.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) struct RtcNode;

pub(crate) fn start(_factory: &Arc<ExecutionContextFactory>) -> Option<RtcNode> {
    if !moqui_lib::env::get_prop_or_env("rtc_media_port").trim().is_empty() {
        tracing::error!("rtc_media_port is set but this build has no call media (the rtc feature)");
    }
    None
}

/// The edge wizard's TURN check (`rtc/turn_probe.rs`) without calls: there is no TURN client.
#[cfg(feature = "edge")]
pub(crate) mod turn_probe {
    pub(crate) fn probe(_url: &str, _username: &str, _credential: &str, _peer: &str) -> Result<String, String> {
        Err("This build has no calls (the rtc feature), so the edge node's TURN is not checked".to_string())
    }
}

/// The media hook points without calls (`rtc/media.rs` has the API and its docs): nothing to attach to or inject into.
#[allow(
    dead_code,
    reason = "hook points: the recorder (W5), captions (W6) and SIP legs (V1) call them"
)]
pub(crate) mod media {
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
    use std::sync::Arc;
    use std::time::Duration;

    /// Which frames a consumer takes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Kinds {
        Audio,
        All,
    }

    /// The kind of a track.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum Kind {
        Audio,
        Video,
    }

    /// Where a track comes from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum Source {
        Microphone,
        Camera,
        Screen,
        ScreenAudio,
        Phone,
    }

    /// A frame taken out of a room; never made here, its fields as with calls.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct OwnedFrame {
        pub(crate) leg: Arc<str>,
        pub(crate) kind: Kind,
        pub(crate) source: Source,
        pub(crate) rid: Option<String>,
        pub(crate) codec: &'static str,
        pub(crate) rtp_time: u64,
        pub(crate) clock_rate: u32,
        pub(crate) data: Vec<u8>,
        pub(crate) keyframe: bool,
        pub(crate) audio_level: Option<i8>,
    }

    /// What a consumer receives.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum TapEvent {
        Frame(OwnedFrame),
        Left(Arc<str>),
    }

    /// No consumer can be attached.
    pub(crate) enum Consumer {}

    /// No leg can be injected.
    pub(crate) enum PcmSource {}

    pub(crate) fn attach(_room: &str, _kinds: Kinds, _capacity: usize) -> Result<Consumer, &'static str> {
        Err("notBuilt")
    }

    pub(crate) fn add_pcm_source(_room: &str, _leg: &str) -> Result<PcmSource, &'static str> {
        Err("notBuilt")
    }

    impl Consumer {
        pub(crate) fn recv_timeout(&self, _wait: Duration) -> Result<TapEvent, RecvTimeoutError> {
            match *self {}
        }

        pub(crate) fn try_recv(&self) -> Result<TapEvent, TryRecvError> {
            match *self {}
        }

        pub(crate) fn dropped(&self) -> u64 {
            match *self {}
        }

        pub(crate) fn delivered(&self) -> u64 {
            match *self {}
        }

        pub(crate) fn room(&self) -> &str {
            match *self {}
        }
    }

    impl PcmSource {
        pub(crate) fn push(&mut self, _pcm: &[i16]) -> Result<(), &'static str> {
            match *self {}
        }

        pub(crate) fn dropped(&self) -> u64 {
            match *self {}
        }

        pub(crate) fn leg(&self) -> &str {
            match *self {}
        }
    }
}
