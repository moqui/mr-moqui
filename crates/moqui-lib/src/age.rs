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

//! The [age](https://age-encryption.org/v1) file format for one X25519 recipient, streamed: what this writes the `age`
//! and `rage` tools decrypt with the recipient's identity (`age -d -i key.txt`). A host that encrypts holds only the
//! public `age1...` recipient. [`decrypt`] reads the same format back, for tests and small files.

use std::io::{self, Read, Write};

use base64::Engine as _;
use chacha20poly1305::aead::{Aead as _, KeyInit as _};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use hkdf::Hkdf;
use hmac::Mac as _;
use sha2::Sha256;

use crate::error::Error;

const VERSION_LINE: &str = "age-encryption.org/v1";
const X25519_INFO: &[u8] = b"age-encryption.org/v1/X25519";
/// Plaintext bytes in each payload chunk.
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
const RECIPIENT_HRP: &str = "age";
const IDENTITY_HRP: &str = "age-secret-key-";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD_NO_PAD
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("no random bytes: {err}")))?;
    Ok(bytes)
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    // 32 bytes is far below HKDF-SHA-256's limit
    let _ = Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut out);
    out
}

/// The public `age1...` key a file is encrypted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipient([u8; 32]);

impl Recipient {
    /// # Errors
    ///
    /// When `text` is not an `age1...` X25519 recipient.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        let bad = || Error::new("not an age X25519 recipient (age1...)");
        let (hrp, data) = bech32::decode(text).ok_or_else(bad)?;
        if hrp != RECIPIENT_HRP || text.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(bad());
        }
        Ok(Self(data.try_into().map_err(|_| bad())?))
    }
}

impl std::fmt::Display for Recipient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&bech32::encode(RECIPIENT_HRP, &self.0))
    }
}

/// The secret `AGE-SECRET-KEY-1...` key that decrypts.
pub struct Identity([u8; 32]);

impl Identity {
    /// # Errors
    ///
    /// When the system has no random bytes.
    pub fn generate() -> Result<Self, Error> {
        Ok(Self(random()?))
    }

    /// # Errors
    ///
    /// When `text` is not an `AGE-SECRET-KEY-1...` identity.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        let bad = || Error::new("not an age X25519 identity (AGE-SECRET-KEY-1...)");
        let (hrp, data) = bech32::decode(text).ok_or_else(bad)?;
        if hrp != IDENTITY_HRP || text.bytes().any(|byte| byte.is_ascii_lowercase()) {
            return Err(bad());
        }
        Ok(Self(data.try_into().map_err(|_| bad())?))
    }

    #[must_use]
    pub fn recipient(&self) -> Recipient {
        Recipient(MontgomeryPoint::mul_base_clamped(self.0).to_bytes())
    }

    /// The `AGE-SECRET-KEY-1...` text.
    #[must_use]
    pub fn secret_text(&self) -> String {
        bech32::encode(IDENTITY_HRP, &self.0).to_ascii_uppercase()
    }
}

/// The header for a fresh file key and the MAC that ends it: `age-encryption.org/v1`, one X25519 stanza, `--- mac`.
fn header(recipient: &Recipient, file_key: &[u8; 16]) -> Result<Vec<u8>, Error> {
    let ephemeral: [u8; 32] = random()?;
    let share = MontgomeryPoint::mul_base_clamped(ephemeral).to_bytes();
    let shared = MontgomeryPoint(recipient.0).mul_clamped(ephemeral).to_bytes();
    if shared == [0; 32] {
        return Err(Error::new("the age recipient is a low-order point"));
    }
    let mut salt = [0_u8; 64];
    salt[..32].copy_from_slice(&share);
    salt[32..].copy_from_slice(&recipient.0);
    let wrap_key = hkdf(&salt, &shared, X25519_INFO);
    let wrapped = ChaCha20Poly1305::new(&wrap_key.into())
        .encrypt(&Nonce::default(), &file_key[..])
        .map_err(|_| Error::new("could not wrap the age file key"))?;
    // the wrapped key is 32 bytes, 43 base64 characters: one stanza line, shorter than 64
    let mut text = format!(
        "{VERSION_LINE}\n-> X25519 {}\n{}\n---",
        b64().encode(share),
        b64().encode(&wrapped)
    );
    let mac_key = hkdf(&[], file_key, b"header");
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(&mac_key).map_err(|_| Error::new("bad header MAC key"))?;
    mac.update(text.as_bytes());
    text.push(' ');
    text.push_str(&b64().encode(mac.finalize().into_bytes()));
    text.push('\n');
    Ok(text.into_bytes())
}

