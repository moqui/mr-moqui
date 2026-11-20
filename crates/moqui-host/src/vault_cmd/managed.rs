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
//! (plans/Vault-Instances.md §4.5, M6), against the manager's server. A share never goes to the server in the clear.
//!
//! - `shares --vault <id>`: each recovery share an instance delivered to this user, checked against the instance's
//!   link key as the manager pinned it, filed into one of this user's personal vaults, and confirmed with this user's
//!   signing key (the instance checks the confirmation itself).
//! - `seal-share <appInstanceId>`: the share, entered here, sealed to the key the sealed instance made for this unseal,
//!   once that key's signature by the instance's pinned link key checks; the manager relays the ciphertext.

use moqui_lib::error::Error;
use moqui_vault::managed::{self, UnsealKey};
use moqui_vault::text::unhex;
use serde_json::{json, Value};

use super::client::VaultClient;
use super::remote::Remote;
use super::{ask, open_key, open_shared, segment, shown, text, vault_err};

/// `shares [--vault ID]`.
pub(super) fn shares(remote: &mut Remote, client: &mut VaultClient, vault_id: &str) -> Result<(), Error> {
    open_key(remote, client)?;
    let key_version = if vault_id.is_empty() {
        None
    } else {
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
        Some(vault["vault"]["keyVersion"].as_u64().unwrap_or(0))
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
                println!("  {}", text(&value, "share"));
                println!("  Keep it where only you can reach it; with --vault it is filed in your vault instead.");
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

/// One opened share as a recovery-share item of the personal vault `vault_id`.
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
    let body = json!({
        "v": 1, "type": "recovery-share", "name": name,
        "fields": {
            "instance": text(&value, "instance"),
            "shareIndex": format!("{} of {}", text(&value, "index"), text(&value, "of")),
            "sharesNeeded": text(&value, "threshold"),
            "share": text(&value, "share"),
        },
    })
    .to_string();
    let item_id = super::new_id("item");
    let sealed = client
        .call(
            "item_seal",
            json!({ "vault_id": vault_id, "item_id": item_id, "key_version": key_version, "body": body }),
        )
        .map_err(vault_err)?;
    remote.post(
        &format!("/vaults/{}/items", segment(vault_id)),
        &json!({ "vaultItemId": item_id, "keyVersion": key_version, "wrappedItemKey": sealed["wrapped_item_key"],
                 "bodyCipher": sealed["sealed_body"] }),
    )?;
    Ok(())
}

/// `seal-share <appInstanceId>`.
pub(super) fn seal_share(remote: &mut Remote, instance_id: &str) -> Result<(), Error> {
    let key = remote.get(&format!("/managedInstances/{}/unsealKey", segment(instance_id)), &[])?;
    if key["waiting"] != json!(true) {
        return Err(Error::new(format!(
            "AppInstance {} asks for no recovery shares now (it is unsealed, or its link is down)",
            shown(instance_id)
        )));
    }
    let bytes = |name: &str| unhex(&text(&key, name)).map_err(|_| Error::new(format!("{name} is not hex")));
    let unseal_public: [u8; 32] = bytes("unsealPublic")?
        .try_into()
        .map_err(|_| Error::new("the unseal key is not an X25519 key"))?;
    let unseal = UnsealKey {
        instance_id: instance_id.to_string(),
        unseal_public,
        signature: bytes("signature")?,
    };
    // the instance's own key for this unseal, by its link key as the manager pinned it: a key of the manager's own
    // in its place is refused here
    unseal
        .verify(&bytes("linkPublic")?, &text(&key, "linkKeyFingerprint"))
        .map_err(|err| Error::new(format!("the instance's unseal key is refused: {err}")))?;
    println!(
        "AppInstance {} asks for recovery shares; its link key is {}",
        shown(instance_id),
        shown(&text(&key, "linkKeyFingerprint"))
    );
    let share = ask("Your recovery share", true)?;
    let sealed = managed::seal_share(&unseal, share.trim(), &mut moqui_vault::rng::OsRng)
        .map_err(|err| Error::new(format!("the share was not sealed: {err}")))?;
    remote.post(
        &format!("/managedInstances/{}/sealedShares", segment(instance_id)),
        &json!({ "unsealPublic": moqui_vault::text::hex(&unseal_public), "sealed": moqui_vault::text::hex(&sealed) }),
    )?;
    println!("Your share is sealed to this unseal of the instance; the manager relays it and cannot open it.");
    Ok(())
}
