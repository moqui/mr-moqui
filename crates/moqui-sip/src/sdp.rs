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

//! The SDP of a phone leg (RFC 4566, RFC 3264 offer and answer; plans/Comms-Framework.md §5.2 "codecs", D-V3): one
//! audio stream, G.711 μ-law and A-law and Opus, telephone-event for DTMF (RFC 4733), and SDES keys (RFC 4568) when the
//! media is SRTP. SIP legs are audio only, so a video stream in an offer is refused with port 0.
//!
//! Hold is the stream's direction (RFC 3264 §8.4): a re-INVITE with `a=sendonly` or `a=inactive` (or the old
//! `c=0.0.0.0`) puts this side on hold; the answer mirrors it.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::srtp::{MasterKey, SUITE};

/// The codecs a phone leg speaks, in the order offered.
pub const CODECS: [&str; 3] = ["pcmu", "pcma", "opus"];
/// The dynamic payload types this side offers.
const OPUS_PT: u8 = 111;
const EVENT_PT: u8 = 101;
const EVENT_48K_PT: u8 = 110;

/// A stream's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl Direction {
    fn attribute(self) -> &'static str {
        match self {
            Direction::SendRecv => "sendrecv",
            Direction::SendOnly => "sendonly",
            Direction::RecvOnly => "recvonly",
            Direction::Inactive => "inactive",
        }
    }

    /// The answer's direction to an offer of this one, from a side that wants `local`.
    pub fn answer_to(self, local: Direction) -> Direction {
        let (offer_sends, offer_receives) = self.flows();
        let (local_sends, local_receives) = local.flows();
        Direction::from_flows(local_sends && offer_receives, local_receives && offer_sends)
    }

    /// (sends, receives)
    pub fn flows(self) -> (bool, bool) {
        match self {
            Direction::SendRecv => (true, true),
            Direction::SendOnly => (true, false),
            Direction::RecvOnly => (false, true),
            Direction::Inactive => (false, false),
        }
    }

    fn from_flows(sends: bool, receives: bool) -> Direction {
        match (sends, receives) {
            (true, true) => Direction::SendRecv,
            (true, false) => Direction::SendOnly,
            (false, true) => Direction::RecvOnly,
            (false, false) => Direction::Inactive,
        }
    }
}

/// The codec both sides settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecChoice {
    /// The name moqui-rtc's `codec::by_name` knows: `pcmu`, `pcma` or `opus`.
    pub name: &'static str,
    pub payload_type: u8,
    pub clock_rate: u32,
}

/// This side's media for an offer or an answer.
#[derive(Debug, Clone)]
pub struct LocalMedia {
    /// The address and port of the leg's RTP socket, as the other side reaches them.
    pub address: IpAddr,
    pub port: u16,
    /// The key this side sends with, for SRTP; `None` for plain RTP.
    pub key: Option<MasterKey>,
    /// The codecs this side takes, preferred first.
    pub codecs: Vec<&'static str>,
    pub direction: Direction,
    /// `o=` session id and version (the version goes up with each new offer of the session).
    pub session_id: u64,
    pub version: u64,
}

impl LocalMedia {
    pub fn new(address: IpAddr, port: u16, key: Option<MasterKey>) -> Self {
        let mut id = [0u8; 4];
        let _ = getrandom::fill(&mut id);
        LocalMedia {
            address,
            port,
            key,
            codecs: CODECS.to_vec(),
            direction: Direction::SendRecv,
            session_id: u64::from(u32::from_be_bytes(id)),
            version: 1,
        }
    }
}

/// A format of the other side's offer or answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Format {
    payload_type: u8,
    name: String,
    rate: u32,
}

/// The audio stream of the other side's SDP.
#[derive(Debug, Clone)]
pub struct RemoteMedia {
    pub address: IpAddr,
    pub port: u16,
    /// `RTP/SAVP`: SRTP.
    pub secure: bool,
    formats: Vec<Format>,
    /// The first usable `AES_CM_128_HMAC_SHA1_80` key, with its tag.
    pub crypto: Option<(u32, MasterKey)>,
    pub direction: Direction,
    /// Streams other than the first audio one (video), refused in the answer.
    other_media: Vec<String>,
}

