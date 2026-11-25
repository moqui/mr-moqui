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

//! `moqui vault shares` and `moqui vault seal-share`: a recovery holder's steps for a managed instance
//! (plans/archive/Vault-Instances.md §4.5, M6), against the manager's server. A share never goes to the server in the clear.
//!
//! - `shares --vault <id>`: each recovery share an instance delivered to this user, checked against the instance's
//!   link key as the manager pinned it, filed into one of this user's personal vaults with that link key's fingerprint
//!   (the holder's own anchor of the instance from then on), and confirmed with this user's signing key (the instance
//!   checks the confirmation itself).
//! - `seal-share <appInstanceId> --vault <id>`: the share filed for that instance, sealed to the key the sealed instance
//!   made for this unseal, once that key's signature checks against the link key fingerprint filed with the share (a
//!   key the manager makes in the instance's name is refused) and it has not expired; the manager relays the
//!   ciphertext.

use moqui_lib::error::Error;
use moqui_vault::managed::{self, UnsealKey};
use moqui_vault::text::unhex;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::client::VaultClient;
use super::remote::Remote;
use super::{open_key, open_shared, segment, shown, text, vault_err};

/// Opens a personal vault of this user's (shared-key) in `client`; its key version.
fn open_personal(remote: &mut Remote, client: &mut VaultClient, vault_id: &str) -> Result<u64, Error> {
    let vault = remote.get(&format!("/vaults/{}", segment(vault_id)), &[])?;
    if text(&vault["vault"], "vaultKindEnumId") == "VkOperational" {
        return Err(Error::new("A recovery share goes into one of your personal vaults"));
    }
    if text(&vault["vault"], "lockEnumId") == "VlSingleSecret" {
        return Err(Error::new(
            "File recovery shares into a personal vault your key opens (shared-key)",
        ));
    }
    open_shared(remote, client, vault_id, &vault)?;
    Ok(vault["vault"]["keyVersion"].as_u64().unwrap_or(0))
}

/// `shares [--vault ID]`.
pub(super) fn shares(remote: &mut Remote, client: &mut VaultClient, vault_id: &str) -> Result<(), Error> {
    open_key(remote, client)?;
    let key_version = if vault_id.is_empty() {
        None
    } else {
        Some(open_personal(remote, client, vault_id)?)
    };
    let deliveries = remote.get("/deliveries", &[])?;
    let mut count = 0;
    for listed in deliveries["deliveryList"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let delivery = &listed["delivery"];
        if text(delivery, "sender_kind") != "instance" || text(delivery, "purpose") != managed::SHARE_PURPOSE {
            continue;
        }
        let delivery_id = text(listed, "deliveryId");
        let opened = client.call(
            "instance_share_open",
            json!({ "delivery": delivery, "link_public": listed["senderPublic"],
                    "link_fingerprint": listed["senderFingerprint"] }),
        );
        let opened = match opened {
            Ok(opened) => opened,
            Err(err) => {
                eprintln!("{}: refused: {}", shown(&delivery_id), shown(&err));
                continue;
            }
        };
        let name = text(&opened, "item_name");
        match key_version {
            Some(key_version) => file_share(remote, client, vault_id, key_version, &opened)?,
            None => {
                println!("{}:", shown(&name));
                let value: Value = serde_json::from_str(opened["value"].as_str().unwrap_or("")).unwrap_or(Value::Null);
                println!("  share:    {}", text(&value, "share"));
                println!("  link key: {}", text(&opened, "link_fingerprint"));
                println!(
                    "  Keep both where only you can reach them; with --vault they are filed in your vault instead."
                );
            }
        }
        remote.post(&format!("/deliveries/{}/file", segment(&delivery_id)), &json!({}))?;
        remote.post(
            &format!("/deliveries/{}/confirmShare", segment(&delivery_id)),
            &json!({ "confirmation": opened["confirmation"] }),
        )?;
        println!("{}: filed and confirmed", shown(&name));
        count += 1;
    }
    if count == 0 {
        println!("No recovery share from an instance waits for you.");
    }
    Ok(())
}

/// One opened share as a recovery-share item of the personal vault `vault_id`, with the link key fingerprint it was
/// checked against.
fn file_share(
    remote: &mut Remote,
    client: &mut VaultClient,
    vault_id: &str,
    key_version: u64,
    opened: &Value,
) -> Result<(), Error> {
    let value: Value = serde_json::from_str(opened["value"].as_str().unwrap_or(""))
        .map_err(|_| Error::new("the share's value is not JSON"))?;
    let name = text(opened, "item_name");
    let body = Zeroizing::new(
        json!({
            "v": 1, "type": "recovery-share", "name": name,
            "fields": {
                "instance": text(&value, "instance"),
                "shareIndex": format!("{} of {}", text(&value, "index"), text(&value, "of")),
                "sharesNeeded": text(&value, "threshold"),
                "share": text(&value, "share"),
                "linkKeyFingerprint": text(opened, "link_fingerprint"),
            },
        })
        .to_string(),
    );
    let item_id = super::new_id("item");
    let sealed = client
        .call(
            "item_seal",
            json!({ "vault_id": vault_id, "item_id": item_id, "key_version": key_version, "body": body.as_str() }),
        )
        .map_err(vault_err)?;
    remote.post(
        &format!("/vaults/{}/items", segment(vault_id)),
        &json!({ "vaultItemId": item_id, "keyVersion": key_version, "wrappedItemKey": sealed["wrapped_item_key"],
                 "bodyCipher": sealed["sealed_body"] }),
    )?;
    Ok(())
}

