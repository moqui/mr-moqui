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

//! The M0 audio core (plans/Comms-Framework.md §6.2): codec round trips and levels, resampling, the jitter buffer's
//! reorder and loss, the mix-minus mixer, and the queue tap that drops rather than blocks.

use std::sync::Arc;

use super::codec::by_name;
use super::g711::{alaw_decode, alaw_encode, mulaw_decode, mulaw_encode};
use super::g722::{G722Decoder, G722Encoder};
use super::jitter::{JitterConfig, LegDecoder, Pulled};
use super::mixer::Mixer;
use super::opus::{OpusDecoder, OpusEncoder, VOICE_BITRATE};
use super::resample::Resampler;
use super::room::AudioRoom;
use super::{level_dbov, FRAME, SILENT_DBOV};
use crate::tap::{Kind, Kinds, MediaTap, OwnedFrame, QueueTap, TapEvent, TapFrame};
use crate::Source;

/// A tone at `hz`, `amplitude` peak, `n` samples at `rate`.
fn tone(hz: f64, amplitude: f64, rate: u32, n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| ((2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() * amplitude) as i16)
        .collect()
}

/// The correlation of `y` with `x` at the lag of `y` (up to `max_lag`) that matches best, and that lag.
fn best_correlation(x: &[i16], y: &[i16], max_lag: usize) -> (f64, usize) {
    let mut best = (-1.0, 0);
    for lag in 0..max_lag {
        let n = x.len().min(y.len().saturating_sub(lag));
        let (mut xy, mut xx, mut yy) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let (a, b) = (f64::from(x[i]), f64::from(y[i + lag]));
            xy += a * b;
            xx += a * a;
            yy += b * b;
        }
        let c = xy / (xx.sqrt() * yy.sqrt()).max(1e-9);
        if c > best.0 {
            best = (c, lag);
        }
    }
    best
}

/// The power of `pcm` at `hz` (Goertzel), relative to its total power.
fn share_at(pcm: &[i16], hz: f64, rate: u32) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / f64::from(rate);
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
fn g711_matches_the_reference_points_and_round_trips() {
    assert_eq!(mulaw_encode(0), 0xFF);
    assert_eq!(mulaw_decode(0xFF), 0);
    assert_eq!(mulaw_decode(0x80), 32_124);
    assert_eq!(mulaw_decode(0x00), -32_124);
    assert_eq!(alaw_encode(0), 0xD5);
    assert_eq!(alaw_decode(0xD5), 8);
    assert_eq!(alaw_decode(0x55), -8);
    assert_eq!(alaw_decode(0xAA), 32_256);
    // each code decodes to a value that encodes back to it
    for code in 0..=255u8 {
        let v = mulaw_decode(code);
        assert_eq!(mulaw_decode(mulaw_encode(v)), v, "mu-law {code:#x}");
        let v = alaw_decode(code);
        assert_eq!(alaw_encode(v), code, "A-law {code:#x}");
    }
    // a tone keeps its shape: about 38 dB of SNR for a logarithmic 8-bit code
    let pcm = tone(1_000.0, 16_000.0, 8_000, 8_000);
    for (name, enc, dec) in [
        ("pcmu", mulaw_encode as fn(i16) -> u8, mulaw_decode as fn(u8) -> i16),
        ("pcma", alaw_encode, alaw_decode),
    ] {
        let back: Vec<i16> = pcm.iter().map(|s| dec(enc(*s))).collect();
        let noise: f64 = pcm
            .iter()
            .zip(&back)
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
            .sum();
        let signal: f64 = pcm.iter().map(|a| f64::from(*a).powi(2)).sum();
        let snr = 10.0 * (signal / noise).log10();
        assert!(snr > 33.0, "{name} SNR {snr:.1} dB");
    }
}

