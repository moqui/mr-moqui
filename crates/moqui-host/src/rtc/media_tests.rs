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

use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;
use moqui_rtc::audio::opus::OpusDecoder;
use moqui_rtc::audio::{level_dbov, FRAME};
use moqui_rtc::{Sfu, SfuConfig, Source};

use super::{add_pcm_source, attach, Kinds, Media, MediaCommand, Pacer, SourceFrame, SourceState, TapEvent, TICK};
use crate::rtc::Command;

fn tone_frame(start: usize) -> Vec<i16> {
    (start..start + FRAME)
        .map(|i| ((2.0 * std::f64::consts::PI * 600.0 * i as f64 / 48_000.0).sin() * 8_000.0) as i16)
        .collect()
}

#[test]
fn the_pacer_waits_for_two_frames_then_sends_one_each_tick() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    let mut due = Vec::new();
    pacer.push(b"f0".to_vec());
    pacer.due(start, &mut due);
    assert!(
        due.is_empty() && pacer.next_due().is_none(),
        "one frame is not enough to start"
    );
    pacer.push(b"f1".to_vec());
    pacer.due(start, &mut due);
    assert_eq!(due, [(0, b"f0".to_vec())]);
    assert_eq!(pacer.next_due(), Some(start + TICK));
    due.clear();
    pacer.due(start + Duration::from_millis(10), &mut due);
    assert!(due.is_empty(), "not yet due");
    pacer.due(start + TICK, &mut due);
    assert_eq!(due, [(960, b"f1".to_vec())]);
    // two ticks with nothing pushed are gaps; the RTP clock goes on
    due.clear();
    pacer.due(start + TICK * 3, &mut due);
    assert!(due.is_empty());
    pacer.push(b"f4".to_vec());
    pacer.due(start + TICK * 4, &mut due);
    assert_eq!(due, [(4 * 960, b"f4".to_vec())]);
    // a stalled driver skips ahead instead of sending a burst
    due.clear();
    pacer.push(b"late".to_vec());
    pacer.due(start + TICK * 4 + Duration::from_secs(1), &mut due);
    assert_eq!(due.len(), 1);
    assert!(due[0].0 >= 50 * 960, "the RTP time followed the clock: {}", due[0].0);

    // frames queued up during a stall do not add to the delay: the newest two are kept
    let stalled_at = pacer.next_due().expect("started");
    for n in 0..8u8 {
        pacer.push(vec![n]);
    }
    due.clear();
    pacer.due(stalled_at + Duration::from_millis(500), &mut due);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0].1, vec![6]);
    due.clear();
    pacer.due(pacer.next_due().expect("next"), &mut due);
    assert_eq!(due[0].1, vec![7]);
}

#[test]
fn an_injected_leg_reaches_a_consumer_on_the_driver_side() {
    let mut sfu = Sfu::new(SfuConfig::default());
    let mut media = Media::default();
    let start = Instant::now();
    let (tap, frames) = moqui_rtc::tap::QueueTap::channel(64, Kinds::Audio);
    let (reply, answer) = sync_channel(1);
    media.command(
        &mut sfu,
        start,
        super::MediaCommand::Attach {
            room: "CS1".to_string(),
            id: 7,
            tap: Box::new(tap),
            reply,
        },
    );
    assert_eq!(answer.recv().expect("a reply"), Ok(()));
    let (tx, state) = add_source(&mut sfu, &mut media, start, "CS1", "PH");
    assert_eq!(sfu.legs("CS1").len(), 1, "a room of a phone only");
    for n in 0..5u8 {
        tx.send(frame(0, vec![0xF8, n])).expect("queued");
    }
    for tick in 0..5u32 {
        media.turn(&mut sfu, start + TICK * tick);
    }
    let seen: Vec<(u64, Vec<u8>)> = std::iter::from_fn(|| frames.try_recv().ok())
        .map(|event| {
            let TapEvent::Frame(f) = event else {
                panic!("a frame: {event:?}")
            };
            assert_eq!((&*f.leg, f.source), ("PH", Source::Phone));
            (f.rtp_time, f.data)
        })
        .collect();
    assert_eq!(seen.len(), 5);
    assert_eq!(seen[4], (4 * 960, vec![0xF8, 4]));
    assert_eq!(state.played.load(Ordering::Acquire), 5);

    // the owner went away: the leg goes, and the tap keeps the room
    drop(tx);
    media.turn(&mut sfu, start + TICK * 6);
    assert!(sfu.legs("CS1").is_empty());
    assert_eq!(sfu.room_count(), 1);
    media.command(&mut sfu, start, super::MediaCommand::Detach { id: 7 });
    assert_eq!(sfu.room_count(), 0, "the last tap gone, the room goes");
}

