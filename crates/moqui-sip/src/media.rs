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

//! A phone leg's audio, sans-IO (plans/archive/Comms-Framework.md §5.1, §5.2; the M0 gate in §6.2).
//!
//! A phone takes one stream, so each phone leg hears a mix of everyone but itself (§5.1). [`PhoneMedia`] sits between
//! the leg's RTP socket, which the host owns, and the call's mixer:
//!
//! - **In.** [`PhoneMedia::receive`] takes each datagram: SRTP is checked and decrypted, RTCP dropped, telephone-event
//!   packets become digits, and the voice is held in a small jitter buffer by sequence number. Every 20 ms
//!   [`PhoneMedia::pull`] decodes the next frame (G.711, G.722 or Opus, concealing a lost one) and resamples it to the
//!   mixer's 48 kHz, for the leg's `PcmSource` in the call's room.
//! - **Out.** Every 20 ms [`PhoneMedia::send`] takes the room's mix without this leg (`AudioRoom` `minus(leg)`),
//!   resamples it to the codec's rate, encodes it and writes the packet, or an RFC 4733 event while a digit is sent.
//!   Nothing is sent while the other side holds the call.
//! - **Symmetric RTP.** Packets go back to where the leg's packets come from (RFC 4961), the SDP's address until the
//!   first one: a trunk behind NAT is reached. With SRTP only an authenticated packet moves it, so a guessed port
//!   can neither inject audio nor take the call's audio away (the RTP-bleed class). Plain RTP has no such proof: it
//!   takes packets only from the SDP's address (on any port, for a NAT that changes ports), latches to the first
//!   one's port, and drops everything else. Plain RTP is for a PBX on a private network, where the SDP's address is
//!   where the media comes from.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;

use moqui_rtc::audio::codec::{self, Codec};
use moqui_rtc::audio::resample::Resampler;
use moqui_rtc::audio::{FRAME, RATE};

use crate::dtmf::{DtmfReceiver, DtmfSender};
use crate::rtp::{self, RtpHeader};
use crate::sdp::{CodecChoice, Direction, Negotiated};
use crate::srtp;

/// Packets held before playing starts: 40 ms of delay to absorb jitter.
const PREFILL: usize = 2;
/// Packets held at most; past this the oldest go, so the delay stays bounded.
const MAX_HELD: usize = 10;
/// Frames concealed in a row before the leg counts as stopped (silent until its packets come again).
const MAX_CONCEAL: u32 = 5;

/// What a datagram was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    Audio,
    /// A new digit from the keypad (RFC 4733).
    Digit(char),
    /// RTCP, another payload type, a late or repeated packet.
    Ignored,
    /// Not RTP, or SRTP that failed its tag or replay check.
    Rejected,
}

/// What a leg's media did, for its quality notes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaStats {
    pub received: u64,
    pub played: u64,
    pub concealed: u64,
    pub late: u64,
    pub rejected: u64,
    pub sent: u64,
}

/// The media of one phone leg.
pub struct PhoneMedia {
    choice: CodecChoice,
    codec: Box<dyn Codec>,
    /// The codec's samples per 20 ms.
    frame: usize,
    /// The RTP clock's step per 20 ms: the frame, except for G.722, whose 16 kHz audio keeps an 8000 clock.
    clock_step: u32,
    dtmf_pt: Option<u8>,
    up: Option<Resampler>,
    down: Option<Resampler>,
    tx: Option<srtp::Sender>,
    rx: Option<srtp::Receiver>,
    remote: SocketAddr,
    /// The address in the SDP: plain RTP is taken only from it.
    signaled: std::net::IpAddr,
    /// Another address plain RTP is taken from: the one a registered phone signals from (its NAT's), for a phone that
    /// puts its private address in the SDP ([`PhoneMedia::allow_source`]).
    also: Option<std::net::IpAddr>,
    /// The other side's SRTP key: a re-INVITE with another needs a new leg media.
    remote_key: Option<crate::srtp::MasterKey>,
    latched: bool,
    direction: Direction,
    // sending
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    event_timestamp: u32,
    started: bool,
    dtmf_tx: DtmfSender,
    // receiving
    held: BTreeMap<u64, Vec<u8>>,
    last_seq: Option<(u16, u64)>,
    next: Option<u64>,
    fifo: VecDeque<i16>,
    misses: u32,
    dtmf_rx: DtmfReceiver,
    scratch: Vec<i16>,
    stats: MediaStats,
}

fn random_u32() -> u32 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    u32::from_be_bytes(bytes)
}