/// What an offer and its answer settled.
#[derive(Debug, Clone)]
pub struct Negotiated {
    pub codec: CodecChoice,
    /// telephone-event's payload type and clock rate, when both sides have it.
    pub dtmf: Option<(u8, u32)>,
    pub remote: SocketAddr,
    /// The keys for SRTP: this side's (sending) and the other's (receiving).
    pub local_key: Option<MasterKey>,
    pub remote_key: Option<MasterKey>,
    /// Which way media flows, from this side: `RecvOnly` or `Inactive` while the other side holds the call, `SendOnly`
    /// while this side holds it.
    pub direction: Direction,
}

/// Why an SDP did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdpError {
    /// Not SDP, or no audio stream (488 Not Acceptable Here).
    NoAudio,
    /// No codec in common.
    NoCodec,
    /// Plain RTP offered to a trunk whose profile requires SRTP, or SRTP with no usable key (D-V4: never a silent
    /// downgrade).
    Security,
}

impl std::fmt::Display for SdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SdpError::NoAudio => "no audio stream",
            SdpError::NoCodec => "no codec in common",
            SdpError::Security => "the media security does not match the trunk's profile",
        })
    }
}

fn addr_type(address: IpAddr) -> &'static str {
    if address.is_ipv6() {
        "IP6"
    } else {
        "IP4"
    }
}

fn codec_line(out: &mut String, name: &str, pt: u8) {
    match name {
        "pcmu" => {
            let _ = write!(out, "a=rtpmap:{pt} PCMU/8000\r\n");
        }
        "pcma" => {
            let _ = write!(out, "a=rtpmap:{pt} PCMA/8000\r\n");
        }
        "opus" => {
            let _ = write!(
                out,
                "a=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} useinbandfec=1;minptime=20\r\n"
            );
        }
        _ => {}
    }
}

fn static_pt(name: &str) -> Option<u8> {
    match name {
        "pcmu" => Some(0),
        "pcma" => Some(8),
        _ => None,
    }
}

/// The SDP text of a stream with these formats. `crypto_tag` is the `a=crypto` tag: 1 in an offer, the offer's chosen
/// one in an answer (RFC 4568 §5.1.2).
fn write_sdp(
    local: &LocalMedia,
    formats: &[(u8, &str)],
    events: &[(u8, u32)],
    direction: Direction,
    crypto_tag: u32,
) -> String {
    let at = addr_type(local.address);
    let proto = if local.key.is_some() { "RTP/SAVP" } else { "RTP/AVP" };
    let mut out = String::new();
    let _ = write!(
        out,
        "v=0\r\no=moqui {} {} IN {at} {}\r\ns=-\r\nc=IN {at} {}\r\nt=0 0\r\nm=audio {} {proto}",
        local.session_id, local.version, local.address, local.address, local.port
    );
    for (pt, _) in formats {
        let _ = write!(out, " {pt}");
    }
    for (pt, _) in events {
        let _ = write!(out, " {pt}");
    }
    out.push_str("\r\n");
    for (pt, name) in formats {
        codec_line(&mut out, name, *pt);
    }
    for (pt, rate) in events {
        let _ = write!(out, "a=rtpmap:{pt} telephone-event/{rate}\r\na=fmtp:{pt} 0-16\r\n");
    }
    out.push_str("a=ptime:20\r\n");
    let _ = write!(out, "a={}\r\n", direction.attribute());
    if let Some(key) = &local.key {
        let _ = write!(out, "a=crypto:{crypto_tag} {SUITE} inline:{}\r\n", key.inline());
    }
    out
}

/// This side's offer.
pub fn offer(local: &LocalMedia) -> String {
    let formats: Vec<(u8, &str)> = local
        .codecs
        .iter()
        .map(|name| (static_pt(name).unwrap_or(OPUS_PT), *name))
        .collect();
    let mut events = vec![(EVENT_PT, 8000)];
    if local.codecs.contains(&"opus") {
        events.push((EVENT_48K_PT, 48_000));
    }
    write_sdp(local, &formats, &events, local.direction, 1)
}

