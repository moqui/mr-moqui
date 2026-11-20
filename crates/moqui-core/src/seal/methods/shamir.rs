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

//! The `shamir` method (plans/Sealed-Secrets.md §3.3, separation of duties): the root key sealed under a key of its
//! own, split `seal@threshold` of `seal@shares` among operators, who each enter one share. Its shares are not the
//! recovery shares: `rekey` to this method issues new ones and leaves the recovery shares as they are.

use moqui_vault::keys::Key32;
use moqui_vault::rng::OsRng;
use moqui_vault::shamir::Share;
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::bytes_field;
use crate::context::Error;
use crate::seal::{encode, recovery, RootKey, SealConf};

/// The associated data of the root key sealed under the method's key.
const AD: &[&[u8]] = &[b"moqui-seal", b"root", b"shamir"];

/// A new `shamir` seal: the method's metadata, and the shares to issue.
pub struct Wrapped {
    pub metadata: Map<String, Value>,
    pub shares: Vec<Zeroizing<String>>,
}

/// The method's metadata for `root`, and the shares to issue.
///
/// # Errors
///
/// The split is refused.
pub fn wrap(root: &RootKey, conf: &SealConf) -> Result<Wrapped, Error> {
    let key = Key32::random(&mut OsRng);
    let split = recovery::split(&key, conf.threshold, conf.shares)?;
    let wrapped = moqui_vault::aead::seal(&key, AD, root.expose(), &mut OsRng);
    let Value::Object(metadata) = json!({
        "threshold": conf.threshold,
        "shares": conf.shares,
        "salt": split.salt,
        "share_hashes": split.hashes,
        "wrapped_root": encode(&wrapped),
    }) else {
        unreachable!("a json! object literal is an object");
    };
    Ok(Wrapped {
        metadata,
        shares: split.shares,
    })
}

/// The salt and share hashes of the method's metadata.
pub(crate) fn hashes(metadata: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    let salt = metadata
        .get("salt")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("seal.json's shamir method has no salt"))?;
    let hashes = metadata
        .get("share_hashes")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("seal.json's shamir method has no share hashes"))?
        .iter()
        .map(|hash| hash.as_str().unwrap_or_default().to_string())
        .collect();
    Ok((salt.to_string(), hashes))
}

/// The root key the method's shares open.
///
/// # Errors
///
/// The shares do not combine, or the key they give does not open the seal.
pub fn unwrap(metadata: &Map<String, Value>, shares: &[Share]) -> Result<RootKey, Error> {
    let key = recovery::combine(shares)?;
    let opened = moqui_vault::aead::open(&key, AD, &bytes_field(metadata, "wrapped_root")?)
        .map_err(|_| Error::new("the shares do not open this seal"))?;
    RootKey::from_slice(&opened)
}