/// A chirp with noise at 16 kHz, from the M0 spike: the G.722 vector below was checked bit for bit against libg722
/// (through ezk-g722, both started from the same state) over 5 s of it, encoding and decoding.
fn chirp_16k(n: usize) -> Vec<i16> {
    let mut seed = 1u64;
    (0..n)
        .map(|i| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let noise = ((seed >> 40) as i32 % 2_000) - 1_000;
            let t = i as f64 / 16_000.0;
            let f = 100.0 + 3_000.0 * t;
            let v = (2.0 * std::f64::consts::PI * f * t).sin() * 12_000.0 * (1.0 + (t * 3.0).sin()) / 2.0;
            (v as i32 + noise).clamp(-32_768, 32_767) as i16
        })
        .collect()
}

#[test]
fn g722_codes_the_reference_vector_and_round_trips_a_tone() {
    let mut coded = Vec::new();
    G722Encoder::new().encode(&chirp_16k(32), &mut coded);
    assert_eq!(
        coded,
        [122, 152, 40, 134, 34, 132, 45, 160, 38, 171, 40, 173, 170, 170, 173, 171]
    );

    let pcm = tone(1_000.0, 10_000.0, 16_000, 16_000);
    let mut coded = Vec::new();
    G722Encoder::new().encode(&pcm, &mut coded);
    assert_eq!(coded.len(), pcm.len() / 2, "one byte per two samples: 64 kbit/s");
    let mut back = Vec::new();
    G722Decoder::new().decode(&coded, &mut back);
    assert_eq!(back.len(), pcm.len());
    let (corr, lag) = best_correlation(&pcm, &back, 64);
    assert!(corr > 0.99, "G.722 tone correlation {corr:.4} at lag {lag}");
    let level = level_dbov(&back[1_000..]) - level_dbov(&pcm[1_000..]);
    assert!(level.abs() < 1.0, "G.722 level moved {level:.2} dB");
}

#[test]
fn opus_round_trips_a_tone_at_its_level() {
    let pcm = tone(440.0, 8_000.0, 48_000, 48_000);
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).expect("an encoder");
    let mut decoder = OpusDecoder::new().expect("a decoder");
    let mut back = Vec::new();
    let mut bytes = 0;
    for frame in pcm.as_chunks::<FRAME>().0 {
        let packet = encoder.encode(frame).expect("encoded").to_vec();
        bytes += packet.len();
        decoder.decode(&packet, &mut back).expect("decoded");
    }
    assert_eq!(back.len(), pcm.len());
    let (corr, lag) = best_correlation(&pcm, &back, 1_200);
    assert!(corr > 0.99, "opus tone correlation {corr:.4} at lag {lag}");
    let level = level_dbov(&back[9_600..]) - level_dbov(&pcm[9_600..]);
    assert!(level.abs() < 1.0, "opus level moved {level:.2} dB");
    // VBR keeps near its 32 kbit/s budget for a tone
    assert!(bytes * 8 < 64_000, "opus spent {} bits a second", bytes * 8);
}

#[test]
fn opus_conceals_and_recovers_a_lost_frame() {
    let pcm = tone(300.0, 8_000.0, 48_000, 48_000);
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).expect("an encoder");
    let packets: Vec<Vec<u8>> = pcm
        .as_chunks::<FRAME>()
        .0
        .iter()
        .map(|f| encoder.encode(f).expect("encoded").to_vec())
        .collect();
    let mut decoder = OpusDecoder::new().expect("a decoder");
    let mut out = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        if i == 20 {
            decoder.recover(&packets[21], FRAME, &mut out);
        } else if i == 30 {
            decoder.conceal(FRAME, &mut out);
        } else {
            decoder.decode(p, &mut out).expect("decoded");
        }
    }
    assert_eq!(out.len(), pcm.len(), "a lost frame still fills its 20 ms");
    for lost in [20, 30] {
        let frame = &out[lost * FRAME..(lost + 1) * FRAME];
        assert!(level_dbov(frame) > -30.0, "frame {lost} is not a dropout");
    }
}

