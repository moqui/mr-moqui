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

//! The crate's one error. Its messages name what was refused and never carry key material or plaintext.

use std::fmt;

/// Why a vault operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// An AEAD or HPKE open failed: a wrong key or password, changed associated data, or changed bytes. Which of these
    /// is never said, so a caller cannot use the error as an oracle.
    Open,
    /// Input that is not in the expected format: a length, an encoding, a version.
    Format(&'static str),
    /// A body over the 64 KiB item limit.
    TooLarge,
    /// Argon2id parameters under the floor or over the ceiling, or a salt that is too short.
    Params(&'static str),
    /// Shamir split or combine input refused (k and n out of range, a duplicate index, mismatched lengths).
    Shares(&'static str),
    /// A signature that is missing or does not verify.
    Signature,
    /// Signed and well formed, but not acceptable: an unknown signer, a role the signer did not hold, a key the user
    /// has not checked, a version lower than one already seen.
    Refused(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Open => f.write_str("does not open: wrong key or changed data"),
            Error::Format(what) => write!(f, "bad format: {what}"),
            Error::TooLarge => f.write_str("over the 64 KiB item limit"),
            Error::Params(what) => write!(f, "key derivation parameters refused: {what}"),
            Error::Shares(what) => write!(f, "shares refused: {what}"),
            Error::Signature => f.write_str("signature missing or wrong"),
            Error::Refused(what) => write!(f, "refused: {what}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
