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

//! The record of a device that remembers the personal key (§3.5, V5).

use serde_json::json;

use super::{call, call_as, setup, sys, Client};

/// Signs in as `user` by a device's key, as the desktop and mobile apps do.
fn device_login(user: &str) {
    sys(|| {
        let key = moqui_core::user::issue_device_key(user, "Alice's phone").unwrap();
        moqui_core::user::logout_local().unwrap();
        assert!(moqui_core::user::apply_header_keys(None, Some(&key), None).unwrap());
    });
}

/// Only a device's key records a memory, for the caller's active personal key and at most the setting's time, and only
/// a device's key reads its secret, while the record is live; the record is checked, listed and ended by its own user
/// only, and a record ended, or one for a replaced personal key, hands out no secret.
#[test]
fn a_device_memory_is_recorded_by_a_device_key_and_ended_by_its_user() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let alice = Client::new(&facade, "alice");
    let _bob = Client::new(&facade, "bob");
    let remember = json!({ "deviceMemoryId": "0123456789abcdef", "personalKeyId": alice.key_id,
                           "deviceName": "Alice's phone", "minutes": 60 * 48 });
    let check = json!({ "deviceMemoryId": "0123456789abcdef" });

    let refused = call_as("alice", "remember#VaultDeviceMemory", remember.clone()).unwrap_err();
    assert!(refused.contains("desktop or mobile app"), "{refused}");

    device_login("alice");
    let mut wrong_key = remember.clone();
    wrong_key["personalKeyId"] = json!("pk-someone-else");
    assert!(
        call("remember#VaultDeviceMemory", wrong_key).is_err(),
        "another personal key"
    );
    let before = moqui_core::user::now_millis();
    let made = call("remember#VaultDeviceMemory", remember.clone()).unwrap();
    let thru = made["thruStamp"].as_i64().unwrap();
    assert!(
        thru <= before + 8 * 3_600_000 + 60_000,
        "capped at device-memory-max-hours"
    );
    let secret = made["memorySecret"].as_str().unwrap().to_string();
    assert_eq!(secret.len(), 64);
    assert!(
        call("remember#VaultDeviceMemory", remember).is_err(),
        "an id is taken once"
    );
    let checked = call("check#VaultDeviceMemory", check.clone()).unwrap();
    assert_eq!(checked["live"], json!(true));
    assert_eq!(checked["memorySecret"], json!(secret));
    let listed = call("list#VaultDeviceMemories", json!({})).unwrap();
    assert_eq!(listed["deviceMemoryList"][0]["deviceName"], json!("Alice's phone"));
    assert!(listed["deviceMemoryList"][0].get("memorySecret").is_none());

    // a session that is not a device's key gets no secret
    let refused = call_as("alice", "check#VaultDeviceMemory", check.clone()).unwrap_err();
    assert!(refused.contains("desktop or mobile app"), "{refused}");

    // another user neither sees, reads nor ends it
    device_login("bob");
    let theirs = call("check#VaultDeviceMemory", check.clone()).unwrap();
    assert_eq!(theirs["live"], json!(false));
    assert!(theirs.get("memorySecret").is_none());
    assert_eq!(
        call("forget#VaultDeviceMemory", check.clone()).unwrap()["ended"],
        json!(false)
    );
    assert_eq!(
        call("list#VaultDeviceMemories", json!({})).unwrap()["deviceMemoryList"],
        json!([])
    );

    // Alice takes the device off her list: it is no longer good, and its secret is gone
    assert_eq!(
        call_as("alice", "forget#VaultDeviceMemory", check.clone()).unwrap()["ended"],
        json!(true)
    );
    device_login("alice");
    let ended = call("check#VaultDeviceMemory", check).unwrap();
    assert_eq!(ended["live"], json!(false));
    assert!(ended.get("memorySecret").is_none());
    assert_eq!(
        call("list#VaultDeviceMemories", json!({})).unwrap()["deviceMemoryList"],
        json!([])
    );
}
