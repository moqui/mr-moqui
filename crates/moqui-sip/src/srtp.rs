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

//! SRTP for a phone leg (RFC 3711; plans/Comms-Framework.md §5.2 "media"), with keys exchanged by SDES (RFC 4568,
//! `a=crypto` in the SDP over TLS signaling). One suite, the one every trunk offers: `AES_CM_128_HMAC_SHA1_80`.
//!
//! str0m keeps its SRTP context to itself, but its crypto provider is public, and it is the aws-lc-rs one the
//! browsers' DTLS-SRTP already runs on: AES-128 for the counter-mode keystream and the key derivation, HMAC-SHA1 for the
//! tag. This module is the protocol around them: the key derivation (RFC 3711 §4.3), the packet index from the
//! sequence number and the rollover counter (§3.3.1), the 80-bit tag, and a 64-packet replay window (§3.3.2). Only
//! RTP is protected; a phone leg sends no RTCP and drops the RTCP it gets.

use std::sync::OnceLock;

use base64::Engine as _;
use str0m::crypto::CryptoProvider;

/// The suite's name in `a=crypto`.
pub const SUITE: &str = "AES_CM_128_HMAC_SHA1_80";
/// The master key and salt: 16 and 14 bytes, 30 together in `inline:`.
pub const KEY_LEN: usize = 16;
pub const SALT_LEN: usize = 14;
/// The length of the authentication tag.
const TAG_LEN: usize = 10;
const REPLAY_WINDOW: u64 = 64;

fn crypto() -> &'static CryptoProvider {
    static PROVIDER: OnceLock<CryptoProvider> = OnceLock::new();
    PROVIDER.get_or_init(str0m::crypto::from_feature_flags)
}