#[test]
fn codecs_by_name_report_their_rates() {
    for (name, rate, clock, pt) in [
        ("opus", 48_000, 48_000, None),
        ("PCMU", 8_000, 8_000, Some(0)),
        ("pcma", 8_000, 8_000, Some(8)),
        ("G722", 16_000, 8_000, Some(9)),
    ] {
        let mut codec = by_name(name).expect("built in");
        assert_eq!(
            (codec.rate(), codec.clock_rate(), codec.payload_type()),
            (rate, clock, pt),
            "{name}"
        );
        let samples = rate as usize / 50;
        let pcm = tone(500.0, 6_000.0, rate, samples * 10);
        let mut back = Vec::new();
        for frame in pcm.chunks_exact(samples) {
            let mut payload = Vec::new();
            codec.encode(frame, &mut payload).expect("encoded");
            codec.decode(&payload, &mut back).expect("decoded");
        }
        assert_eq!(back.len(), pcm.len(), "{name}");
        let (corr, _) = best_correlation(&pcm[samples * 5..], &back[samples * 5..], samples);
        assert!(corr > 0.95, "{name}: correlation {corr:.3}");
        codec.conceal(samples, &mut back);
        assert_eq!(back.len(), pcm.len() + samples);
    }
    assert!(by_name("g729").is_none());
}

#[test]
fn resampling_keeps_the_voice_band_and_stops_aliases() {
    for low in [8_000u32, 16_000] {
        let ratio = (48_000 / low) as usize;
        let pcm = tone(1_000.0, 10_000.0, 48_000, 48_000);
        let mut down = Resampler::new(48_000, low);
        let mut up = Resampler::new(low, 48_000);
        let (mut narrow, mut back) = (Vec::new(), Vec::new());
        for frame in pcm.as_chunks::<FRAME>().0 {
            let before = narrow.len();
            down.process(frame, &mut narrow);
            assert_eq!(narrow.len() - before, FRAME / ratio, "20 ms in, 20 ms out");
        }
        for frame in narrow.chunks_exact(FRAME / ratio) {
            up.process(frame, &mut back);
        }
        assert_eq!(back.len(), pcm.len());
        let (corr, lag) = best_correlation(&pcm[4_800..], &back[4_800..], 600);
        assert!(corr > 0.999, "{low} Hz round trip: correlation {corr:.5} at lag {lag}");
        let level = level_dbov(&back[4_800..]) - level_dbov(&pcm[4_800..]);
        assert!(level.abs() < 0.2, "{low} Hz round trip moved the level {level:.2} dB");

        // a tone above the narrow band's Nyquist frequency is stopped, not folded back
        let above = tone(f64::from(low) * 0.75, 10_000.0, 48_000, 48_000);
        let mut down = Resampler::new(48_000, low);
        let mut folded = Vec::new();
        down.process(&above, &mut folded);
        let attenuation = level_dbov(&folded[1_000..]) - level_dbov(&above);
        assert!(attenuation < -50.0, "{low} Hz: an alias only {attenuation:.1} dB down");
    }
}

/// Opus packets of a tone, 20 ms each, at RTP times 0, 960, ...
fn tone_packets(hz: f64, frames: usize) -> Vec<(u64, Vec<u8>)> {
    let pcm = tone(hz, 8_000.0, 48_000, frames * FRAME);
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).expect("an encoder");
    pcm.as_chunks::<FRAME>()
        .0
        .iter()
        .enumerate()
        .map(|(i, f)| ((i * FRAME) as u64, encoder.encode(f).expect("encoded").to_vec()))
        .collect()
}

#[test]
fn the_jitter_buffer_puts_reordered_frames_back_in_order() {
    let packets = tone_packets(440.0, 50);
    let mut reference = OpusDecoder::new().expect("a decoder");
    let mut expected = Vec::new();
    for (_, p) in &packets {
        reference.decode(p, &mut expected).expect("decoded");
    }

    let mut leg = LegDecoder::new(JitterConfig::default()).expect("a decoder");
    let mut got = Vec::new();
    let mut frame = vec![0i16; FRAME];
    // pairs swapped: 1, 0, 3, 2, ...; one frame arrives each tick
    let mut order: Vec<usize> = (0..packets.len()).collect();
    for pair in order.chunks_mut(2) {
        pair.reverse();
    }
    let mut pulls = Vec::new();
    for i in order {
        leg.push(packets[i].0, &packets[i].1);
        let pulled = leg.pull(&mut frame);
        if pulled != Pulled::Idle {
            got.extend_from_slice(&frame);
        }
        pulls.push(pulled);
    }
    while leg.pull(&mut frame) == Pulled::Played {
        got.extend_from_slice(&frame);
    }
    assert_eq!(pulls[0], Pulled::Idle, "the first tick fills the buffer");
    assert!(pulls[1..].iter().all(|p| *p == Pulled::Played), "{pulls:?}");
    assert_eq!(got, expected, "the same audio as in order");
    assert_eq!(leg.stats().played, 50);
}

