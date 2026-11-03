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
fn feed_needs_a_pubkey_and_https_endpoints() {
    let feed = json!({ "pubkey": "dW50cnVzdGVk", "endpoints": ["https://example.com/{{target}}/{{current_version}}"] });
    assert!(has_feed(Some(&feed)));
    let two = json!({ "pubkey": "dW50cnVzdGVk", "endpoints": ["https://a.example/u", "https://b.example/u"] });
    assert!(has_feed(Some(&two)));

    assert!(!has_feed(None));
    assert!(!has_feed(Some(&json!(null))));
    assert!(!has_feed(Some(&json!({ "endpoints": ["https://example.com/u"] }))));
    assert!(!has_feed(Some(
        &json!({ "pubkey": " ", "endpoints": ["https://example.com/u"] })
    )));
    assert!(!has_feed(Some(&json!({ "pubkey": "dW50cnVzdGVk" }))));
    assert!(!has_feed(Some(&json!({ "pubkey": "dW50cnVzdGVk", "endpoints": [] }))));
    // The plugin refuses http in a release build: the app would not start.
    let http = json!({ "pubkey": "dW50cnVzdGVk", "endpoints": ["https://a.example/u", "http://b.example/u"] });
    assert!(!has_feed(Some(&http)));
    assert!(!has_feed(Some(&json!({ "pubkey": "dW50cnVzdGVk", "endpoints": [1] }))));
}

#[test]
fn item_follows_the_phase() {
    assert_eq!(Phase::Idle.text(), "Check for updates…");
    assert!(Phase::Idle.enabled());
    assert!(!Phase::Checking.enabled());
    let available = Phase::Available("0.2.0".to_string());
    assert_eq!(available.text(), "Install update 0.2.0…");
    assert!(available.enabled());
    let installing = Phase::Installing("0.2.0".to_string());
    assert_eq!(installing.text(), "Installing update 0.2.0…");
    assert!(!installing.enabled());
}

#[test]
fn notification_names_the_version_and_the_item() {
    assert_eq!(available_title("0.2.0"), "MR Moqui 0.2.0 is available");
    let body = available_body("0.2.0");
    assert!(body.contains("\"Install update 0.2.0…\""), "{body}");
    assert!(body.contains("restarts"), "{body}");
}
