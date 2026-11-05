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
