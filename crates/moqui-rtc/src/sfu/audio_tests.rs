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

//! The M0 audio gate (plans/archive/Comms-Framework.md §6.2) on the in-memory network: consumers that fall behind drop frames
//! while forwarding goes on, and the whole path, tap to decode to mix to encode to an injected leg, heard by a peer.

use std::time::Duration;

use str0m::media::MediaKind;

use super::testnet::{Net, ROOM};
use crate::audio::opus::{OpusDecoder, OpusEncoder, VOICE_BITRATE};
use crate::audio::room::AudioRoom;
use crate::audio::FRAME;
use crate::tap::{Kinds, QueueTap, TapEvent};

/// The power of `pcm` at `hz` (Goertzel), as a share of its total power.
fn share_at(pcm: &[i16], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / 48_000.0;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for x in pcm {
        let s = f64::from(*x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    let total: f64 = pcm.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
    2.0 * power / (pcm.len() as f64 * total.max(1e-9))
}

#[test]
fn a_consumer_that_falls_behind_drops_frames_and_forwarding_goes_on() {
    let mut net = Net::new();
    net.join("A", false);
    net.join("B", false);
    net.run(Duration::from_secs(1));
    // a stalled consumer (never read) and a live one, on the same room
    let (stalled, stalled_rx) = QueueTap::channel(8, Kinds::All);
    let (live, live_rx) = QueueTap::channel(1_000, Kinds::Audio);
    net.sfu.add_tap(ROOM, Box::new(stalled)).expect("tapped");
    let live_id = net.sfu.add_tap(ROOM, Box::new(live)).expect("tapped");
    let before = net.received("B", "A", MediaKind::Audio);
    net.run(Duration::from_secs(2));
    let forwarded = net.received("B", "A", MediaKind::Audio) - before;
    assert!(forwarded >= 95, "B heard {forwarded} of A's 100 frames");
    assert_eq!(stalled_rx.stats().delivered(), 8);
    assert!(
        stalled_rx.stats().dropped() > 300,
        "audio and video dropped: {}",
        stalled_rx.stats().dropped()
    );
    let mut live_frames = 0;
    while live_rx.try_recv().is_ok() {
        live_frames += 1;
    }
    assert!(
        live_frames >= 190,
        "the live consumer got both legs' audio: {live_frames}"
    );
    assert_eq!(live_rx.stats().dropped(), 0);

    // a consumer that goes away is dropped at the room's next frame
    drop(live_rx);
    net.run(Duration::from_millis(100));
    assert!(!net.sfu.remove_tap(ROOM, live_id), "already gone");
    // the room stays while a tap is on it, and goes with close_room
    let now = net.now;
    net.sfu.close_room(now, ROOM);
    assert!(matches!(
        stalled_rx.recv_timeout(Duration::from_millis(10)),
        Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    while stalled_rx.try_recv().is_ok() {}
    assert!(stalled_rx
        .try_recv()
        .is_err_and(|e| e == std::sync::mpsc::TryRecvError::Disconnected));
}

#[test]
fn a_mix_minus_goes_out_through_an_injected_leg_and_is_heard() {
    let mut net = Net::new();
    for (leg, hz) in [("A", 440.0), ("B", 1_000.0), ("C", 0.0)] {
        net.join(leg, false);
        let c = net.client_mut(leg);
        c.tone = Some((hz, OpusEncoder::new(VOICE_BITRATE).expect("an encoder")));
        c.keep_audio = true;
    }
    net.run(Duration::from_secs(1));

    // the phone leg PH: it hears everyone but itself, and that is what it sends back (an echoing phone)
    let (tap, frames) = QueueTap::channel(256, Kinds::Audio);
    net.sfu.add_tap(ROOM, Box::new(tap)).expect("tapped");
    let now = net.now;
    net.sfu.add_injected_source(now, ROOM, "PH").expect("a phone leg");
    net.run(Duration::from_millis(300));
    let mut room = AudioRoom::default();
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).expect("an encoder");
    let mut rtp_time = 0;
    for _ in 0..75 {
        net.run(Duration::from_millis(20));
        while let Ok(event) = frames.try_recv() {
            room.take(&event);
        }
        let mix = room.tick().minus("PH").to_vec();
        let packet = encoder.encode(&mix).expect("encoded").to_vec();
        let now = net.now;
        net.sfu.inject(now, ROOM, "PH", rtp_time, &packet).expect("injected");
        rtp_time += FRAME as u64;
    }
    assert_eq!(frames.stats().dropped(), 0);
    let mut legs: Vec<&str> = room.legs().map(|l| &**l).collect();
    legs.sort_unstable();
    assert_eq!(legs, ["A", "B", "C", "PH"], "the tap sees the injected leg too");

    // C decodes what PH sent: A's tone and B's
    let received = net.client("C").audio_frames.get("PH").cloned().unwrap_or_default();
    assert!(received.len() >= 60, "C got {} of PH's 75 frames", received.len());
    let mut decoder = OpusDecoder::new().expect("a decoder");
    let mut pcm = Vec::new();
    for (_, data) in &received {
        decoder.decode(data, &mut pcm).expect("decoded");
    }
    let heard = &pcm[pcm.len() / 2..];
    assert!(share_at(heard, 440.0) > 0.2, "A's tone: {:.3}", share_at(heard, 440.0));
    assert!(
        share_at(heard, 1_000.0) > 0.2,
        "B's tone: {:.3}",
        share_at(heard, 1_000.0)
    );
    assert!(crate::audio::level_dbov(heard) > -30.0);

    // the phone leg hangs up: the tap hears it left, after its frames
    let now = net.now;
    net.sfu.remove_injected_source(now, ROOM, "PH");
    let events: Vec<TapEvent> = std::iter::from_fn(|| frames.try_recv().ok()).collect();
    let left = events
        .iter()
        .position(|e| *e == TapEvent::Left("PH".into()))
        .expect("PH left");
    assert!(
        !events[left..]
            .iter()
            .any(|e| matches!(e, TapEvent::Frame(f) if &*f.leg == "PH")),
        "no frame of PH after it left"
    );
}
