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

//! Shamir's secret sharing over GF(2^8), k of n (plans/Sealed-Secrets.md §3.3, 3 of 5 by default; the organization
//! recovery key of plans/archive/Vault-Instances.md §3.10). Each byte of the secret is the constant term of its own random
//! polynomial of degree k-1, and share `x` holds each polynomial's value at `x`.
//!
//! The field arithmetic is constant time: multiplication is shift-and-add with masks, not log and exp tables, and
//! nothing branches on a secret byte. Share indexes are public and may be branched on.
//!
//! A share's text form is `share-<index as two hex digits>-` and then Crockford base32 of the version, the index, the
//! value and a 4-byte SHA-256 checksum, in groups of five; case, spaces and the look-alikes O/I/L are forgiven, and a
//! typo fails the checksum.

use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::rng::Rng;
use crate::text;
use crate::Error;

const TEXT_VERSION: u8 = 1;
const CHECKSUM_LEN: usize = 4;
const CHECKSUM_LABEL: &[u8] = b"moqui-vault share";
const GROUP: usize = 5;

/// One share: its x coordinate (1 to 255) and the polynomials' values there, one byte per secret byte.
#[derive(Clone)]
pub struct Share {
    pub index: u8,
    pub value: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Share {{ index: {}, value: redacted ({} bytes) }}",
            self.index,
            self.value.len()
        )
    }
}

impl Share {
    /// The text to print or type: `share-03-` and checksummed base32 in groups of five.
    pub fn to_text(&self) -> Zeroizing<String> {
        let mut payload = Zeroizing::new(Vec::with_capacity(self.value.len() + 2 + CHECKSUM_LEN));
        payload.push(TEXT_VERSION);
        payload.push(self.index);
        payload.extend_from_slice(&self.value);
        let checksum = checksum(&payload);
        payload.extend_from_slice(&checksum);
        let encoded = text::base32_grouped(&payload, GROUP);
        Zeroizing::new(format!("share-{:02x}-{}", self.index, *encoded))
    }

    /// Reads [`Share::to_text`]'s form back, refusing a wrong checksum, version or index.
    pub fn from_text(text: &str) -> Result<Share, Error> {
        let compact: Zeroizing<String> =
            Zeroizing::new(text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect());
        let lower = Zeroizing::new(compact.to_ascii_lowercase());
        let Some(rest) = lower.strip_prefix("share") else {
            return Err(Error::Format("a share starts with share-"));
        };
        let Some((index_hex, body)) = rest.split_at_checked(2) else {
            return Err(Error::Format("share index"));
        };
        let index_hex: String = index_hex
            .chars()
            .map(|c| match c {
                'o' => '0',
                'i' | 'l' => '1',
                other => other,
            })
            .collect();
        let index = u8::from_str_radix(&index_hex, 16).map_err(|_| Error::Format("share index"))?;
        let payload = text::base32_decode(body)?;
        let Some((content, sum)) = payload.split_at_checked(payload.len().saturating_sub(CHECKSUM_LEN)) else {
            return Err(Error::Format("share too short"));
        };
        if content.len() < 3 || !bool::from(checksum(content).ct_eq(sum)) {
            return Err(Error::Format("share checksum does not match: check for a typo"));
        }
        if content[0] != TEXT_VERSION {
            return Err(Error::Format("share version"));
        }
        if content[1] != index || index == 0 {
            return Err(Error::Format("share index"));
        }
        Ok(Share {
            index,
            value: Zeroizing::new(content[2..].to_vec()),
        })
    }
}

fn checksum(content: &[u8]) -> [u8; CHECKSUM_LEN] {
    let digest = Sha256::new()
        .chain_update(CHECKSUM_LABEL)
        .chain_update(content)
        .finalize();
    let mut out = [0u8; CHECKSUM_LEN];
    out.copy_from_slice(&digest[..CHECKSUM_LEN]);
    out
}

