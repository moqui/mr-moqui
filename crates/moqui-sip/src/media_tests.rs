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

use std::sync::Arc;

use moqui_rtc::audio::level_dbov;
use moqui_rtc::audio::mixer::Mixer;

use super::*;
use crate::sdp::{self, LocalMedia};
use crate::srtp::MasterKey;

/// Two ends of a phone leg, settled by an offer and answer: this side (`ours`) and the phone (`theirs`).
fn pair(codecs: &[&'static str], srtp: bool) -> (PhoneMedia, PhoneMedia, SocketAddr, SocketAddr) {
    let ours_at: SocketAddr = "127.0.0.1:20000".parse().unwrap();
    let theirs_at: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    let mut ours = LocalMedia::new(ours_at.ip(), ours_at.port(), srtp.then(MasterKey::random));
    ours.codecs = codecs.to_vec();
    let theirs = LocalMedia::new(theirs_at.ip(), theirs_at.port(), srtp.then(MasterKey::random));
    let offer = sdp::parse(&sdp::offer(&ours)).unwrap();
    let (answer, their_side) = sdp::answer(&offer, &theirs).unwrap();
    let our_side = sdp::accept(&ours, &sdp::parse(&answer).unwrap()).unwrap();
    (
        PhoneMedia::new(&our_side).unwrap(),
        PhoneMedia::new(&their_side).unwrap(),
        ours_at,
        theirs_at,
    )
}

fn tone(hz: f64, tick: usize) -> Vec<i16> {
    (0..FRAME)
        .map(|i| {
            let t = (tick * FRAME + i) as f64 / f64::from(RATE);
            (8000.0 * (2.0 * std::f64::consts::PI * hz * t).sin()) as i16
        })
        .collect()
}

#[test]
fn a_tone_crosses_each_codec_with_srtp() {
    for codec in ["pcmu", "pcma", "opus"] {
        let (mut ours, mut theirs, ours_at, _) = pair(&[codec], true);
        assert_eq!(ours.codec().name, codec);
        let mut heard = Vec::new();
        let mut out = Vec::new();
        for tick in 0..30 {
            let packet = ours.send(&tone(440.0, tick)).unwrap();
            assert_eq!(theirs.receive(ours_at, &packet), Received::Audio);
            if theirs.pull(&mut out) {
                assert_eq!(out.len(), FRAME);
                heard.push(level_dbov(&out));
            }
        }
        assert!(heard.len() >= 25, "{codec}: played {}", heard.len());
        let late: Vec<f32> = heard[10..].to_vec();
        assert!(late.iter().all(|l| *l > -20.0), "{codec}: the tone is heard: {late:?}");
        assert_eq!(theirs.stats().rejected, 0);
    }
}

#[test]
fn digits_cross_as_events_and_a_held_leg_sends_nothing() {
    let (mut ours, mut theirs, ours_at, theirs_at) = pair(&["pcmu"], false);
    assert_eq!(ours.send_digits("12#"), 3);
    let mut digits = String::new();
    for tick in 0..60 {
        if let Some(packet) = ours.send(&tone(300.0, tick)) {
            if let Received::Digit(d) = theirs.receive(ours_at, &packet) {
                digits.push(d);
            }
        }
    }
    assert_eq!(digits, "12#");
    // the phone holds: this side answers recvonly and stops sending, but still hears
    let mut held = Negotiated {
        codec: ours.codec(),
        dtmf: Some((101, 8000)),
        remote: theirs_at,
        local_key: None,
        remote_key: None,
        direction: Direction::RecvOnly,
    };
    ours.update(&held);
    assert!(ours.send(&tone(300.0, 0)).is_none());
    let packet = theirs.send(&tone(300.0, 0)).unwrap();
    assert_eq!(ours.receive(theirs_at, &packet), Received::Audio);
    held.direction = Direction::SendRecv;
    ours.update(&held);
    assert!(ours.send(&tone(300.0, 1)).is_some());
}

#[test]
fn reordered_and_lost_packets_are_put_back_and_concealed() {
    let (mut ours, mut theirs, ours_at, _) = pair(&["pcma"], false);
    let packets: Vec<Vec<u8>> = (0..12).map(|t| ours.send(&tone(500.0, t)).unwrap()).collect();
    // 1 comes after 2, 4 is lost; the leg plays a frame after each packet once it started
    let order = [0usize, 2, 1, 3, 5, 6, 7, 8, 9, 10, 11];
    let mut out = Vec::new();
    for (n, i) in order.into_iter().enumerate() {
        theirs.receive(ours_at, &packets[i]);
        if n == 3 {
            // a duplicate is ignored
            assert_eq!(theirs.receive(ours_at, &packets[3]), Received::Ignored);
        }
        if n >= 3 {
            theirs.pull(&mut out);
        }
    }
    // a packet after its time is late
    assert_eq!(theirs.receive(ours_at, &packets[4]), Received::Ignored);
    let mut played = 0;
    while theirs.pull(&mut out) {
        played += 1;
        if played > 20 {
            break;
        }
    }
    let stats = theirs.stats();
    assert_eq!(stats.played, 11);
    assert!(stats.concealed >= 1, "{stats:?}");
    // symmetric RTP: plain RTP latches to the first source at the SDP's address (another port: a NAT)
    let elsewhere: SocketAddr = "127.0.0.1:41000".parse().unwrap();
    let mut fresh = pair(&["pcmu"], false);
    let p = fresh.0.send(&tone(500.0, 0)).unwrap();
    // a packet from another address is not taken and does not latch (a guessed port cannot hijack the call)
    let stranger: SocketAddr = "192.0.2.66:41000".parse().unwrap();
    assert_eq!(fresh.1.receive(stranger, &p), Received::Rejected);
    assert_ne!(fresh.1.remote(), stranger);
    assert_eq!(fresh.1.receive(elsewhere, &p), Received::Audio);
    assert_eq!(fresh.1.remote(), elsewhere);
    // once latched, another port at the same address is not taken either
    let p2 = fresh.0.send(&tone(500.0, 1)).unwrap();
    let same_host: SocketAddr = "127.0.0.1:42000".parse().unwrap();
    assert_eq!(fresh.1.receive(same_host, &p2), Received::Rejected);
    assert_eq!(fresh.1.remote(), elsewhere);
    // garbage and a forged SRTP packet are rejected
    let (_, mut secure, at, _) = pair(&["pcmu"], true);
    assert_eq!(secure.receive(at, b"not rtp at all, not at all"), Received::Rejected);
    let mut forged = Vec::new();
    rtp::write(
        &RtpHeader {
            marker: false,
            payload_type: 0,
            sequence: 1,
            timestamp: 0,
            ssrc: 1,
        },
        &[0u8; 170],
        &mut forged,
    );
    assert_eq!(secure.receive(at, &forged), Received::Rejected);
}

#[test]
fn two_phone_legs_hear_each_other_through_the_mix_minus() {
    // phone A talks, phone B is silent; each leg is sent the mix without itself
    let (mut leg_a, mut phone_a, leg_a_at, phone_a_at) = pair(&["pcmu"], true);
    let (mut leg_b, mut phone_b, leg_b_at, phone_b_at) = pair(&["opus"], true);
    let a: Arc<str> = Arc::from("legA");
    let b: Arc<str> = Arc::from("legB");
    let mut mixer = Mixer::new();
    let mut pcm = Vec::new();
    let mut heard_by_a = Vec::new();
    let mut heard_by_b = Vec::new();
    let silence = vec![0i16; FRAME];
    for tick in 0..40 {
        // the phones send
        let from_a = phone_a.send(&tone(440.0, tick)).unwrap();
        leg_a.receive(phone_a_at, &from_a);
        let from_b = phone_b.send(&silence).unwrap();
        leg_b.receive(phone_b_at, &from_b);
        // the mixer's tick
        mixer.clear();
        if leg_a.pull(&mut pcm) {
            mixer.put(&a, &pcm);
        }
        if leg_b.pull(&mut pcm) {
            mixer.put(&b, &pcm);
        }
        let to_a = mixer.minus("legA").to_vec();
        let to_b = mixer.minus("legB").to_vec();
        if let Some(p) = leg_a.send(&to_a) {
            phone_a.receive(leg_a_at, &p);
        }
        if let Some(p) = leg_b.send(&to_b) {
            phone_b.receive(leg_b_at, &p);
        }
        if phone_a.pull(&mut pcm) {
            heard_by_a.push(level_dbov(&pcm));
        }
        if phone_b.pull(&mut pcm) {
            heard_by_b.push(level_dbov(&pcm));
        }
    }
    assert!(
        heard_by_b.len() > 20 && heard_by_b[15..].iter().all(|l| *l > -20.0),
        "B hears A: {heard_by_b:?}"
    );
    assert!(
        heard_by_a.iter().all(|l| *l < -50.0),
        "A does not hear itself: {heard_by_a:?}"
    );
}
