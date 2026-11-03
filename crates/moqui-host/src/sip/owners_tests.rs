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

use super::{bare_ids, domain_of, key, key_ids, merge_config, number_owner, owner_in, owner_of, trunk_of};

#[test]
fn a_number_goes_to_the_server_first_then_the_copies_then_the_trunk() {
    let paired_trunk = Owner::paired("local-1");
    assert_eq!(
        number_owner(true, Some("local-1".to_string()), paired_trunk.clone()),
        Owner::Own
    );
    assert_eq!(
        number_owner(false, Some("local-1".to_string()), Owner::Own),
        Owner::paired("local-1")
    );
    assert_eq!(number_owner(false, None, paired_trunk.clone()), paired_trunk);
    assert_eq!(number_owner(false, Some(String::new()), Owner::Own), Owner::Own);
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
    let merged = merge_config(Some(own), Some(("local-1", paired.clone())));
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

    // a dedicated edge node: the paired node's alone, with its policy
    let dedicated = merge_config(None, Some(("local-1", paired)));
    assert_eq!(dedicated["trunkList"].as_array().unwrap().len(), 1);
    assert_eq!(dedicated["policy"]["maxConcurrentCalls"], 20);
    // a normal node: its own alone
    let normal = merge_config(Some(json!({"trunkList": [], "policy": {}})), None);
    assert!(normal["trunkList"].as_array().unwrap().is_empty());
    assert!(normal["blockedSourceList"].as_array().unwrap().is_empty());
}

#[test]
fn the_paired_domain_is_read_from_its_configure() {
    assert_eq!(
        domain_of(r#"{"sipDomain": " SIP.Local.example. "}"#),
        "sip.local.example"
    );
    assert_eq!(domain_of(r#"{"xmppDomain": "chat.local.example"}"#), "");
    assert_eq!(domain_of("not json"), "");
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
