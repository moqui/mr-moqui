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

use moqui_core::edge::owner::Owner;
use serde_json::json;

use super::{
    bare_ids, key, key_ids, merge_config, number_owner, owner_in, owner_of, phone_key, realm_owner, trunk_of,
    RealmOwner,
};

#[test]
fn a_number_goes_to_the_server_first_then_the_copies_then_the_trunk() {
    let paired_trunk = Owner::paired("local-1");
    let holder = |node: &str| Some(node.to_string());
    assert_eq!(number_owner(true, holder("local-1"), paired_trunk.clone()), Owner::Own);
    assert_eq!(
        number_owner(false, holder("local-1"), Owner::Own),
        Owner::paired("local-1")
    );
    assert_eq!(number_owner(false, None, paired_trunk.clone()), paired_trunk.clone());
    assert_eq!(number_owner(false, holder(""), Owner::Own), Owner::Own);
    // E16: two pairings' numbers routed apart, whatever trunk they came on (a number both claim is held back as a
    // replica conflict, so only its first holder's copy is there)
    assert_eq!(
        number_owner(false, holder("local-2"), paired_trunk),
        Owner::paired("local-2")
    );
}

#[test]
fn phones_go_by_the_domain_they_register_to() {
    let pairings = vec![
        ("local-1".to_string(), "sip.one.example".to_string()),
        ("local-2".to_string(), "sip.two.example".to_string()),
    ];
    let paired = |node: &str| RealmOwner::Is(Owner::paired(node));
    // an all-in-one node: its own realm, then each pairing's domain
    assert_eq!(
        realm_owner("sip.own.example", "sip.own.example", false, &pairings),
        RealmOwner::Is(Owner::Own)
    );
    assert_eq!(
        realm_owner("SIP.One.example.", "sip.own.example", false, &pairings),
        paired("local-1")
    );
    assert_eq!(
        realm_owner("sip.two.example", "sip.own.example", false, &pairings),
        paired("local-2")
    );
    assert_eq!(
        realm_owner("sip.three.example", "sip.own.example", false, &pairings),
        RealmOwner::NoOne
    );
    // a dedicated edge node: its own realm is the one pairing's, and no one's with two
    assert_eq!(
        realm_owner("sip.edge.example", "sip.edge.example", true, &pairings[..1]),
        paired("local-1")
    );
    assert_eq!(
        realm_owner("sip.edge.example", "sip.edge.example", true, &pairings),
        RealmOwner::NoOne
    );
    assert_eq!(
        realm_owner("sip.two.example", "sip.edge.example", true, &pairings),
        paired("local-2")
    );
    // a domain two pairings name is refused
    let both = vec![
        ("local-1".to_string(), "sip.one.example".to_string()),
        ("local-2".to_string(), "sip.one.example".to_string()),
    ];
    assert!(matches!(
        realm_owner("sip.one.example", "sip.edge.example", true, &both),
        RealmOwner::Claimed(why) if why.contains("local-1, local-2")
    ));
}

#[test]
fn the_same_phone_username_in_two_domains_stays_apart() {
    let pairings = vec![
        ("local-1".to_string(), "sip.one.example".to_string()),
        ("local-2".to_string(), "sip.two.example".to_string()),
        ("local-3".to_string(), String::new()),
    ];
    let one = phone_key("desk", &Owner::paired("local-1"), &pairings);
    let two = phone_key("desk", &Owner::paired("local-2"), &pairings);
    assert_ne!(one, two);
    assert_eq!(one, moqui_sip::ua::aor_key("desk", Some("sip.one.example")));
    assert_eq!(phone_key("desk", &Owner::Own, &pairings), "desk");
    assert_eq!(
        phone_key("desk", &Owner::paired("local-3"), &pairings),
        "desk",
        "a pairing with no domain registers in the edge node's own realm"
    );
}

#[test]
fn a_paired_nodes_ids_are_keyed_and_given_back_bare() {
    let paired = Owner::paired("local-1");
    let leg = key(&paired, "100010");
    assert_ne!(leg, "100010");
    assert_eq!(owner_of(&leg), paired);
    assert_eq!(key(&Owner::Own, "100010"), "100010");
    assert_eq!(owner_of("100010"), Owner::Own);
    assert_eq!(key(&paired, ""), "");

    let mut answer = json!({"callSessionId": "100", "callLegId": "101", "reasonCode": "x",
        "ivr": {"callLegId": "102", "ivrMenuId": "M1"}, "list": [{"sipDeviceId": "D1"}]});
    key_ids("local-1", &mut answer);
    assert_eq!(owner_of(answer["callSessionId"].as_str().unwrap()), paired);
    assert_eq!(owner_of(answer["ivr"]["callLegId"].as_str().unwrap()), paired);
    assert_eq!(owner_of(answer["list"][0]["sipDeviceId"].as_str().unwrap()), paired);
    assert_eq!(
        answer["ivr"]["ivrMenuId"], "M1",
        "only the ids of calls, legs, trunks and phones are keyed"
    );
    assert_eq!(owner_in(&answer), paired);
    bare_ids(&mut answer);
    assert_eq!(answer["callSessionId"], "100");
    assert_eq!(answer["ivr"]["callLegId"], "102");
    assert_eq!(answer["list"][0]["sipDeviceId"], "D1");
    assert_eq!(owner_in(&answer), Owner::Own);
    assert_eq!(owner_in(&json!({"sourceAddress": "192.0.2.1"})), Owner::Own);
}

