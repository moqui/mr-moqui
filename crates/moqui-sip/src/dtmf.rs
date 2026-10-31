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

//! DTMF as RTP events (RFC 4733; plans/Comms-Framework.md §5.2 "DTMF"): the keypad of a phone leg, both ways.
//!
//! Sending, a digit is a run of event packets every 20 ms with the timestamp of its start, the marker on the first,
//! the duration growing, then three copies of the last with the end bit; the next digit follows a short gap. Receiving,
//! a digit is reported once, when its first packet comes, whatever arrives again of it.

use std::collections::VecDeque;

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

/// The digits a leg receives.
#[derive(Default)]
pub struct DtmfReceiver {
    /// The RTP timestamp of the last digit reported.
    last: Option<u32>,
}

impl DtmfReceiver {
    /// An event packet with its RTP timestamp; the digit when it is a new one.
    pub fn receive(&mut self, timestamp: u32, payload: &[u8]) -> Option<char> {
        let [event, ..] = payload else {
            return None;
        };
        if payload.len() < 4 || self.last == Some(timestamp) {
            return None;
        }
        let digit = digit_of(*event)?;
        self.last = Some(timestamp);
        Some(digit)
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
        assert_eq!(digit_of(10), Some('*'));
        assert_eq!(digit_of(16), None);
    }
}