impl PhoneMedia {
    /// The media of what an offer and answer settled.
    pub fn new(settled: &Negotiated) -> Result<Self, String> {
        let codec = codec::by_name(settled.codec.name).ok_or_else(|| format!("no codec {}", settled.codec.name))?;
        let rate = codec.rate();
        let (up, down) = if rate == RATE {
            (None, None)
        } else {
            (Some(Resampler::new(rate, RATE)), Some(Resampler::new(RATE, rate)))
        };
        let tx = match (&settled.local_key, &settled.remote_key) {
            (Some(local), Some(_)) => Some(srtp::Sender::new(local)),
            (None, None) => None,
            _ => return Err("SRTP keys on one side only".to_string()),
        };
        let rx = settled.remote_key.as_ref().map(srtp::Receiver::new);
        Ok(PhoneMedia {
            choice: settled.codec,
            frame: (rate / 50) as usize,
            clock_step: codec.clock_rate() / 50,
            codec,
            dtmf_pt: settled.dtmf.map(|(pt, _)| pt),
            up,
            down,
            tx,
            rx,
            remote: settled.remote,
            signaled: settled.remote.ip(),
            also: None,
            remote_key: settled.remote_key.clone(),
            latched: false,
            direction: settled.direction,
            ssrc: random_u32(),
            sequence: random_u32() as u16,
            timestamp: random_u32(),
            event_timestamp: 0,
            started: false,
            dtmf_tx: DtmfSender::new(settled.dtmf.map(|(_, rate)| rate).unwrap_or(settled.codec.clock_rate)),
            held: BTreeMap::new(),
            last_seq: None,
            next: None,
            fifo: VecDeque::new(),
            misses: 0,
            dtmf_rx: DtmfReceiver::default(),
            scratch: Vec::with_capacity(FRAME),
            stats: MediaStats::default(),
        })
    }

    pub fn codec(&self) -> CodecChoice {
        self.choice
    }

    /// Where this leg's packets go.
    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn stats(&self) -> MediaStats {
        self.stats
    }

    /// Whether a new settlement can [`update`](Self::update) this media: the same codec and keys. Otherwise it needs a
    /// new leg media.
    pub fn takes(&self, settled: &Negotiated) -> bool {
        self.choice == settled.codec && self.remote_key == settled.remote_key
    }

    /// A re-INVITE's new settlement (hold, resume, a new address). A changed codec or keys need a new leg media.
    pub fn update(&mut self, settled: &Negotiated) {
        self.direction = settled.direction;
        if settled.remote != self.remote && !settled.remote.ip().is_unspecified() {
            self.remote = settled.remote;
            self.signaled = settled.remote.ip();
            self.latched = false;
        }
    }

    /// Takes plain RTP from `ip` too: the address a registered phone's signaling comes from, when its SDP names its
    /// private address behind a NAT (§5.2 "NAT"). The port may be any, as for the SDP's address; SRTP needs no such
    /// address, since only an authenticated packet moves it.
    pub fn allow_source(&mut self, ip: std::net::IpAddr) {
        self.also = Some(ip);
    }

    /// Queues keypad digits to send; how many were taken (none without telephone-event).
    pub fn send_digits(&mut self, digits: &str) -> usize {
        if self.dtmf_pt.is_none() {
            return 0;
        }
        self.dtmf_tx.queue(digits)
    }

    fn extend_seq(&mut self, seq: u16) -> u64 {
        let extended = match self.last_seq {
            None => u64::from(seq) + (1 << 16),
            Some((last, ext)) => {
                let delta = i64::from(seq.wrapping_sub(last) as i16);
                (ext as i64 + delta).max(0) as u64
            }
        };
        if self.last_seq.is_none_or(|(_, ext)| extended > ext) {
            self.last_seq = Some((seq, extended));
        }
        extended
    }