fn chunk_nonce(counter: u64, last: bool) -> Nonce {
    let mut nonce = Nonce::default();
    nonce[3..11].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = u8::from(last);
    nonce
}

/// Encrypts what is written to it into `inner`. [`finish`](Self::finish) writes the last chunk; a file dropped
/// before that does not decrypt.
pub struct Encryptor<W: Write> {
    inner: W,
    cipher: ChaCha20Poly1305,
    buffer: Vec<u8>,
    counter: u64,
}

impl<W: Write> Encryptor<W> {
    /// Write the header and the payload nonce to `inner`.
    ///
    /// # Errors
    ///
    /// When the system has no random bytes or `inner` fails.
    pub fn new(mut inner: W, recipient: &Recipient) -> Result<Self, Error> {
        let file_key: [u8; 16] = random()?;
        let nonce: [u8; 16] = random()?;
        let io = |err| Error::new("could not write the age header").with_source(err);
        inner.write_all(&header(recipient, &file_key)?).map_err(io)?;
        inner.write_all(&nonce).map_err(io)?;
        let payload_key = hkdf(&nonce, &file_key, b"payload");
        Ok(Self {
            inner,
            cipher: ChaCha20Poly1305::new(&payload_key.into()),
            buffer: Vec::with_capacity(CHUNK + TAG),
            counter: 0,
        })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let sealed = self
            .cipher
            .encrypt(&chunk_nonce(self.counter, last), self.buffer.as_slice())
            .map_err(|_| io::Error::other("age chunk encryption failed"))?;
        self.inner.write_all(&sealed)?;
        self.buffer.clear();
        self.counter += 1;
        Ok(())
    }

    /// Seal the last chunk and return `inner`, flushed.
    ///
    /// # Errors
    ///
    /// When `inner` fails.
    pub fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for Encryptor<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        // a full chunk is sealed only once more follows, so the one `finish` seals is the last and not empty
        if self.buffer.len() == CHUNK && !data.is_empty() {
            self.seal(false)?;
        }
        let take = data.len().min(CHUNK - self.buffer.len());
        self.buffer.extend_from_slice(&data[..take]);
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Decrypt a whole age file encrypted to `identity`'s recipient.
///
/// # Errors
///
/// When the file is not age v1 with an X25519 stanza for `identity`, or it was changed or cut short.
pub fn decrypt(identity: &Identity, input: &mut dyn Read) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    input
        .read_to_end(&mut data)
        .map_err(|err| Error::new("could not read the age file").with_source(err))?;
    let bad = |what: &str| Error::new(format!("not a readable age file: {what}"));
    let mut lines = Vec::new();
    let mut at = 0;
    loop {
        let end = data[at..]
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| bad("header"))?
            + at;
        let line = std::str::from_utf8(&data[at..end]).map_err(|_| bad("header"))?;
        at = end + 1;
        let done = line.starts_with("---");
        lines.push(line);
        if done {
            break;
        }
    }
    if lines.first() != Some(&VERSION_LINE) {
        return Err(bad("version"));
    }
    let mut file_key = None;
    let mut index = 1;
    while index + 1 < lines.len() {
        let args: Vec<&str> = lines[index].split(' ').collect();
        if args.first() != Some(&"->") {
            return Err(bad("stanza"));
        }
        // a body runs to its first line shorter than 64 characters
        let mut body = String::new();
        index += 1;
        while index + 1 < lines.len() {
            let line = lines[index];
            body.push_str(line);
            index += 1;
            if line.len() < 64 {
                break;
            }
        }
        if args.get(1) != Some(&"X25519") || file_key.is_some() {
            continue;
        }
        let share: [u8; 32] = b64()
            .decode(args.get(2).copied().unwrap_or(""))
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| bad("stanza"))?;
        let wrapped = b64().decode(&body).map_err(|_| bad("stanza"))?;
        let shared = MontgomeryPoint(share).mul_clamped(identity.0).to_bytes();
        let mut salt = [0_u8; 64];
        salt[..32].copy_from_slice(&share);
        salt[32..].copy_from_slice(&identity.recipient().0);
        let wrap_key = hkdf(&salt, &shared, X25519_INFO);
        file_key = ChaCha20Poly1305::new(&wrap_key.into())
            .decrypt(&Nonce::default(), wrapped.as_slice())
            .ok();
    }
    let file_key = file_key.ok_or_else(|| bad("no stanza for this identity"))?;
    let mac_line = lines.last().copied().unwrap_or("");
    let mac = b64()
        .decode(mac_line.strip_prefix("--- ").unwrap_or(""))
        .map_err(|_| bad("MAC"))?;
    let header_len = at - mac_line.len() - 1 + 3;
    let mut check = hmac::Hmac::<Sha256>::new_from_slice(&hkdf(&[], &file_key, b"header")).map_err(|_| bad("MAC"))?;
    check.update(&data[..header_len]);
    check.verify_slice(&mac).map_err(|_| bad("header MAC"))?;
    let nonce = data.get(at..at + 16).ok_or_else(|| bad("nonce"))?;
    let cipher = ChaCha20Poly1305::new(&hkdf(nonce, &file_key, b"payload").into());
    let mut out = Vec::new();
    let sealed = &data[at + 16..];
    let chunks: Vec<&[u8]> = sealed.chunks(CHUNK + TAG).collect();
    for (counter, chunk) in chunks.iter().enumerate() {
        let last = counter + 1 == chunks.len();
        let plain = cipher
            .decrypt(&chunk_nonce(u64::try_from(counter).unwrap_or(u64::MAX), last), *chunk)
            .map_err(|_| bad("payload changed or cut short"))?;
        out.extend_from_slice(&plain);
    }
    if chunks.is_empty() {
        return Err(bad("no payload"));
    }
    Ok(out)
}

