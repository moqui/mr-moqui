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

/// Only a device's key records a memory, for at most the setting's time; the record is checked, listed and ended by its
/// own user only, and one for a replaced personal key is no longer good.
#[test]
fn a_device_memory_is_recorded_by_a_device_key_and_ended_by_its_user() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let _alice = Client::new(&facade, "alice");
    let _bob = Client::new(&facade, "bob");
    let remember = json!({ "deviceMemoryId": "0123456789abcdef", "deviceName": "Alice's phone", "minutes": 60 * 48 });

    let refused = call_as("alice", "remember#VaultDeviceMemory", remember.clone()).unwrap_err();
    assert!(refused.contains("desktop or mobile app"), "{refused}");

    device_login("alice");
    let before = moqui_core::user::now_millis();
    let made = call("remember#VaultDeviceMemory", remember.clone()).unwrap();
    let thru = made["thruStamp"].as_i64().unwrap();
    assert!(
        thru <= before + 8 * 3_600_000 + 60_000,
        "capped at device-memory-max-hours"
    );
    assert!(
        call("remember#VaultDeviceMemory", remember).is_err(),
        "an id is taken once"
    );
    let check = json!({ "deviceMemoryId": "0123456789abcdef" });
    assert_eq!(
        call("check#VaultDeviceMemory", check.clone()).unwrap()["live"],
        json!(true)
    );
    let listed = call("list#VaultDeviceMemories", json!({})).unwrap();
    assert_eq!(listed["deviceMemoryList"][0]["deviceName"], json!("Alice's phone"));

    // another user neither sees nor ends it
    assert_eq!(
        call_as("bob", "check#VaultDeviceMemory", check.clone()).unwrap()["live"],
        json!(false)
    );
    assert_eq!(
        call_as("bob", "forget#VaultDeviceMemory", check.clone()).unwrap()["ended"],
        json!(false)
    );
    assert_eq!(
        call_as("bob", "list#VaultDeviceMemories", json!({})).unwrap()["deviceMemoryList"],
        json!([])
    );

    // Alice takes the device off her list: it is no longer good
    assert_eq!(
        call_as("alice", "forget#VaultDeviceMemory", check.clone()).unwrap()["ended"],
        json!(true)
    );
    assert_eq!(call("check#VaultDeviceMemory", check).unwrap()["live"], json!(false));
    assert_eq!(
        call("list#VaultDeviceMemories", json!({})).unwrap()["deviceMemoryList"],
        json!([])
    );
}