fn frame(epoch: u64, data: Vec<u8>) -> SourceFrame {
    SourceFrame { epoch, data }
}

/// An injected leg added on the driver side, with a tap on the room so it stays.
fn add_source(
    sfu: &mut Sfu,
    media: &mut Media,
    now: Instant,
    room: &str,
    leg: &str,
) -> (SyncSender<SourceFrame>, Arc<SourceState>) {
    let (tx, rx) = sync_channel(16);
    let state = Arc::new(SourceState::default());
    let (reply, answer) = sync_channel(1);
    media.command(
        sfu,
        now,
        MediaCommand::AddSource {
            room: room.to_string(),
            leg: leg.to_string(),
            frames: rx,
            state: Arc::clone(&state),
            reply,
        },
    );
    assert_eq!(answer.recv().expect("a reply"), Ok(()));
    (tx, state)
}

#[test]
fn a_cleared_pacer_sends_gaps_and_keeps_its_clock() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    let mut due = Vec::new();
    for n in 0..6u8 {
        pacer.push(vec![n]);
    }
    pacer.due(start, &mut due);
    assert_eq!(due, [(0, vec![0])]);
    pacer.clear();
    due.clear();
    pacer.due(start + TICK * 2, &mut due);
    assert!(due.is_empty(), "nothing left to send: {due:?}");
    assert_eq!(pacer.next_due(), Some(start + TICK * 3), "the timer goes on");
    pacer.push(vec![9]);
    pacer.due(start + TICK * 3, &mut due);
    assert_eq!(due, [(3 * 960, vec![9])], "the RTP clock went on through the gap");
}

#[test]
fn a_cleared_source_drops_its_queued_frames_and_counts_what_played() {
    let mut sfu = Sfu::new(SfuConfig::default());
    let mut media = Media::default();
    let start = Instant::now();
    let (tap, frames) = moqui_rtc::tap::QueueTap::channel(64, Kinds::Audio);
    let (reply, answer) = sync_channel(1);
    media.command(
        &mut sfu,
        start,
        MediaCommand::Attach {
            room: "CS2".to_string(),
            id: 8,
            tap: Box::new(tap),
            reply,
        },
    );
    assert_eq!(answer.recv().expect("a reply"), Ok(()));
    let (tx, state) = add_source(&mut sfu, &mut media, start, "CS2", "VA");
    let sent = || -> Vec<Vec<u8>> {
        std::iter::from_fn(|| frames.try_recv().ok())
            .map(|event| {
                let TapEvent::Frame(f) = event else {
                    panic!("a frame: {event:?}")
                };
                f.data
            })
            .collect()
    };

    // an utterance of 10 frames: 6 reach the pacer, 4 wait in the queue; 3 go out
    for n in 0..10u8 {
        tx.send(frame(0, vec![0xF8, n])).expect("queued");
    }
    for tick in 0..3u32 {
        media.turn(&mut sfu, start + TICK * tick);
    }
    assert_eq!(sent().len(), 3);
    assert_eq!(state.played.load(Ordering::Acquire), 3);

    // the listener talks over it: what is queued anywhere is dropped at once
    state.epoch.fetch_add(1, Ordering::AcqRel);
    media.command(
        &mut sfu,
        start + TICK * 3,
        MediaCommand::ClearSource {
            room: "CS2".to_string(),
            leg: "VA".to_string(),
        },
    );
    for tick in 3..8u32 {
        media.turn(&mut sfu, start + TICK * tick);
    }
    assert!(sent().is_empty(), "nothing of the old utterance plays");
    assert_eq!(state.played.load(Ordering::Acquire), 3, "the listener heard 60 ms");

    // the next utterance plays as usual, and a frame of before the clear that comes late is dropped
    tx.send(frame(0, vec![0xF8, 0xAA])).expect("queued");
    tx.send(frame(1, vec![0xF8, 0x10])).expect("queued");
    tx.send(frame(1, vec![0xF8, 0x11])).expect("queued");
    for tick in 8..11u32 {
        media.turn(&mut sfu, start + TICK * tick);
    }
    assert_eq!(sent(), [vec![0xF8, 0x10], vec![0xF8, 0x11]]);
    assert_eq!(state.played.load(Ordering::Acquire), 5);

    // a clear the media thread was not told of: the next turn drops the frames all the same
    for n in 0..4u8 {
        tx.send(frame(1, vec![0xF8, 0x20 + n])).expect("queued");
    }
    media.turn(&mut sfu, start + TICK * 11);
    assert_eq!(sent().len(), 1);
    state.epoch.fetch_add(1, Ordering::AcqRel);
    for tick in 12..16u32 {
        media.turn(&mut sfu, start + TICK * tick);
    }
    assert!(sent().is_empty());

    // frames pushed before a clear the media thread has not seen yet are still queued: they go, the new one plays
    tx.send(frame(2, vec![0xF8, 0x30])).expect("queued");
    state.epoch.fetch_add(1, Ordering::AcqRel);
    tx.send(frame(3, vec![0xF8, 0x31])).expect("queued");
    tx.send(frame(3, vec![0xF8, 0x32])).expect("queued");
    media.turn(&mut sfu, start + TICK * 16);
    assert_eq!(sent(), [vec![0xF8, 0x31]]);
    assert_eq!(state.played.load(Ordering::Acquire), 7);

    // a clear and a push between the media thread's epoch read and its take: the newer frame clears the pacer
    tx.send(frame(4, vec![0xF8, 0x40])).expect("queued");
    media.turn(&mut sfu, start + TICK * 17);
    assert_eq!(sent(), [vec![0xF8, 0x40]], "0x32, of before the clear, did not play");
    assert_eq!(state.played.load(Ordering::Acquire), 8);
}

