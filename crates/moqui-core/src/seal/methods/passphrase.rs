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

//! The `passphrase` method (plans/Sealed-Secrets.md §3.3): the root key sealed under Argon2id of a passphrase an
//! operator enters, at m=256 MiB, t=3, a server's cost. A production instance refuses a lower cost, both when it makes
//! the seal and when it opens one, so a `seal.json` edited to a cheaper cost does not open there.

use std::sync::Mutex;

use moqui_vault::kdf::{argon2id, Argon2Params};
use moqui_vault::rng::OsRng;
use serde_json::{json, Map, Value};

use super::bytes_field;
use crate::context::Error;
use crate::seal::{encode, RootKey, SealConf};

/// A server's cost: 256 MiB, 3 passes, 1 lane.
pub const SERVER_COST: Argon2Params = Argon2Params {
    m_kib: 256 * 1024,
    t: 3,
    p: 1,
};

/// The associated data of the root key sealed under the passphrase's key.
const AD: &[&[u8]] = &[b"moqui-seal", b"root", b"passphrase"];

/// The cost a test sets in place of [`SERVER_COST`].
static TEST_COST: Mutex<Option<Argon2Params>> = Mutex::new(None);

/// Make new passphrase seals at `cost` in this process (the vault crate's floor at least): a test's, which a production
/// instance refuses.
#[doc(hidden)]
pub fn set_cost_for_tests(cost: Argon2Params) {
    *moqui_lib::sync::lock(&TEST_COST) = Some(cost);
}

fn under_server_cost(cost: Argon2Params) -> bool {
    cost.m_kib < SERVER_COST.m_kib || cost.t < SERVER_COST.t
}

fn refuse_cheap(cost: Argon2Params, conf: &SealConf) -> Result<(), Error> {
    if conf.production() && under_server_cost(cost) {
        return Err(Error::new(
            "a passphrase seal under m=256 MiB, t=3 is refused on a production instance",
        ));
    }
    Ok(())
}

/// The method's metadata for `root` sealed under `passphrase`.
///
/// # Errors
///
/// A cost under the server's on a production instance, or a derivation refused.
pub fn wrap(root: &RootKey, passphrase: &str, conf: &SealConf) -> Result<Map<String, Value>, Error> {
    let cost = moqui_lib::sync::lock(&TEST_COST).unwrap_or(SERVER_COST);
    refuse_cheap(cost, conf)?;
    let mut salt = [0_u8; 16];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut salt);
    let key = argon2id(passphrase.as_bytes(), &salt, cost).map_err(|err| Error::new(format!("passphrase: {err}")))?;
    let wrapped = moqui_vault::aead::seal(&key, AD, root.expose(), &mut OsRng);
    let Value::Object(metadata) = json!({
        "salt": encode(&salt),
        "m_kib": cost.m_kib,
        "t": cost.t,
        "p": cost.p,
        "wrapped_root": encode(&wrapped),
    }) else {
        unreachable!("a json! object literal is an object");
    };
    Ok(metadata)
}

/// The root key `passphrase` opens, or `None` when the passphrase is wrong.
///
/// # Errors
///
/// Metadata that is not this method's, or a cost under the server's on a production instance.
pub fn unwrap(metadata: &Map<String, Value>, passphrase: &str, conf: &SealConf) -> Result<Option<RootKey>, Error> {
    let number = |name: &str| {
        metadata
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| Error::new(format!("seal.json's passphrase method has no {name}")))
    };
    let cost = Argon2Params {
        m_kib: number("m_kib")?,
        t: number("t")?,
        p: number("p")?,
    };
    refuse_cheap(cost, conf)?;
    let salt = bytes_field(metadata, "salt")?;
    let key = argon2id(passphrase.as_bytes(), &salt, cost).map_err(|err| Error::new(format!("passphrase: {err}")))?;
    match moqui_vault::aead::open(&key, AD, &bytes_field(metadata, "wrapped_root")?) {
        Ok(root) => RootKey::from_slice(&root).map(Some),
        Err(_) => Ok(None),
    }
}