#[test]
fn the_trunks_of_both_sides_load_with_the_servers_policy() {
    let own = json!({"trunkList": [{"voiceTrunkId": "T1", "host": "own.example"}],
        "policy": {"maxConcurrentCalls": 5}, "blockedSourceList": [{"sourceAddress": "192.0.2.9"}]});
    let paired = json!({"trunkList": [{"voiceTrunkId": "T1", "host": "local.example"}],
        "policy": {"maxConcurrentCalls": 20}, "blockedSourceList": []});
    let merged = merge_config(Some(own), &[("local-1".to_string(), paired.clone())]);
    let trunks = merged["trunkList"].as_array().unwrap();
    assert_eq!(trunks.len(), 2, "one id on both sides is two trunks");
    assert_eq!(trunks[0]["voiceTrunkId"], "T1");
    assert_eq!(
        owner_of(trunks[1]["voiceTrunkId"].as_str().unwrap()),
        Owner::paired("local-1")
    );
    assert_eq!(trunks[1]["host"], "local.example");
    assert_eq!(merged["policy"]["maxConcurrentCalls"], 5);
    assert_eq!(merged["blockedSourceList"].as_array().unwrap().len(), 1);

    // a dedicated edge node: every pairing's trunks, each keyed by its node, with the first one's policy (E16)
    let second = json!({"trunkList": [{"voiceTrunkId": "T1", "host": "second.example"}],
        "policy": {"maxConcurrentCalls": 9}});
    let dedicated = merge_config(
        None,
        &[("local-1".to_string(), paired), ("local-2".to_string(), second)],
    );
    let trunks = dedicated["trunkList"].as_array().unwrap();
    assert_eq!(trunks.len(), 2, "one id in two pairings is two trunks");
    assert_eq!(
        owner_of(trunks[0]["voiceTrunkId"].as_str().unwrap()),
        Owner::paired("local-1")
    );
    assert_eq!(
        owner_of(trunks[1]["voiceTrunkId"].as_str().unwrap()),
        Owner::paired("local-2")
    );
    assert_eq!(trunks[1]["host"], "second.example");
    assert_eq!(dedicated["policy"]["maxConcurrentCalls"], 20);
    // a normal node: its own alone
    let normal = merge_config(Some(json!({"trunkList": [], "policy": {}})), &[]);
    assert!(normal["trunkList"].as_array().unwrap().is_empty());
    assert!(normal["blockedSourceList"].as_array().unwrap().is_empty());
}

#[test]
fn an_id_from_a_link_never_names_another_nodes_item() {
    // a paired node's id that holds the key's separator stays under that node
    let mut control = json!({"callSessionId": "local-2\u{1f}100", "callLegId": "\u{1f}101",
        "voiceTrunkId": "local-1\u{1f}T1"});
    key_ids("local-1", &mut control);
    for field in ["callSessionId", "callLegId", "voiceTrunkId"] {
        assert_eq!(
            owner_of(control[field].as_str().unwrap()),
            Owner::paired("local-1"),
            "{field}"
        );
    }
    assert_eq!(owner_in(&control), Owner::paired("local-1"));
    bare_ids(&mut control);
    assert_eq!(
        control["callSessionId"], "local-2\u{1f}100",
        "given back as the node sent it"
    );
}

#[test]
fn a_trunk_id_goes_only_to_its_own_side() {
    let paired = Owner::paired("local-1");
    let paired_trunk = key(&paired, "T1");
    assert_eq!(trunk_of(&paired, &paired_trunk), Some("T1"));
    assert_eq!(
        trunk_of(&Owner::Own, &paired_trunk),
        None,
        "the server's number on the paired node's trunk"
    );
    assert_eq!(trunk_of(&Owner::Own, "T9"), Some("T9"));
    assert_eq!(
        trunk_of(&paired, "T9"),
        None,
        "the paired node's number on the server's trunk"
    );
    assert_eq!(trunk_of(&Owner::paired("local-2"), &paired_trunk), None);
    assert_eq!(trunk_of(&Owner::Own, ""), None);
}
