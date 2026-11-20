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

//! DTMF as RTP events (RFC 4733; plans/archive/Comms-Framework.md §5.2 "DTMF"): the keypad of a phone leg, both ways.
//!
//! Sending, a digit is a run of event packets every 20 ms with the timestamp of its start, the marker on the first,
//! the duration growing, then three copies of the last with the end bit; the next digit follows a short gap. Receiving,
//! a digit is reported once, when its first packet comes, whatever arrives again of it; a new one only once the last
//! ended or 40 ms of RTP time after it began, and [`DigitRate`] holds a leg to [`MAX_DIGITS_PER_SECOND`], so a far end
//! that sends event packets with a new timestamp each does not make a digit (a task and a service call) of every one
//! (SEC-COM-86).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a sent digit lasts, and the gap after it, in 20 ms packets.
const TONE_PACKETS: u32 = 5;
const GAP_PACKETS: u32 = 3;
/// How many times the last packet of a digit is sent (RFC 4733 §2.5.1.4).
const END_REPEATS: u32 = 3;
/// The digits a send may queue.
const MAX_QUEUED: usize = 64;

/// The event code of a keypad digit (RFC 4733 §3.2).
pub fn event_of(digit: char) -> Option<u8> {
    match digit {
        '0'..='9' => Some(digit as u8 - b'0'),
        '*' => Some(10),
        '#' => Some(11),
        'A'..='D' => Some(12 + (digit as u8 - b'A')),
        'a'..='d' => Some(12 + (digit as u8 - b'a')),
        _ => None,
    }
}

/// The keypad digit of an event code.
pub fn digit_of(event: u8) -> Option<char> {
    match event {
        0..=9 => Some(char::from(b'0' + event)),
        10 => Some('*'),
        11 => Some('#'),
        12..=15 => Some(char::from(b'A' + event - 12)),
        _ => None,
    }
}

/// One event packet to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPacket {
    pub payload: [u8; 4],
    /// The marker bit: the first packet of a digit.
    pub marker: bool,
    /// The digit's start, in packets since its first, so the leg keeps one timestamp for the whole digit.
    pub new_digit: bool,
}

enum Sending {
    Idle,
    Tone { event: u8, packet: u32 },
    End { event: u8, repeat: u32 },
    Gap { packet: u32 },
}

/// The digits a leg is sending.
pub struct DtmfSender {
    queue: VecDeque<u8>,
    state: Sending,
    /// Samples a 20 ms packet adds to the duration (the event's clock rate / 50).
    step: u16,
}

impl DtmfSender {
    /// For an event clock of `clock_rate` (8000 for telephone-event/8000).
    pub fn new(clock_rate: u32) -> Self {
        DtmfSender {
            queue: VecDeque::new(),
            state: Sending::Idle,
            step: (clock_rate / 50).clamp(1, u32::from(u16::MAX)) as u16,
        }
    }

    /// Queues digits; anything not a keypad digit is skipped. The number of digits queued.
    pub fn queue(&mut self, digits: &str) -> usize {
        let mut queued = 0;
        for event in digits.chars().filter_map(event_of) {
            if self.queue.len() >= MAX_QUEUED {
                break;
            }
            self.queue.push_back(event);
            queued += 1;
        }
        queued
    }

    /// Whether a digit is being sent (the leg's voice waits while it is).
    pub fn is_active(&self) -> bool {
        !matches!(self.state, Sending::Idle) || !self.queue.is_empty()
    }

    fn payload(event: u8, end: bool, duration: u16) -> [u8; 4] {
        // volume 10 (-10 dBm0), as phones send
        let flags = if end { 0x80 } else { 0 } | 10;
        let d = duration.to_be_bytes();
        [event, flags, d[0], d[1]]
    }

    /// The packet of this 20 ms tick, if a digit is being sent; `None` in a gap or when idle (send voice).
    pub fn tick(&mut self) -> Option<EventPacket> {
        loop {
            match self.state {
                Sending::Idle => {
                    let event = self.queue.pop_front()?;
                    self.state = Sending::Tone { event, packet: 0 };
                }
                Sending::Tone { event, packet } => {
                    let duration = self.step.saturating_mul((packet + 1) as u16);
                    self.state = if packet + 1 >= TONE_PACKETS {
                        Sending::End { event, repeat: 0 }
                    } else {
                        Sending::Tone {
                            event,
                            packet: packet + 1,
                        }
                    };
                    return Some(EventPacket {
                        payload: Self::payload(event, false, duration),
                        marker: packet == 0,
                        new_digit: packet == 0,
                    });
                }
                Sending::End { event, repeat } => {
                    let duration = self.step.saturating_mul(TONE_PACKETS as u16);
                    self.state = if repeat + 1 >= END_REPEATS {
                        Sending::Gap { packet: 0 }
                    } else {
                        Sending::End {
                            event,
                            repeat: repeat + 1,
                        }
                    };
                    return Some(EventPacket {
                        payload: Self::payload(event, true, duration),
                        marker: false,
                        new_digit: false,
                    });
                }
                Sending::Gap { packet } => {
                    self.state = if packet + 1 >= GAP_PACKETS {
                        Sending::Idle
                    } else {
                        Sending::Gap { packet: packet + 1 }
                    };
                    return None;
                }
            }
        }
    }
}