/// The share and the link key fingerprint this holder filed for `instance_id` in the personal vault `vault_id`.
fn filed_share(
    remote: &mut Remote,
    client: &mut VaultClient,
    vault_id: &str,
    instance_id: &str,
) -> Result<(Zeroizing<String>, String), Error> {
    open_personal(remote, client, vault_id)?;
    let items = remote.get(&format!("/vaults/{}/items", segment(vault_id)), &[])?;
    for item in items["itemList"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let opened = client.call(
            "item_open",
            json!({ "vault_id": vault_id, "item_id": text(item, "vaultItemId"), "key_version": item["keyVersion"],
                    "label": Value::Null, "wrapped_item_key": item["wrappedItemKey"], "sealed_body": item["bodyCipher"] }),
        );
        let Ok(opened) = opened else { continue };
        let mut body: Value = serde_json::from_str(opened["body"].as_str().unwrap_or("")).unwrap_or(Value::Null);
        let fields = &body["fields"];
        if text(&body, "type") != "recovery-share" || text(fields, "instance") != instance_id {
            continue;
        }
        let fingerprint = text(fields, "linkKeyFingerprint");
        let share = match body["fields"].get_mut("share").map(Value::take) {
            Some(Value::String(share)) => Zeroizing::new(share),
            _ => continue,
        };
        if fingerprint.is_empty() {
            return Err(Error::new(format!(
                "The recovery share of {} you filed has no link key fingerprint: run `moqui vault shares` again, or \
                 enter it on the instance's operator socket",
                shown(instance_id)
            )));
        }
        return Ok((share, fingerprint));
    }
    Err(Error::new(format!(
        "No recovery share of {} is filed in vault {}",
        shown(instance_id),
        shown(vault_id)
    )))
}

/// The unseal key the manager relays for `instance_id`, checked against the link key fingerprint `filed` and the
/// clock: a key the manager made in the instance's name, or an expired one, is refused.
pub(super) fn checked_unseal_key(
    instance_id: &str,
    offered: &Value,
    filed: &str,
    now: i64,
) -> Result<UnsealKey, Error> {
    let bytes = |name: &str| unhex(&text(offered, name)).map_err(|_| Error::new(format!("{name} is not hex")));
    let unseal_public: [u8; 32] = bytes("unsealPublic")?
        .try_into()
        .map_err(|_| Error::new("the unseal key is not an X25519 key"))?;
    let key = UnsealKey {
        instance_id: instance_id.to_string(),
        unseal_public,
        expires: offered["expires"].as_i64().unwrap_or(0),
        signature: bytes("signature")?,
    };
    key.verify(&bytes("linkPublic")?, filed, now).map_err(|err| {
        Error::new(format!(
            "the instance's unseal key is refused ({err}): it is not signed by the link key you filed with your share"
        ))
    })?;
    Ok(key)
}

/// `seal-share <appInstanceId> --vault ID`.
pub(super) fn seal_share(
    remote: &mut Remote,
    client: &mut VaultClient,
    instance_id: &str,
    vault_id: &str,
) -> Result<(), Error> {
    if vault_id.is_empty() {
        return Err(Error::new(
            "seal-share takes --vault: the personal vault your recovery share of the instance is filed in",
        ));
    }
    open_key(remote, client)?;
    let (share, filed) = filed_share(remote, client, vault_id, instance_id)?;
    let offered = remote.get(&format!("/managedInstances/{}/unsealKey", segment(instance_id)), &[])?;
    if offered["waiting"] != json!(true) {
        return Err(Error::new(format!(
            "AppInstance {} asks for no recovery shares now (it is unsealed, or its link is down)",
            shown(instance_id)
        )));
    }
    let key = checked_unseal_key(instance_id, &offered, &filed, moqui_core::user::now_millis())?;
    let sealed = managed::seal_share(&key, share.trim(), &mut moqui_vault::rng::OsRng)
        .map_err(|err| Error::new(format!("the share was not sealed: {err}")))?;
    remote.post(
        &format!("/managedInstances/{}/sealedShares", segment(instance_id)),
        &json!({ "unsealPublic": moqui_vault::text::hex(&key.unseal_public), "sealed": moqui_vault::text::hex(&sealed) }),
    )?;
    println!(
        "Your share is sealed to this unseal of {} (its link key {}); the manager relays it and cannot open it.",
        shown(instance_id),
        shown(&filed)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_share_takes_an_unseal_key_only_by_the_link_key_the_holder_filed() {
        use moqui_vault::sign::SigningKey;
        let instance = SigningKey::from_seed(&moqui_vault::Key32::from_bytes([1; 32]));
        let manager = SigningKey::from_seed(&moqui_vault::Key32::from_bytes([2; 32]));
        let filed = managed::link_fingerprint(instance.verifying_key().as_bytes());
        let unseal = [7_u8; 32];
        let offered = |by: &SigningKey, expires: i64| {
            let key = UnsealKey::sign(by, "prod-1", &unseal, expires);
            json!({ "waiting": true, "unsealPublic": moqui_vault::text::hex(&unseal),
                    "signature": moqui_vault::text::hex(&key.signature),
                    "linkPublic": moqui_vault::text::hex(by.verifying_key().as_bytes()), "expires": expires })
        };
        assert!(checked_unseal_key("prod-1", &offered(&instance, 2_000), &filed, 1_000).is_ok());
        // the manager's own key, with its own link key, as a compromised manager would hand it over
        assert!(checked_unseal_key("prod-1", &offered(&manager, 2_000), &filed, 1_000).is_err());
        // expired, or for another instance
        assert!(checked_unseal_key("prod-1", &offered(&instance, 900), &filed, 1_000).is_err());
        assert!(checked_unseal_key("prod-2", &offered(&instance, 2_000), &filed, 1_000).is_err());
    }
}
