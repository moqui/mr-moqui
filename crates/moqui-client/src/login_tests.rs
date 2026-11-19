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

use serde_json::json;

use super::*;

#[test]
fn the_current_device_row_gives_the_key_period_as_millis_or_text() {
    let list = json!({"deviceList": [
        {"deviceName": "Phone", "current": false, "fromDate": 1, "thruDate": 2},
        {"deviceName": "Laptop", "current": true, "fromDate": "1000", "thruDate": 4000},
    ]});
    assert_eq!(current_period(&list), Some((1000, 4000)));
    assert_eq!(current_period(&json!({"deviceList": []})), None);
}

#[test]
fn a_key_is_renewed_when_less_than_a_third_of_its_life_is_left() {
    assert!(!renew_due(0, 90, 59));
    assert!(renew_due(0, 90, 61));
    assert!(
        renew_due(0, 90, 100),
        "an expired key is due too, though the server will refuse it"
    );
}

#[test]
fn a_server_with_no_client_api_is_the_first() {
    assert_eq!(server_client_api(&json!({"server": {"name": "x"}})), 1);
    assert_eq!(server_client_api(&json!({"server": {"clientApi": 3}})), 3);
}

#[test]
fn this_client_knows_the_sync_contracts() {
    // 4: the changes feeds, clientRef on more mutations, conflicts and chunked uploads (MD5)
    assert_eq!(CLIENT_API, 4);
    assert!(server_client_api(&json!({"server": {"clientApi": 4}})) <= CLIENT_API);
    assert!(server_client_api(&json!({"server": {"clientApi": 5}})) > CLIENT_API);
}

#[test]
fn the_server_sync_is_read_from_the_client_config() {
    let md4 = ServerSync::of(&json!({"server": {"clientApi": 3, "features": {"internalSendRef": true}}}));
    assert!(!md4.syncs() && !md4.internal_send());
    let basic = ServerSync::of(&json!({"server": {"clientApi": 4, "features": {}}}));
    assert!(basic.syncs() && !basic.internal_send() && basic.mail_uncovered.is_empty());
    // at the top level is not where the server puts them
    assert!(
        !ServerSync::of(&json!({"server": {"clientApi": 4}, "features": {"internalSendRef": true}})).internal_send()
    );
    let boss = ServerSync::of(&json!({"server": {"clientApi": 4,
        "features": {"internalSendRef": true, "mailChangesUncovered": ["internal"], "nativePush": {}}}}));
    assert!(boss.internal_send());
    assert_eq!(boss.mail_uncovered, vec!["internal".to_string()]);
    assert_eq!(ServerSync::default().client_api, 0);
}

/// SEC-CLI-108: a key the server did not hear signed out is revoked at a later pass: one the server answers goes, one
/// it does not stays and is tried again only after five minutes, and one past a device key's life goes untried.
#[test]
fn a_key_signed_out_unheard_is_revoked_at_a_later_pass() {
    use std::cell::RefCell;
    let dir = std::env::temp_dir().join(format!("moqui-client-revoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = crate::keys::Keys::new(Box::new(crate::keys::FileKeyStore::new(&dir)), "file");
    let now = 100 * 24 * 60 * 60 * 1000;
    keys.add_pending_revoke("https://up.example.com", "up", now - 1_000).unwrap();
    keys.add_pending_revoke("https://down.example.com", "down", now - 1_000).unwrap();
    keys.add_pending_revoke("https://old.example.com", "old", now - crate::keys::PENDING_MILLIS - 1).unwrap();
    let tried = RefCell::new(Vec::new());
    let revoke = |server: &str, key: &str| -> Result<(), Error> {
        tried.borrow_mut().push(key.to_string());
        if server.contains("down") {
            Err(Error::new("unreachable"))
        } else {
            Ok(())
        }
    };
    assert_eq!(revoke_pending(&keys, now, &revoke), 1);
    assert_eq!(tried.borrow().clone(), ["up", "down"]);
    let left = keys.pending_revokes();
    assert_eq!(left.iter().map(|entry| entry.key.as_str()).collect::<Vec<_>>(), ["down"]);
    assert_eq!(left[0].tried, now);
    // a minute later it waits; five minutes later it is tried, and answered, it goes
    tried.borrow_mut().clear();
    assert_eq!(revoke_pending(&keys, now + 60_000, &revoke), 0);
    assert!(tried.borrow().is_empty());
    let revoke_up = |_: &str, _: &str| -> Result<(), Error> { Ok(()) };
    assert_eq!(revoke_pending(&keys, now + 5 * 60_000, &revoke_up), 1);
    assert!(keys.pending_revokes().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