/// The RTP time after a digit began before another can begin without its end: 40 ms at the 8 kHz event clock.
const MIN_DIGIT_TICKS: u32 = 320;
/// The digits a leg takes in a second; a person, or a dialer at the shortest tone and gap, sends far fewer.
pub const MAX_DIGITS_PER_SECOND: u32 = 20;

/// The digits a leg receives.
#[derive(Default)]
pub struct DtmfReceiver {
    /// The RTP timestamp of the last digit reported.
    last: Option<u32>,
    /// The last digit's end bit came.
    ended: bool,
}

impl DtmfReceiver {
    /// An event packet with its RTP timestamp; the digit when it is a new one: a timestamp other than the last
    /// digit's, once that digit ended or [`MIN_DIGIT_TICKS`] after it began.
    pub fn receive(&mut self, timestamp: u32, payload: &[u8]) -> Option<char> {
        let [event, flags, ..] = payload else {
            return None;
        };
        if payload.len() < 4 {
            return None;
        }
        if let Some(last) = self.last {
            if last == timestamp {
                self.ended |= flags & 0x80 != 0;
                return None;
            }
            if !self.ended && timestamp.wrapping_sub(last) < MIN_DIGIT_TICKS {
                return None;
            }
        }
        let digit = digit_of(*event)?;
        self.last = Some(timestamp);
        self.ended = flags & 0x80 != 0;
        Some(digit)
    }
}

/// A leg's digits in the current second, at most [`MAX_DIGITS_PER_SECOND`]. Sans-IO: the caller passes the time.
#[derive(Default)]
pub struct DigitRate {
    since: Option<Instant>,
    taken: u32,
}

impl DigitRate {
    /// Whether a digit received at `now` is taken.
    pub fn take(&mut self, now: Instant) -> bool {
        match self.since {
            Some(since) if now.saturating_duration_since(since) < Duration::from_secs(1) => {}
            _ => {
                self.since = Some(now);
                self.taken = 0;
            }
        }
        if self.taken >= MAX_DIGITS_PER_SECOND {
            return false;
        }
        self.taken += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_go_out_as_events_and_come_back_once() {
        let mut sender = DtmfSender::new(8000);
        assert_eq!(sender.queue("1#x"), 2);
        let mut receiver = DtmfReceiver::default();
        let mut got = String::new();
        let mut timestamp = 0u32;
        let mut packets = 0;
        let mut ends = 0;
        for tick in 0..40 {
            if let Some(packet) = sender.tick() {
                packets += 1;
                if packet.new_digit {
                    timestamp = tick * 160;
                    assert!(packet.marker);
                }
                if packet.payload[1] & 0x80 != 0 {
                    ends += 1;
                    assert_eq!(u16::from_be_bytes([packet.payload[2], packet.payload[3]]), 800);
                }
                if let Some(d) = receiver.receive(timestamp, &packet.payload) {
                    got.push(d);
                }
            }
        }
        assert_eq!(got, "1#");
        assert_eq!(packets, 2 * (TONE_PACKETS + END_REPEATS));
        assert_eq!(ends, 2 * END_REPEATS);
        assert!(!sender.is_active());
        assert_eq!(event_of('D'), Some(15));
        // a new timestamp is a new digit only once the last ended or 40 ms after it began (SEC-COM-86)
        let mut receiver = DtmfReceiver::default();
        let start = [1u8, 0x0a, 0, 160];
        let end = [1u8, 0x8a, 3, 32];
        assert_eq!(receiver.receive(1000, &start), Some('1'));
        assert_eq!(
            receiver.receive(1001, &start),
            None,
            "a new timestamp each packet is no new digit"
        );
        assert_eq!(receiver.receive(1319, &start), None);
        assert_eq!(
            receiver.receive(1320, &start),
            Some('1'),
            "40 ms on, a digit whose end was lost"
        );
        assert_eq!(receiver.receive(1320, &end), None);
        assert_eq!(
            receiver.receive(1321, &[2u8, 0x0a, 0, 160]),
            Some('2'),
            "after its end, at once"
        );
        // a leg takes at most MAX_DIGITS_PER_SECOND a second
        let mut rate = DigitRate::default();
        let now = Instant::now();
        let taken = (0..100).filter(|_| rate.take(now)).count();
        assert_eq!(taken, MAX_DIGITS_PER_SECOND as usize);
        assert!(rate.take(now + Duration::from_secs(1)));
        assert_eq!(digit_of(10), Some('*'));
        assert_eq!(digit_of(16), None);
    }
}
