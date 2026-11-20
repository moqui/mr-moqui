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
//!
//! These carry secrets, so they are constant time in the secret: no table indexed by a secret value and no early exit
//! at a bad character. Each character is mapped by arithmetic on masks; a bad one is found after the whole text. Only
//! where the separators are, which is public layout, is branched on.

use zeroize::Zeroizing;

use crate::Error;

/// 1 when `x >= n`, else 0, for values under 2^15, without a branch.
fn ge(x: u16, n: u16) -> u16 {
    (x.wrapping_sub(n) >> 15) ^ 1
}

/// 1 when `lo <= x <= hi`, else 0.
fn in_range(x: u16, lo: u16, hi: u16) -> u16 {
    ge(x, lo) & (ge(x, hi + 1) ^ 1)
}

/// 1 when `x == y`, else 0.
fn eq(x: u16, y: u16) -> u16 {
    in_range(x, y, y)
}

/// The Crockford character of a 5-bit value: digits, then the letters without I, L, O and U.
fn base32_char(v: u16) -> char {
    let code = 48 + v + 7 * ge(v, 10) + ge(v, 18) + ge(v, 20) + ge(v, 22) + ge(v, 27);
    char::from(code as u8)
}

/// A character's 5-bit value and 1, or 0 and 0 for a character that is not base32. Lower case and the look-alikes
/// O (0), I and L (1) are read too.
fn base32_value(c: u8) -> (u16, u16) {
    let c = u16::from(c);
    let upper = c - 32 * in_range(c, 97, 122);
    let digit = in_range(upper, 48, 57);
    let letter = in_range(upper, 65, 90);
    let (is_i, is_l, is_o, is_u) = (eq(upper, 73), eq(upper, 76), eq(upper, 79), eq(upper, 85));
    // A letter's value: A is 10, less one for each skipped letter (I, L, O, U) before it.
    let letter_value = upper.wrapping_sub(55)
        - ge(upper, 74) // after I
        - ge(upper, 77) // after L
        - ge(upper, 80) // after O
        - ge(upper, 86); // after U
    let plain_letter = letter & (is_i | is_l | is_o | is_u) ^ letter;
    let value = digit * upper.wrapping_sub(48) + plain_letter * letter_value + (is_i | is_l);
    let valid = digit | plain_letter | is_i | is_l | is_o;
    (value * valid, valid)
}

/// Crockford base32 of `bytes`, upper case, in groups of `group` characters joined by dashes.
pub(crate) fn base32_grouped(bytes: &[u8], group: usize) -> Zeroizing<String> {
    let chars = (bytes.len() * 8).div_ceil(5);
    let mut out = Zeroizing::new(String::with_capacity(chars + chars / group.max(1)));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    let mut written = 0;
    let mut push = |out: &mut String, v: u32| {
        if written > 0 && written % group == 0 {
            out.push('-');
        }
        out.push(base32_char((v & 31) as u16));
        written += 1;
    };
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            push(&mut out, buffer >> bits);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        push(&mut out, buffer << (5 - bits));
    }
    out
}

/// The bytes of base32 text, skipping spaces and dashes. Trailing bits short of a byte are dropped. A character that
/// is not base32 is refused once the whole text has been read.
pub(crate) fn base32_decode(text: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() * 5 / 8 + 1));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    let mut valid = 1u16;
    for &c in text.as_bytes() {
        if c == b'-' || c.is_ascii_whitespace() {
            continue;
        }
        let (value, ok) = base32_value(c);
        valid &= ok;
        buffer = ((buffer << 5) | u32::from(value)) & 0xffff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    if valid == 0 {
        return Err(Error::Format("a character that is not base32"));
    }
    Ok(out)
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    let nibble = |n: u8| {
        let n = u16::from(n);
        char::from((48 + n + 39 * ge(n, 10)) as u8)
    };
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(nibble(byte >> 4));
        out.push(nibble(byte & 15));
    }
    out
}

/// The bytes of hex text, either case; a character that is not hex is refused once the whole text has been read.
pub fn unhex(text: &str) -> Result<Vec<u8>, Error> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(Error::Format("hex of odd length"));
    }
    let value = |c: u8| {
        let c = u16::from(c);
        let (digit, lower, upper) = (in_range(c, 48, 57), in_range(c, 97, 102), in_range(c, 65, 70));
        let v = digit * c.wrapping_sub(48) + lower * c.wrapping_sub(87) + upper * c.wrapping_sub(55);
        (v & 15, digit | lower | upper)
    };
    let mut out = Vec::with_capacity(digits.len() / 2);
    let mut valid = 1u16;
    for pair in digits.chunks(2) {
        let (high, high_ok) = value(pair[0]);
        let (low, low_ok) = value(pair[1]);
        valid &= high_ok & low_ok;
        out.push(((high << 4) | low) as u8);
    }
    if valid == 0 {
        return Err(Error::Format("not hex"));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
