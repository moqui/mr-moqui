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

use super::*;

#[test]
fn a_route_is_found_by_any_spelling_of_its_name() {
    let hosts = Hosts::default();
    write(&hosts.table).routes.insert(
        "erp.example.com".to_string(),
        Route {
            node: "local-1".to_string(),
            mode: Mode::Terminate,
            sticky_cookie: None,
            member: None,
        },
    );
    let route = hosts.route("ERP.example.com:443").unwrap();
    assert_eq!(route.node, "local-1");
    assert_eq!(route.mode, Mode::Terminate);
    assert!(hosts.route("other.example.com").is_none());
    assert!(hosts.route("").is_none());
    assert_eq!(Mode::from_enum(HOST_PASSTHROUGH), Some(Mode::Passthrough));
    assert_eq!(Mode::from_enum(HOST_LINK_NAME), Some(Mode::Passthrough));
    assert_eq!(Mode::from_enum("EdgeHostOther"), None);
}

#[test]
fn a_pairing_s_chat_paths_are_answered_here_only_for_its_own_bridges() {
    let hosts = Hosts::default();
    write(&hosts.table)
        .chat
        .insert("local-1".to_string(), vec!["B1".to_string()]);
    assert!(hosts.answers_here("local-1", "/channels/B1/webhook"));
    assert!(hosts.answers_here("local-1", "/channels/B1/media/tok"));
    assert!(
        !hosts.answers_here("local-1", "/channels/B2/webhook"),
        "a bridge it routes elsewhere"
    );
    assert!(
        !hosts.answers_here("local-2", "/channels/B1/webhook"),
        "another pairing's"
    );
    assert!(!hosts.answers_here("local-1", "/channels/B1/other"));
    assert!(!hosts.answers_here("local-1", "/apps/channels/B1/webhook"));
    assert!(!hosts.answers_here("local-1", "/channels//webhook"));
}

/// What the active pairings' setups give the table: chat bridges only while this node takes webhooks, and each PASS
/// site's one pairing; a site two pairings name is no one's (logged), so its calls are refused rather than sent to either.
#[test]
fn a_pass_site_has_one_pairing_and_chat_paths_need_webhooks_on() {
    let setups = vec![
        (
            "local-a".to_string(),
            serde_json::json!({"chatBridgeIds": ["B1"], "passSites": ["SITE_A", "BOTH"]}),
        ),
        (
            "local-b".to_string(),
            serde_json::json!({"passSites": ["site_b", "both"]}),
        ),
        ("local-c".to_string(), serde_json::json!({})),
    ];
    let loaded = setup_maps(setups.clone(), true);
    assert_eq!(loaded.chat["local-a"], ["B1"]);
    let hosts = Hosts::default();
    *write(&hosts.table) = loaded;
    assert_eq!(hosts.active(), ["local-a", "local-b", "local-c"]);
    assert_eq!(hosts.site_node("site_a").unwrap().as_deref(), Some("local-a"));
    assert_eq!(hosts.site_node("SITE_B").unwrap().as_deref(), Some("local-b"));
    assert_eq!(hosts.site_node("other").unwrap(), None);
    assert!(hosts.site_node("Both").is_err(), "two pairings name it");
    assert!(hosts.answers_here("local-a", "/channels/B1/webhook"));
    // webhooks off (or no channels feature): every path of the host goes down the link as before
    *write(&hosts.table) = setup_maps(setups, false);
    assert!(!hosts.answers_here("local-a", "/channels/B1/webhook"));
}