/// The audio stream of an SDP.
pub fn parse(text: &str) -> Result<RemoteMedia, SdpError> {
    let mut session_address = None;
    let mut session_direction = None;
    let mut media: Option<RemoteMedia> = None;
    let mut media_address = None;
    let mut media_direction = None;
    let mut other_media = Vec::new();
    // within a second audio or a video stream, attributes are skipped
    let mut in_other = false;
    let mut fmt_list: Vec<u8> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((kind, value)) = line.split_once('=') else {
            continue;
        };
        match kind {
            "c" => {
                let address = value
                    .split_whitespace()
                    .nth(2)
                    .and_then(|a| a.split('/').next()?.parse().ok());
                if media.is_none() {
                    session_address = address;
                } else if !in_other {
                    media_address = address;
                }
            }
            "m" => {
                let mut parts = value.split_whitespace();
                let kind = parts.next().unwrap_or_default();
                if kind == "audio" && media.is_none() {
                    let port: u16 = parts
                        .next()
                        .and_then(|p| p.split('/').next()?.parse().ok())
                        .ok_or(SdpError::NoAudio)?;
                    let proto = parts.next().unwrap_or_default();
                    fmt_list = parts.filter_map(|p| p.parse().ok()).collect();
                    media = Some(RemoteMedia {
                        address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                        port,
                        secure: proto.eq_ignore_ascii_case("RTP/SAVP"),
                        formats: Vec::new(),
                        crypto: None,
                        direction: Direction::SendRecv,
                        other_media: Vec::new(),
                    });
                    in_other = false;
                } else {
                    let proto = value.split_whitespace().nth(2).unwrap_or("RTP/AVP");
                    let fmt = value.split_whitespace().nth(3).unwrap_or("0");
                    other_media.push(format!("{kind} 0 {proto} {fmt}"));
                    in_other = true;
                }
            }
            "a" => {
                let (name, rest) = value.split_once(':').unwrap_or((value, ""));
                let direction = match name {
                    "sendrecv" => Some(Direction::SendRecv),
                    "sendonly" => Some(Direction::SendOnly),
                    "recvonly" => Some(Direction::RecvOnly),
                    "inactive" => Some(Direction::Inactive),
                    _ => None,
                };
                if media.is_none() {
                    if direction.is_some() {
                        session_direction = direction;
                    }
                    continue;
                }
                if in_other {
                    continue;
                }
                let Some(m) = media.as_mut() else { continue };
                if direction.is_some() {
                    media_direction = direction;
                } else if name == "rtpmap" {
                    let Some((pt, encoding)) = rest.split_once(' ') else {
                        continue;
                    };
                    let Ok(payload_type) = pt.trim().parse::<u8>() else {
                        continue;
                    };
                    let mut enc = encoding.split('/');
                    let codec = enc.next().unwrap_or_default().to_ascii_lowercase();
                    let rate: u32 = enc.next().and_then(|r| r.parse().ok()).unwrap_or(8000);
                    m.formats.push(Format {
                        payload_type,
                        name: codec,
                        rate,
                    });
                } else if name == "crypto" && m.crypto.is_none() {
                    let mut parts = rest.split_whitespace();
                    let tag = parts.next().and_then(|t| t.parse().ok());
                    let suite = parts.next().unwrap_or_default();
                    let key = parts.next().and_then(MasterKey::from_inline);
                    if let (Some(tag), true, Some(key)) = (tag, suite.eq_ignore_ascii_case(SUITE), key) {
                        m.crypto = Some((tag, key));
                    }
                }
            }
            _ => {}
        }
    }
    let mut media = media.ok_or(SdpError::NoAudio)?;
    // the static types need no rtpmap
    for pt in &fmt_list {
        if !media.formats.iter().any(|f| f.payload_type == *pt) {
            if let Some(name) = match pt {
                0 => Some("pcmu"),
                8 => Some("pcma"),
                _ => None,
            } {
                media.formats.push(Format {
                    payload_type: *pt,
                    name: name.to_string(),
                    rate: 8000,
                });
            }
        }
    }
    // in the m= line's order, only the formats it lists
    media.formats.retain(|f| fmt_list.contains(&f.payload_type));
    media
        .formats
        .sort_by_key(|f| fmt_list.iter().position(|p| *p == f.payload_type));
    media.address = media_address.or(session_address).ok_or(SdpError::NoAudio)?;
    media.direction = media_direction.or(session_direction).unwrap_or(Direction::SendRecv);
    if media.address.is_unspecified() {
        // RFC 2543 hold: c=0.0.0.0 means do not send to me
        media.direction = match media.direction {
            Direction::SendRecv | Direction::SendOnly => Direction::SendOnly,
            _ => Direction::Inactive,
        };
    }
    media.other_media = other_media;
    Ok(media)
}

