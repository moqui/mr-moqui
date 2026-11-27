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

//! The bootstrap bundle (plans/archive/Vault-Instances.md §4.5): the secrets a new instance needs before its database opens,
//! by their secret names (`entity-facade/datasource[transactional]@password`), sealed by HPKE (RFC 9180, the vault
//! crate's) to the bundle key the instance pinned at pairing, with its instance id as associated data, so a bundle
//! made for one instance does not open at another. The instance keeps them with the seal's bootstrap API in
//! `seal.json` (Sealed-Secrets S4), never in `.env`.

use std::collections::BTreeMap;

use moqui_vault::hpke;
use zeroize::Zeroizing;

/// The HPKE `info` of a bundle.
const INFO: &[u8] = b"moqui-manage bootstrap-bundle/1";

/// The secrets of a bundle, by name. Zeroized when dropped.
pub type Values = BTreeMap<String, Zeroizing<String>>;

/// Seal `values` for `instance_id` to its bundle key `to`: the manager's (the `instances` feature).
#[cfg_attr(not(feature = "instances"), allow(dead_code))]
pub(crate) fn seal(to: &[u8], instance_id: &str, values: &Values) -> Result<Vec<u8>, String> {
    let public = hpke::PublicKey::from_bytes(to).map_err(|err| format!("the instance's bundle key: {err}"))?;
    // borrowed, so the one copy of the values is the zeroized text
    let map: BTreeMap<&str, &str> = values
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let plain = Zeroizing::new(serde_json::to_string(&map).map_err(|err| err.to_string())?);
    hpke::seal_raw(
        &public,
        INFO,
        instance_id.as_bytes(),
        plain.as_bytes(),
        &mut moqui_vault::rng::OsRng,
    )
    .map_err(|err| format!("the bundle was not sealed: {err}"))
}

/// Open a bundle sealed for `instance_id` with the bundle key `secret`.
pub(crate) fn open(secret: &hpke::SecretKey, instance_id: &str, sealed: &[u8]) -> Result<Values, String> {
    let plain = hpke::open_raw(secret, INFO, instance_id.as_bytes(), sealed)
        .map_err(|_| "the bootstrap bundle does not open with this instance's key".to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&plain).map_err(|_| "the bootstrap bundle is not a bundle".to_string())?;
    let serde_json::Value::Object(map) = parsed else {
        return Err("the bootstrap bundle is not a bundle".to_string());
    };
    Ok(map
        .into_iter()
        .filter_map(|(name, value)| match value {
            serde_json::Value::String(text) => Some((name, Zeroizing::new(text))),
            _ => None,
        })
        .collect())
}
