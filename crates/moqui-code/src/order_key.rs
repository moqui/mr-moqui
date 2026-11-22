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

//! Fractional order keys for pinned and active threads (plans/agent-code/02-Orchestration.md section 1.1): a key
//! between two others, so a reorder writes one row. The algorithm of the `fractional-indexing` package T3 Code uses
//! (base 62, an integer part whose head letter gives its length, then a fraction with no trailing zero), so keys made
//! by a T3 client sort the same here.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ZERO: u8 = b'0';
const LAST: u8 = b'z';

/// Why a key cannot be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderKeyError {
    /// A key is not a valid order key.
    Invalid(String),
    /// The lower key is not below the upper key.
    NotOrdered,
    /// No key is left at that end of the range.
    Exhausted,
}

impl std::fmt::Display for OrderKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(key) => write!(f, "{key} is not an order key"),
            Self::NotOrdered => f.write_str("the lower order key is not below the upper one"),
            Self::Exhausted => f.write_str("no order key is left at that end"),
        }
    }
}

fn digit(byte: u8) -> Option<usize> {
    DIGITS.iter().position(|d| *d == byte)
}

fn integer_length(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some(usize::from(head - b'a') + 2),
        b'A'..=b'Z' => Some(usize::from(b'Z' - head) + 2),
        _ => None,
    }
}

fn integer_part(key: &[u8]) -> Result<&[u8], OrderKeyError> {
    let invalid = || OrderKeyError::Invalid(String::from_utf8_lossy(key).into_owned());
    let length = key.first().copied().and_then(integer_length).ok_or_else(invalid)?;
    key.get(..length).ok_or_else(invalid)
}

fn validate(key: &[u8]) -> Result<(), OrderKeyError> {
    let invalid = || OrderKeyError::Invalid(String::from_utf8_lossy(key).into_owned());
    let smallest = smallest_integer();
    if key == smallest.as_slice() || key.iter().any(|b| digit(*b).is_none()) {
        return Err(invalid());
    }
    let integer = integer_part(key)?;
    let fraction = &key[integer.len()..];
    if fraction.last() == Some(&ZERO) {
        return Err(invalid());
    }
    Ok(())
}

fn smallest_integer() -> Vec<u8> {
    let mut key = vec![b'A'];
    key.extend(std::iter::repeat_n(ZERO, 26));
    key
}

/// A fraction strictly between `a` and `b` (`None` = above every fraction).
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let mut n = 0;
        while a.get(n).copied().unwrap_or(ZERO) == b.get(n).copied().unwrap_or(0) && n < b.len() {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(midpoint(a.get(n..).unwrap_or_default(), Some(&b[n..])));
            return out;
        }
    }
    let digit_a = a.first().and_then(|d| digit(*d)).unwrap_or(0);
    let digit_b = b.and_then(|b| b.first()).and_then(|d| digit(*d)).unwrap_or(DIGITS.len());
    if digit_b - digit_a > 1 {
        // round half up, as Math.round
        return vec![DIGITS[(digit_a + digit_b).div_ceil(2)]];
    }
    match b {
        Some(b) if b.len() > 1 => vec![b[0]],
        _ => {
            let mut out = vec![DIGITS[digit_a]];
            out.extend(midpoint(a.get(1..).unwrap_or_default(), None));
            out
        }
    }
}

fn increment_integer(x: &[u8]) -> Option<Vec<u8>> {
    let head = x[0];
    let mut digits = x[1..].to_vec();
    let mut carry = true;
    for d in digits.iter_mut().rev() {
        let next = digit(*d).unwrap_or(0) + 1;
        if next == DIGITS.len() {
            *d = ZERO;
        } else {
            *d = DIGITS[next];
            carry = false;
            break;
        }
    }
    if !carry {
        let mut out = vec![head];
        out.extend(digits);
        return Some(out);
    }
    match head {
        b'Z' => Some(vec![b'a', ZERO]),
        b'z' => None,
        _ => {
            let next = head + 1;
            if next > b'a' {
                digits.push(ZERO);
            } else {
                digits.pop();
            }
            let mut out = vec![next];
            out.extend(digits);
            Some(out)
        }
    }
}

fn decrement_integer(x: &[u8]) -> Option<Vec<u8>> {
    let head = x[0];
    let mut digits = x[1..].to_vec();
    let mut borrow = true;
    for d in digits.iter_mut().rev() {
        match digit(*d).unwrap_or(0).checked_sub(1) {
            None => *d = LAST,
            Some(prev) => {
                *d = DIGITS[prev];
                borrow = false;
                break;
            }
        }
    }
    if !borrow {
        let mut out = vec![head];
        out.extend(digits);
        return Some(out);
    }
    match head {
        b'a' => Some(vec![b'Z', LAST]),
        b'A' => None,
        _ => {
            let prev = head - 1;
            if prev < b'Z' {
                digits.push(LAST);
            } else {
                digits.pop();
            }
            let mut out = vec![prev];
            out.extend(digits);
            Some(out)
        }
    }
}

/// True when `key` is an order key a client may send.
#[must_use]
pub fn is_valid(key: &str) -> bool {
    key.len() <= 64 && validate(key.as_bytes()).is_ok()
}

/// A key above `a` and below `b`; `None` is the open end. Two `None`s give the first key, `a0`.
///
/// # Errors
///
/// A key that is not an order key, `a` not below `b`, or a range end with no key left.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> Result<String, OrderKeyError> {
    let (a, b) = (a.map(str::as_bytes), b.map(str::as_bytes));
    if let Some(a) = a {
        validate(a)?;
    }
    if let Some(b) = b {
        validate(b)?;
    }
    if let (Some(a), Some(b)) = (a, b) {
        if a >= b {
            return Err(OrderKeyError::NotOrdered);
        }
    }
    let key = match (a, b) {
        (None, None) => b"a0".to_vec(),
        (None, Some(b)) => {
            let ib = integer_part(b)?;
            let fb = &b[ib.len()..];
            if ib == smallest_integer().as_slice() {
                let mut out = ib.to_vec();
                out.extend(midpoint(b"", Some(fb)));
                out
            } else if ib < b {
                ib.to_vec()
            } else {
                decrement_integer(ib).ok_or(OrderKeyError::Exhausted)?
            }
        }
        (Some(a), None) => {
            let ia = integer_part(a)?;
            let fa = &a[ia.len()..];
            match increment_integer(ia) {
                Some(next) => next,
                None => {
                    let mut out = ia.to_vec();
                    out.extend(midpoint(fa, None));
                    out
                }
            }
        }
        (Some(a), Some(b)) => {
            let (ia, ib) = (integer_part(a)?, integer_part(b)?);
            let (fa, fb) = (&a[ia.len()..], &b[ib.len()..]);
            if ia == ib {
                let mut out = ia.to_vec();
                out.extend(midpoint(fa, Some(fb)));
                out
            } else {
                let next = increment_integer(ia).ok_or(OrderKeyError::Exhausted)?;
                if next.as_slice() < b {
                    next
                } else {
                    let mut out = ia.to_vec();
                    out.extend(midpoint(fa, None));
                    out
                }
            }
        }
    };
    Ok(String::from_utf8(key).unwrap_or_default())
}

#[cfg(test)]
#[path = "order_key_tests.rs"]
mod tests;