#[test]
fn the_jitter_buffer_recovers_conceals_and_drops_late_frames() {
    let packets = tone_packets(440.0, 60);
    let mut leg = LegDecoder::new(JitterConfig::default()).expect("a decoder");
    let mut frame = vec![0i16; FRAME];
    let mut pulls = Vec::new();
    for (i, (time, data)) in packets.iter().enumerate() {
        // 20 is lost but 21 is there (FEC); 30 and 31 are lost (the first concealed, the second recovered from 32); 40
        // comes ten frames late
        if matches!(i, 20 | 30 | 31 | 40) {
            continue;
        }
        leg.push(*time, data);
        if i == 50 {
            leg.push(packets[40].0, &packets[40].1);
        }
        pulls.push(leg.pull(&mut frame));
        if pulls.len() > 25 {
            // a recovered frame after a concealed one is quieter (the FEC copy is coded at a lower rate), but not
            // silent
            assert!(level_dbov(&frame) > -60.0, "tick {}: a dropout", pulls.len());
        }
    }
    let stats = leg.stats();
    assert_eq!(stats.recovered, 3, "20, 31 and 40 from the next frame's FEC: {stats:?}");
    assert_eq!(stats.concealed, 1, "30: {stats:?}");
    assert_eq!(stats.late, 1, "40 came after its time: {stats:?}");

    // the leg stops: a few concealed frames, then silence, and it starts again with its next frame
    let mut tail = Vec::new();
    for _ in 0..15 {
        tail.push(leg.pull(&mut frame));
    }
    assert_eq!(tail.last(), Some(&Pulled::Idle), "{tail:?}");
    assert!(!leg.is_playing());
    // the next talk spurt, 10 s on
    let later = 480_000;
    for (time, data) in tone_packets(440.0, 3) {
        leg.push(later + time, &data);
    }
    assert_eq!(leg.pull(&mut frame), Pulled::Played);

    // the sender restarts its stream at an RTP time far behind: not late frames, a new timeline
    let mut leg = LegDecoder::new(JitterConfig::default()).expect("a decoder");
    let far = 10_000_000;
    for (time, data) in tone_packets(440.0, 5) {
        leg.push(far + time, &data);
        leg.pull(&mut frame);
    }
    let late = leg.stats().late;
    let mut restarted = Vec::new();
    for (time, data) in tone_packets(440.0, 4) {
        leg.push(time, &data);
        restarted.push(leg.pull(&mut frame));
    }
    assert_eq!(leg.stats().late, late, "nothing dropped as late");
    assert_eq!(restarted.last(), Some(&Pulled::Played), "{restarted:?}");
}