    /// A datagram from `from` on the leg's socket.
    pub fn receive(&mut self, from: SocketAddr, datagram: &[u8]) -> Received {
        if rtp::is_rtcp(datagram) {
            return Received::Ignored;
        }
        // plain RTP: only from the SDP's address, and once latched only from the latched source (unless the SDP had
        // no address: then the first packet latches)
        if self.rx.is_none() {
            let allowed = if self.latched {
                from == self.remote
            } else {
                self.signaled.is_unspecified() || from.ip() == self.signaled || Some(from.ip()) == self.also
            };
            if !allowed {
                self.stats.rejected += 1;
                return Received::Rejected;
            }
        }
        let mut packet = datagram.to_vec();
        if let Some(rx) = self.rx.as_mut() {
            if rx.unprotect(&mut packet).is_err() {
                self.stats.rejected += 1;
                return Received::Rejected;
            }
        }
        let Some(parsed) = rtp::parse(&packet) else {
            self.stats.rejected += 1;
            return Received::Rejected;
        };
        // symmetric RTP: answer where the leg sends from; plain RTP latches once, SRTP on each authenticated packet
        if from != self.remote && (self.rx.is_some() || !self.latched) {
            self.remote = from;
        }
        self.latched = true;
        self.stats.received += 1;
        let payload = &packet[parsed.header_len..parsed.payload_end];
        let header = parsed.header;
        if Some(header.payload_type) == self.dtmf_pt {
            return match self.dtmf_rx.receive(header.timestamp, payload) {
                Some(digit) => Received::Digit(digit),
                None => Received::Ignored,
            };
        }
        if header.payload_type != self.choice.payload_type {
            return Received::Ignored;
        }
        let extended = self.extend_seq(header.sequence);
        if self.next.is_some_and(|next| extended < next) || self.held.contains_key(&extended) {
            self.stats.late += 1;
            return Received::Ignored;
        }
        self.held.insert(extended, payload.to_vec());
        while self.held.len() > MAX_HELD {
            self.held.pop_first();
            self.stats.late += 1;
        }
        Received::Audio
    }

    /// The next 20 ms at 48 kHz into `out` (cleared first, [`FRAME`] samples when it plays). `false` when the leg is not
    /// playing (not started or stopped): nothing for the mix.
    pub fn pull(&mut self, out: &mut Vec<i16>) -> bool {
        out.clear();
        if self.next.is_none() {
            if self.held.len() < PREFILL {
                return false;
            }
            self.next = self.held.keys().next().copied();
            self.misses = 0;
        }
        while self.fifo.len() < self.frame {
            let Some(next) = self.next else { break };
            self.scratch.clear();
            if let Some(payload) = self.held.remove(&next) {
                if self.codec.decode(&payload, &mut self.scratch).is_err() {
                    self.codec.conceal(self.frame, &mut self.scratch);
                }
                self.misses = 0;
                self.stats.played += 1;
            } else {
                self.misses += 1;
                if self.misses > MAX_CONCEAL && self.held.is_empty() {
                    // stopped: silent until packets come again
                    self.next = None;
                    self.fifo.clear();
                    return false;
                }
                self.codec.conceal(self.frame, &mut self.scratch);
                self.stats.concealed += 1;
            }
            self.fifo.extend(self.scratch.iter().copied());
            self.next = Some(next + 1);
        }
        if self.fifo.len() < self.frame {
            return false;
        }
        self.scratch.clear();
        self.scratch.extend(self.fifo.drain(..self.frame));
        match self.up.as_mut() {
            Some(up) => up.process(&self.scratch, out),
            None => out.extend_from_slice(&self.scratch),
        }
        out.resize(FRAME, 0);
        true
    }

    /// The packet for this 20 ms tick, from `pcm` (the mix without this leg, [`FRAME`] samples at 48 kHz): a digit's
    /// event while one is sent, else the voice. `None` while the other side holds the call.
    pub fn send(&mut self, pcm: &[i16]) -> Option<Vec<u8>> {
        let samples = self.clock_step;
        let sends = self.direction.flows().0;
        let mut payload = Vec::with_capacity(self.frame);
        let event = if sends && self.dtmf_pt.is_some() {
            self.dtmf_tx.tick()
        } else {
            None
        };
        let header = if let (Some(event), Some(pt)) = (event, self.dtmf_pt) {
            if event.new_digit {
                self.event_timestamp = self.timestamp;
            }
            payload.extend_from_slice(&event.payload);
            RtpHeader {
                marker: event.marker,
                payload_type: pt,
                sequence: self.sequence,
                timestamp: self.event_timestamp,
                ssrc: self.ssrc,
            }
        } else {
            if !sends {
                self.timestamp = self.timestamp.wrapping_add(samples);
                return None;
            }
            self.scratch.clear();
            match self.down.as_mut() {
                Some(down) => down.process(pcm, &mut self.scratch),
                None => self.scratch.extend_from_slice(pcm),
            }
            self.scratch.resize(self.frame, 0);
            if self.codec.encode(&self.scratch, &mut payload).is_err() {
                self.timestamp = self.timestamp.wrapping_add(samples);
                return None;
            }
            RtpHeader {
                marker: !self.started,
                payload_type: self.choice.payload_type,
                sequence: self.sequence,
                timestamp: self.timestamp,
                ssrc: self.ssrc,
            }
        };
        self.started = true;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(samples);
        let mut packet = Vec::with_capacity(12 + payload.len() + 10);
        rtp::write(&header, &payload, &mut packet);
        if let Some(tx) = self.tx.as_mut() {
            tx.protect(&mut packet).ok()?;
        }
        self.stats.sent += 1;
        Some(packet)
    }
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
