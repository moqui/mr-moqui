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

//! A phone leg's media (plans/Comms-Framework.md §5.1, §6.5; the M0 hook points in `rtc/media.rs`): its RTP socket
//! from the `sip_rtp_ports` range and one thread, on a 20 ms tick, between that socket and the call's SFU room.
//!
//! - **In.** The phone's RTP is decoded (G.711 or Opus) and resampled to 48 kHz by [`moqui_sip::media::PhoneMedia`],
//!   and each frame is pushed into the leg's [`PcmSource`]: the browsers hear it as one more participant, and the
//!   room's consumers (the mixer here, the recorder, captions) see it too.
//! - **Out.** A [`Consumer`] of the room's audio feeds an [`AudioRoom`]; each tick its `minus(leg)` (everyone but this
//!   leg, other phone legs included) goes back to the phone, resampled to 8 kHz and encoded, or as RFC 4733 events
//!   while digits are sent. A prompt ([`Prompt`]) is mixed in over it.
//!
//! None of this needs a browser in the room. The thread ends when the leg is stopped, when the room closes (the call
//! ended) or when the media node drops the leg; the last two are reported, so the SIP side hangs up.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_rtc::audio::room::AudioRoom;
use moqui_rtc::audio::{saturate, FRAME};
use moqui_sip::media::{PhoneMedia, Received};
use moqui_sip::sdp::Negotiated;

use crate::rtc::media::{self, Consumer, Kinds, PcmSource};

/// The frames a leg's consumer queues before it drops (a second).
const CONSUMER_QUEUE: usize = 50;
const TICK: Duration = Duration::from_millis(20);

/// Audio played to the phone over what it hears, 48 kHz mono: a call's notice (`prompt.rs`), its tones and any
/// recorded speech.
#[derive(Debug, Clone)]
pub(crate) struct Prompt(pub(crate) Arc<[i16]>);

/// A prompt after the one playing, if any, so a joiner's notices (the recording's, then the captions') are each heard
/// whole: what is left of `queue` from `at`, then `next`. The new queue plays from 0.
pub(crate) fn queue_prompt(queue: &mut Vec<i16>, at: &mut usize, next: &[i16]) {
    queue.drain(..(*at).min(queue.len()));
    *at = 0;
    queue.extend_from_slice(next);
}

/// What the leg's thread is told.
pub(crate) enum LegCommand {
    /// The offer and answer settled (an outbound leg's answer, or a re-INVITE's new settlement).
    Settled(Box<Negotiated>),
    Digits(String),
    Prompt(Prompt),
    Stop,
}

/// What the leg's thread reports.
#[derive(Debug)]
pub(crate) enum LegNote {
    /// The room closed or the media node dropped the leg: the call or the leg ended.
    Gone {
        leg: String,
    },
    Digit {
        leg: String,
        digit: char,
    },
}

/// The UDP ports phone legs take their RTP sockets from.
pub(crate) struct PortRange {
    host: IpAddr,
    low: u16,
    high: u16,
    next: AtomicU16,
}

impl PortRange {
    /// `20000-20999` on `host`.
    pub(crate) fn parse(text: &str, host: IpAddr) -> Result<PortRange, String> {
        let (low, high) = text
            .trim()
            .split_once('-')
            .ok_or_else(|| format!("sip_rtp_ports {text} is not a range like 20000-20999"))?;
        let low: u16 = low
            .trim()
            .parse()
            .map_err(|_| format!("sip_rtp_ports {text}: {low} is not a port"))?;
        let high: u16 = high
            .trim()
            .parse()
            .map_err(|_| format!("sip_rtp_ports {text}: {high} is not a port"))?;
        if low == 0 || high < low {
            return Err(format!("sip_rtp_ports {text} is empty"));
        }
        Ok(PortRange {
            host,
            low,
            high,
            next: AtomicU16::new(low),
        })
    }

    /// A socket on the next free port of the range; `None` when every port is taken.
    pub(crate) fn take(&self) -> Option<UdpSocket> {
        let size = u32::from(self.high - self.low) + 1;
        for _ in 0..size {
            let port = self.next.fetch_add(1, Ordering::Relaxed);
            let port = if port < self.low || port > self.high {
                self.next.store(self.low + 1, Ordering::Relaxed);
                self.low
            } else {
                port
            };
            if let Ok(socket) = UdpSocket::bind((self.host, port)) {
                return Some(socket);
            }
        }
        None
    }
}

/// A running leg: its commands, and its thread (stopped when dropped).
pub(crate) struct LegMedia {
    commands: Sender<LegCommand>,
    thread: Option<JoinHandle<()>>,
}