#[test]
fn the_mixer_gives_each_leg_everyone_but_itself() {
    let (a, b, c): (Arc<str>, Arc<str>, Arc<str>) = ("A".into(), "B".into(), "C".into());
    let mut mixer = Mixer::new();
    mixer.clear();
    mixer.put(&a, &[100; FRAME]);
    mixer.put(&b, &[20; FRAME]);
    mixer.put(&c, &[3; FRAME]);
    assert!(mixer.minus("A").iter().all(|s| *s == 23));
    assert!(mixer.minus("B").iter().all(|s| *s == 103));
    assert!(mixer.minus("C").iter().all(|s| *s == 120));
    assert!(mixer.all().iter().all(|s| *s == 123));
    assert!(
        mixer.minus("PHONE").iter().all(|s| *s == 123),
        "a listener that put nothing hears all"
    );
    assert!(mixer.level("A") > mixer.level("B"));

    // a second frame from a leg replaces its first
    mixer.put(&a, &[200; FRAME]);
    assert!(mixer.minus("C").iter().all(|s| *s == 220));

    // the next tick: A is missing (silent), B joined late with a short frame
    mixer.clear();
    mixer.put(&b, &[20; 100]);
    mixer.put(&c, &[3; FRAME]);
    assert_eq!(mixer.level("A"), SILENT_DBOV);
    let out = mixer.minus("C");
    assert!(out[..100].iter().all(|s| *s == 20) && out[100..].iter().all(|s| *s == 0));

    // loud legs saturate rather than wrap
    mixer.clear();
    mixer.put(&a, &[30_000; FRAME]);
    mixer.put(&b, &[30_000; FRAME]);
    mixer.put(&c, &[-30_000; FRAME]);
    assert!(mixer.minus("C").iter().all(|s| *s == i16::MAX));
    assert!(mixer.minus("A").iter().all(|s| *s == 0));
    mixer.put(&b, &[-30_000; FRAME]);
    assert!(mixer.minus("A").iter().all(|s| *s == i16::MIN));
    assert_eq!(mixer.clipped(), 2 * FRAME as u64);
}

fn opus_frame(leg: &Arc<str>, time: u64, data: Vec<u8>) -> OwnedFrame {
    OwnedFrame {
        leg: leg.clone(),
        kind: Kind::Audio,
        source: Source::Microphone,
        rid: None,
        codec: "opus",
        rtp_time: time,
        clock_rate: 48_000,
        data,
        keyframe: false,
        audio_level: None,
    }
}

#[test]
fn a_room_decodes_and_mixes_three_legs() {
    let legs: [(Arc<str>, f64); 3] = [("A".into(), 440.0), ("B".into(), 1_000.0), ("C".into(), 2_500.0)];
    let streams: Vec<Vec<(u64, Vec<u8>)>> = legs.iter().map(|(_, hz)| tone_packets(*hz, 40)).collect();
    let mut room = AudioRoom::default();
    let mut heard_by_a = Vec::new();
    let mut all = Vec::new();
    for tick in 0..40 {
        for ((leg, _), stream) in legs.iter().zip(&streams) {
            let (time, data) = &stream[tick];
            room.push(&opus_frame(leg, *time, data.clone()));
        }
        let mixer = room.tick();
        if tick >= 10 {
            heard_by_a.extend_from_slice(mixer.minus("A"));
            all.extend_from_slice(mixer.all());
        }
    }
    assert_eq!(room.legs().count(), 3);
    assert!(room.leg_audio("B").is_some());
    // A hears B and C, not itself
    assert!(share_at(&heard_by_a, 1_000.0, 48_000) > 0.3);
    assert!(share_at(&heard_by_a, 2_500.0, 48_000) > 0.3);
    assert!(share_at(&heard_by_a, 440.0, 48_000) < 0.01);
    // the whole mix has all three
    for (_, hz) in &legs {
        assert!(share_at(&all, *hz, 48_000) > 0.2, "{hz} Hz in the mix");
    }
    // a leg that left goes now
    room.take(&TapEvent::Left(legs[2].0.clone()));
    assert_eq!(room.legs().count(), 2);
    // video and other codecs are not audio to mix
    let mut video = opus_frame(&legs[0].0, 0, vec![1, 2, 3]);
    video.kind = Kind::Video;
    room.push(&video);
    assert_eq!(room.ignored(), 1);
}

