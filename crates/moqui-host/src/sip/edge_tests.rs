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
use serde_json::json;

fn value(entity: &str, pk: Json, fields: Json) -> replica::ReplicaValue {
    replica::ReplicaValue {
        paired_node_id: "local".to_string(),
        set_name: TRUNKS_SET.to_string(),
        entity_name: entity.to_string(),
        pk,
        fields,
    }
}

#[test]
fn the_engine_config_from_the_copies_is_what_the_voice_services_answer() {
    let values = [
        value(
            "moqui.call.VoiceTrunk",
            json!({"voiceTrunkId": "T1"}),
            json!({"name": "Main", "host": "sip.example.com", "port": 5061, "transportEnumId": "VtrTls",
                "mediaEncryptionEnumId": "VmeSrtp", "providerProfileEnumId": "VppTelnyx",
                "authJson": "{\"username\":\"acct\",\"password\":\"pw\",\"algorithms\":\"SHA-256,MD4\"}",
                "registerFlag": "Y", "inboundAddresses": "192.0.2.0/24, 198.51.100.7", "emergencyCapable": "Y"}),
        ),
        value(
            "moqui.call.VoiceTrunk",
            json!({"voiceTrunkId": "T2"}),
            json!({"host": "10.0.0.2", "port": "5060", "transportEnumId": "VtrUdp", "mediaEncryptionEnumId": "VmePlain",
                "providerProfileEnumId": "VppPlainLan", "authJson": null}),
        ),
        // a disabled trunk, and one whose credentials do not read, are left out
        value(
            "moqui.call.VoiceTrunk",
            json!({"voiceTrunkId": "T3"}),
            json!({"host": "h", "disabled": "Y"}),
        ),
        value(
            "moqui.call.VoiceTrunk",
            json!({"voiceTrunkId": "T4"}),
            json!({"host": "h", "authJson": "not json"}),
        ),
        value(
            "moqui.call.VoicePolicy",
            json!({"voicePolicyId": "DEFAULT"}),
            json!({"internationalEnabled": "Y", "homeCountryCode": "+44", "emergencyNumbers": "999, 112",
                "authFailureLimit": 3, "premiumPrefixes": ""}),
        ),
    ];
    let config = config_of(&values);
    let trunks = config["trunkList"].as_array().unwrap();
    assert_eq!(trunks.len(), 2, "{trunks:?}");
    assert_eq!(trunks[0]["voiceTrunkId"], "T1");
    assert_eq!(trunks[0]["transport"], "tls");
    assert_eq!(trunks[0]["mediaEncryption"], "srtp");
    assert_eq!(trunks[0]["port"], 5061);
    assert_eq!(trunks[0]["username"], "acct");
    assert_eq!(
        trunks[0]["algorithms"],
        json!(["SHA-256"]),
        "only the algorithms it knows"
    );
    assert_eq!(trunks[0]["inboundAddresses"], json!(["192.0.2.0/24", "198.51.100.7"]));
    assert_eq!(trunks[0]["register"], true);
    assert_eq!(trunks[0]["inboundDigest"], false);
    assert_eq!(trunks[1]["name"], "T2");
    assert_eq!(trunks[1]["transport"], "udp");
    assert_eq!(trunks[1]["mediaEncryption"], "plain");
    assert_eq!(trunks[1]["port"], 5060);
    assert_eq!(trunks[1]["algorithms"], json!(["SHA-256", "MD5"]), "the default");
    assert_eq!(trunks[1]["inboundDigest"], true, "a PBX's profile");
    let policy = &config["policy"];
    assert_eq!(policy["internationalEnabled"], true);
    assert_eq!(policy["premiumEnabled"], false);
    assert_eq!(policy["homeCountryCode"], "44");
    assert_eq!(policy["emergencyNumbers"], json!(["999", "112"]));
    assert_eq!(policy["authFailureLimit"], 3);
    assert_eq!(policy["maxConcurrentCalls"], 20, "Voice.js's default");
    assert!(
        policy.get("premiumPrefixes").is_none(),
        "empty keeps the engine's default"
    );
    // nothing paired yet: no trunks, the defaults
    assert_eq!(config_of(&[])["trunkList"], json!([]));
}

#[test]
fn a_number_s_fallback_from_its_copy() {
    let fallback = fallback_from(&json!({"voiceTrunkId": "T1", "voicemailFlag": "Y",
        "greetingLocation": "dbresource://moqui/call/voicemail/greeting/B1/greeting-1.ogg",
        "forwardNumber": "+15550001111", "forwardSeconds": 5}))
    .unwrap();
    assert!(fallback.voicemail);
    assert_eq!(fallback.forward.as_deref(), Some("+15550001111"));
    assert_eq!(fallback.forward_seconds, 10, "held to 10 to 120");
    assert_eq!(fallback.trunk.as_deref(), Some("T1"));
    assert!(fallback.greeting.is_some());
    assert_eq!(
        fallback_from(&json!({"voicemailFlag": "Y"})).map(|f| f.forward_seconds),
        Some(30)
    );
    assert_eq!(
        fallback_from(&json!({"voicemailFlag": "N", "forwardNumber": ""})),
        None,
        "nothing to do"
    );
}

#[test]
fn the_fallback_s_ids_and_the_offline_errors_are_told_apart() {
    assert!(is_fallback("edge-fallback-1234-in"));
    assert!(!is_fallback("100234"));
    assert!(is_offline(&format!("{}: the link is down", moqui_core::edge::OFFLINE)));
    assert!(is_offline(&format!(
        "refused: {}: the local node did not answer the call of x in time",
        moqui_core::edge::OFFLINE
    )));
    assert!(!is_offline("notFound: no such number"));
}