/// Multiplication in GF(2^8) modulo x^8 + x^4 + x^3 + x + 1, in constant time.
pub(crate) fn gf_mul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut product = 0u8;
    for _ in 0..8 {
        // All ones when the low bit of b is set, else zero: add a without a branch.
        product ^= a & 0u8.wrapping_sub(b & 1);
        let carry = 0u8.wrapping_sub(a >> 7);
        a = (a << 1) ^ (0x1b & carry);
        b >>= 1;
    }
    product
}

/// The inverse in GF(2^8), as a^254 by a fixed sequence of squarings and multiplications; 0 maps to 0.
pub(crate) fn gf_inv(a: u8) -> u8 {
    // 254 = 0b11111110: square and multiply over a public exponent, so the work does not depend on `a`.
    let mut result = 1u8;
    let mut power = a;
    for bit in 0..8 {
        if (254u32 >> bit) & 1 == 1 {
            result = gf_mul(result, power);
        }
        power = gf_mul(power, power);
    }
    result
}

/// Splits `secret` into `n` shares, any `k` of which give it back (1 < k <= n <= 255).
pub fn split(secret: &[u8], k: u8, n: u8, rng: &mut dyn Rng) -> Result<Vec<Share>, Error> {
    if k < 2 || k > n {
        return Err(Error::Shares("k must be at least 2 and at most n"));
    }
    if secret.is_empty() {
        return Err(Error::Shares("the secret is empty"));
    }
    let degree = usize::from(k - 1);
    // coefficients[i * degree + j] is the x^(j+1) coefficient of byte i's polynomial.
    let mut coefficients = Zeroizing::new(vec![0u8; secret.len() * degree]);
    rng.fill(&mut coefficients);
    let mut shares = Vec::with_capacity(usize::from(n));
    for x in 1..=n {
        let mut value = Zeroizing::new(Vec::with_capacity(secret.len()));
        for (at, &constant) in secret.iter().enumerate() {
            let own = &coefficients[at * degree..(at + 1) * degree];
            // Horner's rule from the highest coefficient down to the constant term.
            let mut y = 0u8;
            for &coefficient in own.iter().rev() {
                y = gf_mul(y, x) ^ coefficient;
            }
            value.push(gf_mul(y, x) ^ constant);
        }
        shares.push(Share { index: x, value });
    }
    Ok(shares)
}

/// The secret from `shares` by Lagrange interpolation at 0. Shamir cannot tell too few shares from enough: fewer than
/// the split's k give a wrong value, not an error, so a caller checks the result (a key check, an AEAD open).
pub fn combine(shares: &[Share]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let [first, ..] = shares else {
        return Err(Error::Shares("no shares"));
    };
    if shares.len() < 2 {
        return Err(Error::Shares("at least two shares are needed"));
    }
    let len = first.value.len();
    if len == 0 {
        return Err(Error::Shares("a share is empty"));
    }
    for (at, share) in shares.iter().enumerate() {
        if share.index == 0 {
            return Err(Error::Shares("share index 0"));
        }
        if share.value.len() != len {
            return Err(Error::Shares("shares of different lengths"));
        }
        if shares[..at].iter().any(|earlier| earlier.index == share.index) {
            return Err(Error::Shares("the same share twice"));
        }
    }
    let mut secret = Zeroizing::new(vec![0u8; len]);
    for (j, share) in shares.iter().enumerate() {
        // The Lagrange basis at 0: the product over the other shares of x_m / (x_m - x_j); minus is xor here.
        let mut numerator = 1u8;
        let mut denominator = 1u8;
        for (m, other) in shares.iter().enumerate() {
            if m != j {
                numerator = gf_mul(numerator, other.index);
                denominator = gf_mul(denominator, other.index ^ share.index);
            }
        }
        let basis = gf_mul(numerator, gf_inv(denominator));
        for (out, &y) in secret.iter_mut().zip(share.value.iter()) {
            *out ^= gf_mul(y, basis);
        }
    }
    Ok(secret)
}

#[cfg(test)]
#[path = "shamir_tests.rs"]
mod tests;