#[test]
fn a_full_queue_drops_and_counts_rather_than_blocks() {
    let (mut tap, rx) = QueueTap::channel(4, Kinds::Audio);
    let leg: Arc<str> = "A".into();
    let frame = |kind| TapFrame {
        leg: &leg,
        kind,
        source: Source::Microphone,
        rid: None,
        codec: "opus",
        rtp_time: 0,
        clock_rate: 48_000,
        data: b"frame",
        keyframe: false,
        audio_level: Some(-30),
    };
    for _ in 0..10 {
        tap.on_frame(&frame(Kind::Audio));
    }
    tap.on_frame(&frame(Kind::Video));
    assert_eq!((rx.stats().delivered(), rx.stats().dropped()), (4, 6));
    let Ok(TapEvent::Frame(first)) = rx.try_recv() else {
        panic!("a frame")
    };
    assert_eq!((&*first.leg, first.data.as_slice()), ("A", &b"frame"[..]));

    // a leave that finds the queue full is held, not dropped, and comes before the next frame
    tap.on_frame(&frame(Kind::Audio));
    tap.on_leave(&leg);
    for _ in 0..4 {
        assert!(matches!(rx.try_recv(), Ok(TapEvent::Frame(_))));
    }
    tap.on_frame(&frame(Kind::Audio));
    assert_eq!(rx.try_recv(), Ok(TapEvent::Left(leg.clone())));
    assert!(matches!(rx.try_recv(), Ok(TapEvent::Frame(_))));
    assert_eq!(rx.stats().dropped(), 6, "the frame behind the held leave went through");

    assert!(!tap.is_closed());
    drop(rx);
    assert!(tap.is_closed(), "the consumer went away, seen before any frame");
}

#[test]
fn malformed_opus_packets_are_dropped_not_a_panic() {
    let packets = tone_packets(440.0, 4);
    let mut leg = LegDecoder::new(JitterConfig::default()).expect("a decoder");
    let mut frame = vec![0i16; FRAME];
    let mut seed = 7u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as u32
    };
    // random packets, and real ones with a byte flipped and cut short, between good frames
    for i in 0..200u64 {
        let data: Vec<u8> = if i % 2 == 0 {
            (0..1 + next() % 120).map(|_| next() as u8).collect()
        } else {
            let mut p = packets[(i % 4) as usize].1.clone();
            let at = next() as usize % p.len();
            p[at] ^= next() as u8;
            p.truncate(1 + next() as usize % p.len());
            p
        };
        leg.push(i * FRAME as u64, &data);
        leg.pull(&mut frame);
    }
    let base = 200 * FRAME as u64;
    for (time, data) in &packets {
        leg.push(base + time, data);
    }
    let pulled = leg.pull(&mut frame);
    assert_ne!(pulled, Pulled::Idle, "{:?}", leg.stats());
}

#[test]
fn a_legs_microphone_and_screen_audio_are_two_streams_of_one_input() {
    let a: Arc<str> = "A".into();
    let b: Arc<str> = "B".into();
    let mic = tone_packets(440.0, 30);
    let screen = tone_packets(2_500.0, 30);
    let other = tone_packets(1_000.0, 30);
    let mut room = AudioRoom::default();
    let (mut heard_by_a, mut heard_by_b, mut a_mic) = (Vec::new(), Vec::new(), Vec::new());
    for tick in 0..30 {
        room.push(&opus_frame(&a, mic[tick].0, mic[tick].1.clone()));
        // the screen share's own RTP timeline, far from the microphone's
        let mut share = opus_frame(&a, 5_000_000 + screen[tick].0, screen[tick].1.clone());
        share.source = Source::ScreenAudio;
        room.push(&share);
        room.push(&opus_frame(&b, other[tick].0, other[tick].1.clone()));
        let mixer = room.tick();
        if tick >= 10 {
            heard_by_a.extend_from_slice(mixer.minus("A"));
            heard_by_b.extend_from_slice(mixer.minus("B"));
            a_mic.extend_from_slice(room.source_audio("A", Source::Microphone).expect("A's microphone"));
        }
    }
    assert_eq!(
        room.stats("A").expect("A").late,
        0,
        "neither stream is late to the other"
    );
    assert_eq!(room.legs().count(), 2);
    // B hears both of A's streams; A hears neither of its own
    assert!(share_at(&heard_by_b, 440.0, 48_000) > 0.2);
    assert!(share_at(&heard_by_b, 2_500.0, 48_000) > 0.2);
    assert!(share_at(&heard_by_a, 440.0, 48_000) < 0.01);
    assert!(share_at(&heard_by_a, 2_500.0, 48_000) < 0.01);
    assert!(share_at(&a_mic, 2_500.0, 48_000) < 0.01, "the microphone alone");
}
