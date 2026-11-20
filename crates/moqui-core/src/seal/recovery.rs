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

//! Shares (plans/Sealed-Secrets.md §3.3, §6 "What the root key wraps before S1"): a 32-byte key split k of n by
//! `moqui_vault::shamir`, each share kept on the server only as SHA-256(salt ‖ index ‖ share), so a wrong share is
//! refused when it is entered rather than after k of them give a wrong key. The recovery shares split the recovery key,
//! which wraps the root key for every method but `dev`; the `shamir` method's shares split its own key the same way.
//! A recovery share opens any method's seal and authorizes `rekey` and `rotate-root`.

use moqui_vault::keys::Key32;
use moqui_vault::rng::OsRng;
use moqui_vault::shamir::{self, Share};
use sha2::Digest as _;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use super::file::Recovery;
use super::{decode, encode, RootKey, SealConf};
use crate::context::Error;

/// The associated data of the root key wrapped by the recovery key.
const RECOVERY_AD: &[&[u8]] = &[b"moqui-seal", b"root", b"recovery"];

/// The salt, the hash of each share and the shares' texts of `secret` split `threshold` of `shares`.
pub(crate) struct Split {
    pub(crate) salt: String,
    pub(crate) hashes: Vec<String>,
    pub(crate) shares: Vec<Zeroizing<String>>,
}

/// Split `secret`, `threshold` of `shares`.
pub(crate) fn split(secret: &Key32, threshold: u8, shares: u8) -> Result<Split, Error> {
    let mut salt = [0_u8; 16];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut salt);
    let parts = shamir::split(secret.as_bytes(), threshold, shares, &mut OsRng)
        .map_err(|err| Error::new(format!("could not split the key: {err}")))?;
    Ok(Split {
        salt: encode(&salt),
        hashes: parts.iter().map(|part| encode(&hash(&salt, part))).collect(),
        shares: parts.iter().map(Share::to_text).collect(),
    })
}

/// SHA-256(salt ‖ index ‖ share value).
fn hash(salt: &[u8], share: &Share) -> [u8; 32] {
    let mut digest = sha2::Sha256::new();
    digest.update(salt);
    digest.update([share.index]);
    digest.update(share.value.as_slice());
    digest.finalize().into()
}

/// A share's text checked against the hashes of its split: the share, or `None` when it is not one of them (a typing
/// error, a share of another seal or split).
pub(crate) fn check(text: &str, salt: &str, hashes: &[String]) -> Result<Option<Share>, Error> {
    let Ok(share) = Share::from_text(text) else {
        return Ok(None);
    };
    let salt = decode(salt)?;
    let Some(expected) = hashes.get(usize::from(share.index).wrapping_sub(1)) else {
        return Ok(None);
    };
    let expected = decode(expected)?;
    Ok(bool::from(hash(&salt, &share).as_slice().ct_eq(&expected)).then_some(share))
}

/// The key `shares` give back.
pub(crate) fn combine(shares: &[Share]) -> Result<Key32, Error> {
    let secret = shamir::combine(shares).map_err(|err| Error::new(format!("the shares do not combine: {err}")))?;
    Key32::from_slice(&secret).map_err(|err| Error::new(format!("the shares do not give a key: {err}")))
}

/// The recovery part of a new seal for `root`, and the shares to issue.
pub(crate) fn make(root: &RootKey, conf: &SealConf) -> Result<(Recovery, Vec<Zeroizing<String>>), Error> {
    let key = Key32::random(&mut OsRng);
    let split = split(&key, conf.threshold, conf.shares)?;
    Ok((
        Recovery {
            threshold: conf.threshold,
            shares: conf.shares,
            salt: split.salt,
            share_hashes: split.hashes,
            wrapped_root: wrap_root(&key, root),
        },
        split.shares,
    ))
}

/// `root` wrapped by the recovery key.
pub(crate) fn wrap_root(key: &Key32, root: &RootKey) -> String {
    encode(&moqui_vault::aead::seal(key, RECOVERY_AD, root.expose(), &mut OsRng))
}

/// The root key the recovery key opens.
pub(crate) fn open_root(key: &Key32, recovery: &Recovery) -> Result<RootKey, Error> {
    let opened = moqui_vault::aead::open(key, RECOVERY_AD, &decode(&recovery.wrapped_root)?)
        .map_err(|_| Error::new("the recovery shares do not open this seal"))?;
    RootKey::from_slice(&opened)
}

/// The recovery key from `texts`, each checked against `recovery`'s hashes: `Ok(None)` when one of them is wrong.
///
/// # Errors
///
/// Fewer than the threshold, or the same share twice.
pub(crate) fn key_from(texts: &[Zeroizing<String>], recovery: &Recovery) -> Result<Option<Key32>, Error> {
    if texts.len() < usize::from(recovery.threshold) {
        return Err(Error::new(format!(
            "{} recovery shares are needed, {} were given",
            recovery.threshold,
            texts.len()
        )));
    }
    let mut shares: Vec<Share> = Vec::with_capacity(texts.len());
    for text in texts {
        let Some(share) = check(text, &recovery.salt, &recovery.share_hashes)? else {
            return Ok(None);
        };
        if shares.iter().any(|other| other.index == share.index) {
            return Err(Error::new(format!("recovery share {} was given twice", share.index)));
        }
        shares.push(share);
    }
    combine(&shares).map(Some)
}