impl LegMedia {
    /// Starts a leg in `room` (the `callSessionId`) as `leg` (its `callLegId`), on `socket`. `settled` is known for an
    /// inbound leg (this side answered); an outbound leg gets it with its answer.
    pub(crate) fn start(
        room: &str,
        leg: &str,
        socket: UdpSocket,
        settled: Option<Negotiated>,
        notes: tokio::sync::mpsc::UnboundedSender<LegNote>,
    ) -> Result<LegMedia, String> {
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let source =
            media::add_pcm_source(room, leg).map_err(|code| format!("the leg's audio did not start: {code}"))?;
        let consumer = media::attach(room, Kinds::Audio, CONSUMER_QUEUE)
            .map_err(|code| format!("the room's audio did not start: {code}"))?;
        let media = match settled {
            Some(settled) => Some(PhoneMedia::new(&settled)?),
            None => None,
        };
        let (commands, inbox) = channel();
        let worker = Worker {
            leg: leg.to_string(),
            socket,
            media,
            source,
            consumer,
            room: AudioRoom::new(Default::default()),
            prompt: Vec::new(),
            prompt_at: 0,
            notes,
        };
        let thread = std::thread::Builder::new()
            .name("moqui-sip-leg".to_string())
            .spawn(move || worker.run(&inbox))
            .map_err(|e| e.to_string())?;
        Ok(LegMedia {
            commands,
            thread: Some(thread),
        })
    }

    pub(crate) fn send(&self, command: LegCommand) {
        let _ = self.commands.send(command);
    }
}

impl Drop for LegMedia {
    fn drop(&mut self) {
        let _ = self.commands.send(LegCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    leg: String,
    socket: UdpSocket,
    media: Option<PhoneMedia>,
    source: PcmSource,
    consumer: Consumer,
    room: AudioRoom,
    prompt: Vec<i16>,
    prompt_at: usize,
    notes: tokio::sync::mpsc::UnboundedSender<LegNote>,
}

impl Worker {
    fn run(mut self, inbox: &Receiver<LegCommand>) {
        let mut next = Instant::now() + TICK;
        let mut buf = vec![0u8; 2048];
        let mut pcm = Vec::with_capacity(FRAME);
        loop {
            // commands, waiting until the tick
            let wait = next.saturating_duration_since(Instant::now());
            match inbox.recv_timeout(wait) {
                Ok(LegCommand::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(command) => {
                    self.command(command);
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            next += TICK;
            if next + TICK * 10 < Instant::now() {
                // far behind (a stall): start the ticks again from now rather than burst
                next = Instant::now() + TICK;
            }
            // the room's audio
            loop {
                match self.consumer.try_recv() {
                    Ok(event) => self.room.take(&event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.gone();
                        return;
                    }
                }
            }
            // the phone's packets
            while let Ok((len, from)) = self.socket.recv_from(&mut buf) {
                if let Some(media) = self.media.as_mut() {
                    if let Received::Digit(digit) = media.receive(from, &buf[..len]) {
                        let _ = self.notes.send(LegNote::Digit {
                            leg: self.leg.clone(),
                            digit,
                        });
                    }
                }
            }
            let mixer = self.room.tick();
            let mut out = mixer.minus(&self.leg).to_vec();
            out.resize(FRAME, 0);
            self.mix_prompt(&mut out);
            let Some(media) = self.media.as_mut() else { continue };
            if let Some(packet) = media.send(&out) {
                if let Err(err) = self.socket.send_to(&packet, media.remote()) {
                    tracing::debug!("SIP leg {}: RTP not sent: {err}", self.leg);
                }
            }
            if media.pull(&mut pcm) {
                if let Err("closed") = self.source.push(&pcm) {
                    self.gone();
                    return;
                }
            }
        }
    }

    fn command(&mut self, command: LegCommand) {
        match command {
            LegCommand::Settled(settled) => match self.media.as_mut() {
                Some(media) if media.takes(&settled) => media.update(&settled),
                _ => match PhoneMedia::new(&settled) {
                    Ok(media) => self.media = Some(media),
                    Err(err) => tracing::warn!("SIP leg {}: its media did not start: {err}", self.leg),
                },
            },
            LegCommand::Digits(digits) => {
                if let Some(media) = self.media.as_mut() {
                    media.send_digits(&digits);
                }
            }
            LegCommand::Prompt(prompt) => queue_prompt(&mut self.prompt, &mut self.prompt_at, &prompt.0),
            LegCommand::Stop => {}
        }
    }

    fn mix_prompt(&mut self, out: &mut [i16]) {
        if self.prompt_at >= self.prompt.len() {
            return;
        }
        let end = (self.prompt_at + out.len()).min(self.prompt.len());
        for (sample, add) in out.iter_mut().zip(&self.prompt[self.prompt_at..end]) {
            *sample = saturate(i32::from(*sample) + i32::from(*add));
        }
        self.prompt_at = end;
    }

    fn gone(&self) {
        let _ = self.notes.send(LegNote::Gone { leg: self.leg.clone() });
    }
}

/// The address in the SDP: the configured public one, else the bound one.
pub(crate) fn sdp_address(public: Option<IpAddr>, socket: &UdpSocket) -> Option<SocketAddr> {
    let bound = socket.local_addr().ok()?;
    Some(SocketAddr::new(public.unwrap_or(bound.ip()), bound.port()))
}