fn codec_of(format: &Format) -> Option<CodecChoice> {
    let name = match (format.name.as_str(), format.rate) {
        ("pcmu", 8000) => "pcmu",
        ("pcma", 8000) => "pcma",
        ("opus", 48_000) => "opus",
        _ => return None,
    };
    Some(CodecChoice {
        name,
        payload_type: format.payload_type,
        clock_rate: format.rate,
    })
}

fn telephone_event(media: &RemoteMedia, rate: u32) -> Option<(u8, u32)> {
    let events: Vec<&Format> = media.formats.iter().filter(|f| f.name == "telephone-event").collect();
    events
        .iter()
        .find(|f| f.rate == rate)
        .or(events.first())
        .map(|f| (f.payload_type, f.rate))
}

fn remote_key(media: &RemoteMedia, local: &LocalMedia) -> Result<Option<MasterKey>, SdpError> {
    match (local.key.is_some(), media.secure) {
        (true, true) => media
            .crypto
            .as_ref()
            .map(|(_, k)| Some(k.clone()))
            .ok_or(SdpError::Security),
        (false, false) => Ok(None),
        _ => Err(SdpError::Security),
    }
}

/// This side's answer to the other side's offer, and what it settles. `local.key` decides the security: SRTP with a
/// key, plain RTP without; an offer of the other kind is refused, never downgraded.
pub fn answer(offer: &RemoteMedia, local: &LocalMedia) -> Result<(String, Negotiated), SdpError> {
    let remote_key = remote_key(offer, local)?;
    // the offerer's order first (RFC 3264 §6.1), among what this side takes
    let codec = offer
        .formats
        .iter()
        .filter_map(codec_of)
        .find(|c| local.codecs.contains(&c.name))
        .ok_or(SdpError::NoCodec)?;
    let dtmf = telephone_event(offer, codec.clock_rate);
    let direction = offer.direction.answer_to(local.direction);
    let events: Vec<(u8, u32)> = dtmf.into_iter().collect();
    let tag = offer.crypto.as_ref().map(|(tag, _)| *tag).unwrap_or(1);
    let mut sdp = write_sdp(local, &[(codec.payload_type, codec.name)], &events, direction, tag);
    for refused in &offer.other_media {
        let _ = write!(sdp, "m={refused}\r\n");
    }
    Ok((
        sdp,
        Negotiated {
            codec,
            dtmf,
            remote: SocketAddr::new(offer.address, offer.port),
            local_key: local.key.clone(),
            remote_key,
            direction,
        },
    ))
}

/// What the other side's answer to this side's offer settles.
pub fn accept(local: &LocalMedia, answer: &RemoteMedia) -> Result<Negotiated, SdpError> {
    let remote_key = remote_key(answer, local)?;
    let codec = answer
        .formats
        .iter()
        .filter_map(codec_of)
        .find(|c| local.codecs.contains(&c.name))
        .ok_or(SdpError::NoCodec)?;
    // the answerer's direction, seen from here
    let (they_send, they_receive) = answer.direction.flows();
    let (we_send, we_receive) = local.direction.flows();
    Ok(Negotiated {
        codec,
        dtmf: telephone_event(answer, codec.clock_rate),
        remote: SocketAddr::new(answer.address, answer.port),
        local_key: local.key.clone(),
        remote_key,
        direction: Direction::from_flows(we_send && they_receive, we_receive && they_send),
    })
}

#[cfg(test)]
#[path = "sdp_tests.rs"]
mod tests;