/// Bech32 (BIP 173, not bech32m) of 8-bit data, as age writes its keys; no length limit.
mod bech32 {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];

    fn polymod(values: impl Iterator<Item = u8>) -> u32 {
        let mut chk = 1_u32;
        for value in values {
            let top = chk >> 25;
            chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(value);
            for (bit, generator) in GEN.iter().enumerate() {
                if (top >> bit) & 1 == 1 {
                    chk ^= generator;
                }
            }
        }
        chk
    }

    fn hrp_expand(hrp: &str) -> Vec<u8> {
        let bytes = hrp.as_bytes();
        let mut out: Vec<u8> = bytes.iter().map(|byte| byte >> 5).collect();
        out.push(0);
        out.extend(bytes.iter().map(|byte| byte & 31));
        out
    }

    fn regroup(data: &[u8], from: u32, to: u32, pad: bool) -> Option<Vec<u8>> {
        let mut acc = 0_u32;
        let mut bits = 0_u32;
        let mut out = Vec::new();
        let max = (1_u32 << to) - 1;
        for value in data {
            acc = (acc << from) | u32::from(*value);
            bits += from;
            while bits >= to {
                bits -= to;
                out.push(((acc >> bits) & max) as u8);
            }
        }
        if pad {
            if bits > 0 {
                out.push(((acc << (to - bits)) & max) as u8);
            }
        } else if bits >= from || (acc << (to - bits)) & max != 0 {
            return None;
        }
        Some(out)
    }

    pub(super) fn encode(hrp: &str, data: &[u8]) -> String {
        let values = regroup(data, 8, 5, true).unwrap_or_default();
        let checked = polymod(hrp_expand(hrp).into_iter().chain(values.iter().copied()).chain([0; 6])) ^ 1;
        let mut out = format!("{hrp}1");
        for value in values
            .iter()
            .copied()
            .chain((0..6).map(|i| ((checked >> (5 * (5 - i))) & 31) as u8))
        {
            out.push(char::from(CHARSET[usize::from(value)]));
        }
        out
    }

    /// The lowercase human-readable part and the data, when the checksum holds. Mixed case is refused.
    pub(super) fn decode(text: &str) -> Option<(String, Vec<u8>)> {
        if text.bytes().any(|byte| byte.is_ascii_lowercase()) && text.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return None;
        }
        let lower = text.to_ascii_lowercase();
        let split = lower.rfind('1')?;
        let (hrp, rest) = (&lower[..split], &lower[split + 1..]);
        if hrp.is_empty() || rest.len() < 6 {
            return None;
        }
        let values: Vec<u8> = rest
            .bytes()
            .map(|byte| CHARSET.iter().position(|c| *c == byte).map(|at| at as u8))
            .collect::<Option<_>>()?;
        if polymod(hrp_expand(hrp).into_iter().chain(values.iter().copied())) != 1 {
            return None;
        }
        let data = regroup(&values[..values.len() - 6], 5, 8, false)?;
        Some((hrp.to_string(), data))
    }
}

#[cfg(test)]
#[path = "age_tests.rs"]
mod tests;
