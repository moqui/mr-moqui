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

//! Text a person prints and types back: Crockford base32 in groups, forgiving case, spaces, dashes and the
//! look-alikes O, I and L (shares, recovery codes, generated keys), and hex for the vectors and the wasm calls.

use zeroize::Zeroizing;

use crate::Error;

const BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Crockford base32 of `bytes`, upper case, in groups of `group` characters joined by dashes.
pub(crate) fn base32_grouped(bytes: &[u8], group: usize) -> Zeroizing<String> {
    let mut plain = Zeroizing::new(String::with_capacity(bytes.len() * 8 / 5 + 1));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            plain.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        plain.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    let mut out = Zeroizing::new(String::with_capacity(plain.len() + plain.len() / group));
    for (at, c) in plain.chars().enumerate() {
        if at > 0 && at % group == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// One base32 character's value, with the look-alikes read as the digits they resemble.
fn base32_value(c: u8) -> Option<u32> {
    let c = match c.to_ascii_uppercase() {
        b'O' => b'0',
        b'I' | b'L' => b'1',
        other => other,
    };
    BASE32.iter().position(|&d| d == c).map(|at| at as u32)
}

/// The bytes of base32 text, skipping spaces and dashes. Trailing bits short of a byte are dropped.
pub(crate) fn base32_decode(text: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() * 5 / 8));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &c in text.as_bytes() {
        if c == b'-' || c.is_ascii_whitespace() {
            continue;
        }
        let Some(value) = base32_value(c) else {
            return Err(Error::Format("a character that is not base32"));
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    out
}

/// The bytes of hex text.
pub fn unhex(text: &str) -> Result<Vec<u8>, Error> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(Error::Format("hex of odd length"));
    }
    let value = |d: u8| char::from(d).to_digit(16).ok_or(Error::Format("not hex"));
    digits
        .chunks(2)
        .map(|pair| Ok((value(pair[0])? << 4 | value(pair[1])?) as u8))
        .collect()
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