/// A master key and salt.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterKey {
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    /// A new random key.
    pub fn random() -> Self {
        let mut bytes = [0u8; KEY_LEN + SALT_LEN];
        let _ = getrandom::fill(&mut bytes);
        Self::from_bytes(&bytes).expect("30 bytes make a key and salt")
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != KEY_LEN + SALT_LEN {
            return None;
        }
        let (key, salt) = bytes.split_at(KEY_LEN);
        Some(MasterKey {
            key: key.try_into().ok()?,
            salt: salt.try_into().ok()?,
        })
    }

    /// The `inline:` key parameter (base64 of key and salt).
    pub fn inline(&self) -> String {
        let mut bytes = Vec::with_capacity(KEY_LEN + SALT_LEN);
        bytes.extend_from_slice(&self.key);
        bytes.extend_from_slice(&self.salt);
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// From an `inline:<base64>[|lifetime]` key parameter. A key with an MKI (`|mki:len`) is not taken: its packets
    /// carry the MKI, which this side does not handle, so the stream is refused rather than fail every packet.
    pub fn from_inline(text: &str) -> Option<Self> {
        let mut parts = text.trim().strip_prefix("inline:")?.split('|');
        let encoded = parts.next()?;
        if parts.any(|p| p.contains(':')) {
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
        Self::from_bytes(&bytes)
    }
}

/// The session keys of one direction.
struct SessionKeys {
    key: [u8; 16],
    salt: [u8; SALT_LEN],
    auth: [u8; 20],
}

fn aes_block(key: &[u8; 16], input: &[u8; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    crypto().srtp_provider.srtp_aes_128_ecb_round(key, input, &mut out);
    out
}

/// AES-CM: the keystream from `iv` (its last 16 bits the block counter) XORed into `data`.
fn aes_cm(key: &[u8; 16], iv: &[u8; 16], data: &mut [u8]) {
    for (block, chunk) in data.chunks_mut(16).enumerate() {
        let mut counter = *iv;
        let low = u16::from_be_bytes([iv[14], iv[15]]).wrapping_add(block as u16);
        counter[14..].copy_from_slice(&low.to_be_bytes());
        let stream = aes_block(key, &counter);
        for (byte, k) in chunk.iter_mut().zip(stream) {
            *byte ^= k;
        }
    }
}

/// RFC 3711 §4.3.1 with a key derivation rate of 0: AES-CM of the master key over (label · 2^48) XOR salt.
fn derive(master: &MasterKey, label: u8, out: &mut [u8]) {
    let mut iv = [0u8; 16];
    iv[..SALT_LEN].copy_from_slice(&master.salt);
    iv[7] ^= label;
    out.fill(0);
    aes_cm(&master.key, &iv, out);
}

fn session_keys(master: &MasterKey) -> SessionKeys {
    let mut keys = SessionKeys {
        key: [0; 16],
        salt: [0; SALT_LEN],
        auth: [0; 20],
    };
    derive(master, 0, &mut keys.key);
    derive(master, 1, &mut keys.auth);
    derive(master, 2, &mut keys.salt);
    keys
}

fn packet_iv(salt: &[u8; SALT_LEN], ssrc: u32, index: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[..SALT_LEN].copy_from_slice(salt);
    for (i, b) in ssrc.to_be_bytes().iter().enumerate() {
        iv[4 + i] ^= b;
    }
    for (i, b) in index.to_be_bytes()[2..].iter().enumerate() {
        iv[8 + i] ^= b;
    }
    iv
}

fn tag(auth: &[u8; 20], packet: &[u8], roc: u32) -> [u8; TAG_LEN] {
    let full = crypto()
        .sha1_hmac_provider
        .sha1_hmac(auth, &[packet, &roc.to_be_bytes()]);
    let mut out = [0u8; TAG_LEN];
    out.copy_from_slice(&full[..TAG_LEN]);
    out
}

/// Why a packet was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrtpError {
    Malformed,
    Auth,
    Replay,
}

/// Protects the packets one side sends.
pub struct Sender {
    keys: SessionKeys,
    roc: u32,
    last_seq: Option<u16>,
}

impl Sender {
    pub fn new(master: &MasterKey) -> Self {
        Sender {
            keys: session_keys(master),
            roc: 0,
            last_seq: None,
        }
    }

    /// Encrypts an RTP packet's payload in place and appends its tag.
    pub fn protect(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        let parsed = crate::rtp::parse_header(packet).ok_or(SrtpError::Malformed)?;
        let seq = parsed.header.sequence;
        if let Some(last) = self.last_seq {
            if seq < last && last - seq > 0x8000 {
                self.roc = self.roc.wrapping_add(1);
            }
        }
        self.last_seq = Some(seq);
        let index = (u64::from(self.roc) << 16) | u64::from(seq);
        let iv = packet_iv(&self.keys.salt, parsed.header.ssrc, index);
        aes_cm(&self.keys.key, &iv, &mut packet[parsed.header_len..]);
        let tag = tag(&self.keys.auth, packet, self.roc);
        packet.extend_from_slice(&tag);
        Ok(())
    }
}

/// Checks and decrypts the packets the other side sends.
pub struct Receiver {
    keys: SessionKeys,
    roc: u32,
    /// The highest sequence number taken, and the replay window's bits below it.
    highest: Option<u16>,
    window: u64,
}

impl Receiver {
    pub fn new(master: &MasterKey) -> Self {
        Receiver {
            keys: session_keys(master),
            roc: 0,
            highest: None,
            window: 0,
        }
    }

    /// RFC 3711 §3.3.1: the rollover counter this sequence number belongs to.
    fn guess_roc(&self, seq: u16) -> u32 {
        let Some(s_l) = self.highest else {
            return self.roc;
        };
        if s_l < 0x8000 {
            if seq > s_l && seq - s_l > 0x8000 {
                self.roc.wrapping_sub(1)
            } else {
                self.roc
            }
        } else if s_l - 0x8000 > seq {
            self.roc.wrapping_add(1)
        } else {
            self.roc
        }
    }

    /// Checks a packet's tag and window, decrypts it in place and drops its tag.
    pub fn unprotect(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        if packet.len() < 12 + TAG_LEN {
            return Err(SrtpError::Malformed);
        }
        let body_len = packet.len() - TAG_LEN;
        // the padding count is in the encrypted payload: not read before decrypting
        let parsed = crate::rtp::parse_header(&packet[..body_len]).ok_or(SrtpError::Malformed)?;
        let seq = parsed.header.sequence;
        let roc = self.guess_roc(seq);
        let index = (u64::from(roc) << 16) | u64::from(seq);
        let current = self.highest.map(|h| (u64::from(self.roc) << 16) | u64::from(h));
        if let Some(current) = current {
            if index <= current {
                let behind = current - index;
                if behind >= REPLAY_WINDOW || self.window & (1 << behind) != 0 {
                    return Err(SrtpError::Replay);
                }
            }
        }
        let expected = tag(&self.keys.auth, &packet[..body_len], roc);
        let got = &packet[body_len..];
        if expected.iter().zip(got).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
            return Err(SrtpError::Auth);
        }
        packet.truncate(body_len);
        let iv = packet_iv(&self.keys.salt, parsed.header.ssrc, index);
        aes_cm(&self.keys.key, &iv, &mut packet[parsed.header_len..]);
        match current {
            Some(current) if index <= current => self.window |= 1 << (current - index),
            Some(current) => {
                let ahead = index - current;
                self.window = if ahead >= REPLAY_WINDOW {
                    0
                } else {
                    self.window << ahead
                };
                self.window |= 1;
                self.highest = Some(seq);
                self.roc = roc;
            }
            None => {
                self.window = 1;
                self.highest = Some(seq);
                self.roc = roc;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "srtp_tests.rs"]
mod tests;
