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

/// SEC-CLI-118: an update past the ceiling, as it comes or as announced, is dropped before it is all in memory.
#[test]
fn an_update_past_the_ceiling_is_dropped() {
    assert!(!too_large(0, None));
    assert!(!too_large(300 << 20, Some(300 << 20)));
    assert!(too_large(MAX_UPDATE_BYTES + 1, None));
    assert!(
        too_large(1, Some(MAX_UPDATE_BYTES + 1)),
        "an announced size past it stops at the first chunk"
    );
}

fn vars() -> FeedVars {
    FeedVars {
        version: "1.2.0+build.5".to_string(),
        target: "darwin".to_string(),
        arch: "aarch64".to_string(),
        bundle: "app".to_string(),
    }
}

/// SEC-CLI-118: the app reads the feed itself, so it puts the plugin's URL variables in as the plugin does (in the
/// path, where the parser encodes the braces, and in the query), and on HTTPS only.
#[test]
fn a_feeds_url_takes_the_plugins_variables_and_https_only() {
    let url = feed_url(
        "https://u.example/{{target}}/{{arch}}/{{current_version}}?b={{bundle_type}}",
        &vars(),
    )
    .unwrap();
    assert_eq!(url.as_str(), "https://u.example/darwin/aarch64/1.2.0%2Bbuild.5?b=app");
    assert_eq!(
        feed_url(" https://u.example/latest.json ", &vars()).unwrap().as_str(),
        "https://u.example/latest.json"
    );
    assert!(feed_url("http://u.example/{{target}}", &vars()).is_err());
    assert!(feed_url("file:///etc/feed.json", &vars()).is_err());
    assert!(feed_url("not a url", &vars()).is_err());
    assert_eq!(bundle_name(None), "unknown");
    assert_eq!(bundle_name(Some(tauri::utils::config::BundleType::Dmg)), "app");
    assert_eq!(
        bundle_name(Some(tauri::utils::config::BundleType::AppImage)),
        "appimage"
    );
}

/// SEC-CLI-118: a feed past the ceiling is refused, read to one byte more and no further.
#[test]
fn a_feed_past_the_ceiling_is_refused_and_not_read_whole() {
    assert_eq!(read_capped(&b"{}"[..], 2).unwrap(), b"{}");
    assert!(read_capped(&b"{} "[..], 2).is_err());
    let endless = std::io::repeat(b' ');
    let refused = read_capped(endless, MAX_FEED_BYTES).unwrap_err();
    assert!(refused.contains("256 KiB"), "{refused}");
    // check_feed reads the feed this way and hands it to restore_update, which binds the version to the signature
    let source = include_str!("updates.rs");
    let check = &source[source.find("async fn check_feed(").expect("check_feed")..];
    assert!(check.contains("fetch_feed(&endpoints, &vars)") && check.contains("updater.restore_update(feed)"));
    assert!(!source.contains("updater.check()"), "the plugin reads a feed whole");
}
