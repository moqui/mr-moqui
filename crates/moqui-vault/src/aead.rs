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

//! AES-256-GCM with a random 96-bit nonce and associated data that names what a value is (plans/Vault-Instances.md
//! §3.2, AEAD everywhere). The associated data is a list of parts, each written with its length, so no two lists give
//! the same bytes: a value sealed for one vault, item, version or purpose does not open as another.
//!
//! The sealed form is `nonce (12) ‖ ciphertext ‖ tag (16)`.

use aes_gcm::aead::{Aead as _, KeyInit as _, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use zeroize::Zeroizing;

use crate::keys::Key32;
use crate::rng::Rng;
use crate::Error;

pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
/// The largest item body, before padding (plans/Vault-Instances.md §3.2: an item is at most 64 KiB).
pub const MAX_BODY: usize = 64 * 1024;
/// Bodies are padded to a multiple of this, so a ciphertext's length tells little about its value.
pub const BODY_STEP: usize = 256;

/// The associated data for `parts`: each part as a 4-byte big-endian length and its bytes.
pub fn associated_data(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(parts.iter().map(|part| part.len() + 4).sum());
    for part in parts {
        let len = u32::try_from(part.len()).expect("an associated data part is far under 4 GiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// Seals `plaintext` under `key` with `ad` as its associated data and a new random nonce.
pub fn seal(key: &Key32, ad: &[&[u8]], plaintext: &[u8], rng: &mut dyn Rng) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut nonce);
    seal_with_nonce(key, &associated_data(ad), &nonce, plaintext)
}

fn seal_with_nonce(key: &Key32, aad: &[u8], nonce: &[u8; NONCE_LEN], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.as_bytes().into());
    let sealed = cipher
        .encrypt(&Nonce::from(*nonce), Payload { msg: plaintext, aad })
        .expect("AES-GCM seals any plaintext under its size limit");
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&sealed);
    out
}

/// Opens what [`seal`] made with the same key and associated data.
pub fn open(key: &Key32, ad: &[&[u8]], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let Some((nonce, ciphertext)) = sealed.split_at_checked(NONCE_LEN) else {
        return Err(Error::Format("sealed value shorter than its nonce"));
    };
    if ciphertext.len() < TAG_LEN {
        return Err(Error::Format("sealed value shorter than its tag"));
    }
    let nonce = Nonce::try_from(nonce).map_err(|_| Error::Format("nonce"))?;
    let cipher = Aes256Gcm::new(key.as_bytes().into());
    let aad = associated_data(ad);
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::Open)
}

/// Seals an item body: padded to a multiple of [`BODY_STEP`] (a 0x80 byte, then zeros), at most [`MAX_BODY`] before
/// padding.
pub fn seal_body(key: &Key32, ad: &[&[u8]], body: &[u8], rng: &mut dyn Rng) -> Result<Vec<u8>, Error> {
    if body.len() > MAX_BODY {
        return Err(Error::TooLarge);
    }
    let padded_len = (body.len() / BODY_STEP + 1) * BODY_STEP;
    let mut padded = Zeroizing::new(Vec::with_capacity(padded_len));
    padded.extend_from_slice(body);
    padded.push(0x80);
    padded.resize(padded_len, 0);
    Ok(seal(key, ad, &padded, rng))
}

/// Opens what [`seal_body`] made and takes the padding off.
pub fn open_body(key: &Key32, ad: &[&[u8]], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let max_padded = (MAX_BODY / BODY_STEP + 1) * BODY_STEP;
    if sealed.len() > NONCE_LEN + max_padded + TAG_LEN {
        return Err(Error::TooLarge);
    }
    let mut padded = open(key, ad, sealed)?;
    if padded.is_empty() || padded.len() % BODY_STEP != 0 {
        return Err(Error::Format("body padding"));
    }
    // The padding is the last 0x80 and the zeros after it; it was authenticated with the body, so this scan's
    // timing tells only the padding length, which the ciphertext length already shows to within a step.
    let Some(end) = padded.iter().rposition(|&byte| byte != 0) else {
        return Err(Error::Format("body padding"));
    };
    if padded[end] != 0x80 {
        return Err(Error::Format("body padding"));
    }
    padded.truncate(end);
    Ok(padded)
}

#[cfg(test)]
#[path = "aead_tests.rs"]
mod tests;