/// A stand-in for the driver thread: the same `Media` on a bare SFU, turning every 2 ms.
fn fake_driver(commands: Receiver<Command>) {
    let mut sfu = Sfu::new(SfuConfig::default());
    let mut media = Media::default();
    loop {
        match commands.recv_timeout(Duration::from_millis(2)) {
            Ok(Command::Media(command)) => media.command(&mut sfu, Instant::now(), command),
            Ok(Command::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        media.turn(&mut sfu, Instant::now());
    }
}

#[test]
fn a_consumer_hears_a_pcm_source_through_the_hooks() {
    let (tx, rx) = sync_channel(64);
    *lock(super::super::media()) = Some(tx);
    let driver = std::thread::spawn(move || fake_driver(rx));

    let consumer = attach("CS9", Kinds::Audio, 64).expect("attached");
    let mut source = add_pcm_source("CS9", "PH").expect("a phone leg");
    assert_eq!(source.push(&[0; 10]), Err("badFrame"));
    for n in 0..10 {
        source.push(&tone_frame(n * FRAME)).expect("pushed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut decoder = OpusDecoder::new().expect("a decoder");
    let mut pcm = Vec::new();
    let mut times = Vec::new();
    while let Ok(event) = consumer.recv_timeout(Duration::from_millis(200)) {
        let TapEvent::Frame(frame) = event else {
            panic!("a frame: {event:?}")
        };
        assert_eq!(&*frame.leg, "PH");
        times.push(frame.rtp_time);
        decoder.decode(&frame.data, &mut pcm).expect("decoded");
    }
    assert_eq!(times.len(), 10, "every frame pushed, at {times:?}");
    assert!(
        times.windows(2).all(|w| w[1] > w[0] && (w[1] - w[0]) % 960 == 0),
        "{times:?}"
    );
    assert!(level_dbov(&pcm[FRAME * 5..]) > -25.0);
    assert_eq!(consumer.dropped(), 0);
    assert_eq!(source.played_frames(), 10, "every frame pushed was sent");
    assert_eq!(source.clear(), Ok(()));
    drop(source);
    drop(consumer);

    // with no media node, the hooks say so
    let _ = lock(super::super::media())
        .take()
        .map(|tx| tx.send(Command::Stop).is_ok());
    driver.join().expect("the driver stops");
    assert_eq!(attach("CS9", Kinds::Audio, 8).err(), Some("noMedia"));
    assert_eq!(add_pcm_source("CS9", "PH").err(), Some("noMedia"));
}
