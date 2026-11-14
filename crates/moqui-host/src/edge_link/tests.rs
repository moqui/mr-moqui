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

use std::time::Duration;

use super::pair::{parse_url, EdgeUrl, Identity};
use super::store::normalize_code;
use super::{
    check_edge_id, check_versions, edge_info, hello, read_answer, write_answer, Answer, Backoff, LINK_VERSION,
};

#[test]
fn versions_one_apart_link_and_further_apart_are_refused_plainly() {
    assert!(check_versions(3, 3, "edge node").is_ok());
    assert!(check_versions(3, 2, "local node").is_ok());
    assert!(check_versions(3, 4, "local node").is_ok());
    let behind = check_versions(3, 1, "local node").unwrap_err();
    assert!(behind.contains("version 1") && behind.contains("version 3"), "{behind}");
    assert!(behind.contains("upgrade local node"), "{behind}");
    let ahead = check_versions(3, 5, "edge node").unwrap_err();
    assert!(ahead.contains("upgrade this node"), "{ahead}");
    assert!(check_versions(3, 0, "edge node")
        .unwrap_err()
        .contains("no link protocol version"));
    let mut far = hello("local-1", "Local", &[], &[]);
    far.link_version = LINK_VERSION + 2;
    let info = edge_info(&far, "ab");
    assert!(!info.compatible && info.message.contains("cannot link"), "{info:?}");
    assert!(edge_info(&hello("e", "Edge", &[], &[]), "ab").compatible);
}

#[tokio::test]
async fn an_answer_reads_back_as_written_and_a_long_one_is_cut() {
    let (mut client, mut server) = tokio::io::duplex(4096);
    write_answer(&mut server, Answer::Refused, "revoked").await.unwrap();
    write_answer(&mut server, Answer::Ok, &"x".repeat(5000)).await.unwrap();
    assert_eq!(
        read_answer(&mut client).await.unwrap(),
        (Answer::Refused, "revoked".to_string())
    );
    let (answer, text) = read_answer(&mut client).await.unwrap();
    assert_eq!(answer, Answer::Ok);
    assert_eq!(text.len(), 1024);
}

#[test]
fn the_backoff_doubles_to_its_cap_with_jitter_in_the_upper_half() {
    let backoff = Backoff::default();
    assert_eq!(backoff.delay(0, 0.0), Duration::from_millis(500));
    assert_eq!(backoff.delay(0, 1.0), Duration::from_secs(1));
    assert_eq!(backoff.delay(3, 1.0), Duration::from_secs(8));
    assert_eq!(backoff.delay(10, 1.0), Duration::from_secs(30));
    assert_eq!(backoff.delay(40, 0.0), Duration::from_secs(15));
    assert_eq!(Backoff::refused().delay(20, 1.0), Duration::from_secs(600));
    for _ in 0..20 {
        let jitter = super::jitter();
        assert!((0.0..1.0).contains(&jitter), "{jitter}");
    }
}

#[test]
fn edge_node_urls_take_a_host_and_an_optional_port() {
    let at = |host: &str, port: u16| EdgeUrl {
        host: host.to_string(),
        port,
    };
    assert_eq!(
        parse_url("https://edge.example.com").unwrap(),
        at("edge.example.com", 443)
    );
    assert_eq!(
        parse_url(" https://edge.example.com:8443/x ").unwrap(),
        at("edge.example.com", 8443)
    );
    assert_eq!(
        parse_url("edge.example.com:9443").unwrap(),
        at("edge.example.com", 9443)
    );
    assert_eq!(parse_url("https://[::1]:8443").unwrap(), at("::1", 8443));
    assert!(parse_url("http://edge.example.com").is_err());
    assert!(parse_url("https://").is_err());
}

#[test]
fn a_pairing_code_matches_however_it_is_typed() {
    assert_eq!(normalize_code("abcd-EFGH-2345"), "ABCDEFGH2345");
    assert_eq!(normalize_code(" ABCD EFGH 2345 "), "ABCDEFGH2345");
}

#[test]
fn a_node_key_gives_the_same_fingerprint_each_time_and_a_bad_one_is_refused() {
    let key = super::new_key().unwrap();
    let first = Identity::from_key(&key, "local-1").unwrap();
    let again = Identity::from_key(&key, "local-1").unwrap();
    assert_eq!(first.fingerprint, again.fingerprint);
    assert_eq!(first.fingerprint.len(), 64);
    let other = Identity::from_key(&super::new_key().unwrap(), "local-1").unwrap();
    assert_ne!(first.fingerprint, other.fingerprint);
    assert!(Identity::from_key("not base64!", "x").is_err());
    assert!(Identity::from_key("AAAA", "x").is_err());
}

#[test]
fn a_generated_secret_seeds_an_ed25519_link_key_with_a_stable_fingerprint() {
    use base64::Engine as _;
    let seed = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
    let first = Identity::from_key(&seed, "moqui-edge-link").unwrap();
    let again = Identity::from_key(&seed, "moqui-edge-link").unwrap();
    assert_eq!(first.fingerprint, again.fingerprint);
    assert_eq!(
        crate::tls_pin::public_key_sha256(&first.cert()).map(|digest| crate::tls_pin::hex(&digest)),
        Some(first.fingerprint.clone())
    );
    let other = base64::engine::general_purpose::STANDARD.encode([8_u8; 32]);
    assert_ne!(Identity::from_key(&other, "x").unwrap().fingerprint, first.fingerprint);
}

#[test]
fn node_ids_have_no_spaces_or_control_characters_and_a_length_limit() {
    use super::valid_node_id;
    assert!(valid_node_id("local-1"));
    assert!(valid_node_id(&"a".repeat(40)));
    assert!(!valid_node_id(""));
    assert!(!valid_node_id(&"a".repeat(41)));
    assert!(!valid_node_id("local 1"));
    assert!(!valid_node_id("local\n1"));
    assert!(!valid_node_id("local\u{7}"));
}

#[test]
fn debug_output_redacts_keys_and_codes() {
    let key = super::new_key().unwrap();
    let local = super::LocalConfig::new("https://edge.test", &key, "ab", "local-1");
    let shown = format!("{local:?}");
    assert!(!shown.contains(&key) && shown.contains("<redacted>"), "{shown}");
    let pairing = super::PairConfig::new("https://edge.test", "SECRET-CODE", "local-1");
    let shown = format!("{pairing:?}");
    assert!(!shown.contains("SECRET-CODE"), "{shown}");
}

#[test]
fn the_calls_offer_reads_back_and_one_with_no_media_address_is_not_available() {
    let calls = super::proto::CallsOffer {
        available: true,
        media_address: "203.0.113.5:10000".to_string(),
        turn_urls: vec!["turn:203.0.113.5:3478?transport=udp".to_string()],
        other_nodes: true,
    };
    let offer = super::offer::offer_of(super::proto::Offer {
        calls: Some(calls.clone()),
        ..super::proto::Offer::default()
    });
    assert!(offer.calls.available && offer.calls.other_nodes, "{:?}", offer.calls);
    assert_eq!(offer.calls.media_address, "203.0.113.5:10000");
    assert_eq!(offer.calls.turn_urls, calls.turn_urls);
    let no_address = super::offer::offer_of(super::proto::Offer {
        calls: Some(super::proto::CallsOffer {
            media_address: String::new(),
            ..calls
        }),
        ..super::proto::Offer::default()
    });
    assert!(!no_address.calls.available);
    assert!(
        !super::offer::offer_of(super::proto::Offer::default()).calls.available,
        "an edge node from before the calls area"
    );
}

#[test]
fn the_sip_offer_reads_back_and_one_with_no_address_is_not_available() {
    let sip = super::proto::SipOffer {
        available: true,
        sip_address: "203.0.113.5:5060".to_string(),
        tls_address: "203.0.113.5:5061".to_string(),
        domain: "sip.example.com".to_string(),
        registrar: true,
        trunks: vec![super::proto::SipTrunk {
            voice_trunk_id: "T1".to_string(),
            name: "Main".to_string(),
            state: "failed".to_string(),
            error: "403 Forbidden".to_string(),
        }],
        fallbacks: 3,
        other_nodes: true,
    };
    let offer = super::offer::offer_of(super::proto::Offer {
        sip: Some(sip.clone()),
        ..super::proto::Offer::default()
    });
    assert!(
        offer.sip.available && offer.sip.registrar && offer.sip.other_nodes,
        "{:?}",
        offer.sip
    );
    assert_eq!(
        (
            offer.sip.sip_address.as_str(),
            offer.sip.tls_address.as_str(),
            offer.sip.domain.as_str()
        ),
        ("203.0.113.5:5060", "203.0.113.5:5061", "sip.example.com")
    );
    assert_eq!(offer.sip.fallbacks, 3);
    let trunk = &offer.sip.trunks[0];
    assert_eq!(
        (
            trunk.voice_trunk_id.as_str(),
            trunk.name.as_str(),
            trunk.state.as_str(),
            trunk.error.as_str()
        ),
        ("T1", "Main", "failed", "403 Forbidden")
    );
    let no_address = super::offer::offer_of(super::proto::Offer {
        sip: Some(super::proto::SipOffer {
            sip_address: String::new(),
            tls_address: String::new(),
            ..sip
        }),
        ..super::proto::Offer::default()
    });
    assert!(!no_address.sip.available);
    assert!(
        !super::offer::offer_of(super::proto::Offer::default()).sip.available,
        "an edge node from before the phones area"
    );
}

#[test]
fn the_edge_node_s_instance_id_is_the_pinned_one_and_never_this_node_s() {
    assert!(check_edge_id("", "edge-1", "local-1").is_ok(), "pinned on a first link");
    assert!(check_edge_id("edge-1", "edge-1", "local-1").is_ok());
    assert!(check_edge_id("edge-1", "member-2", "local-1").is_err_and(|err| err.contains("changed from edge-1")));
    assert!(check_edge_id("", "local-1", "local-1").is_err_and(|err| err.contains("own instance id")));
    assert!(check_edge_id("", "", "local-1").is_err());
}

#[test]
fn each_role_offers_only_its_own_sets() {
    use moqui_core::edge::NodeRole;
    let full = || super::proto::Offer {
        web: Some(super::proto::WebOffer {
            public_names: vec!["edge.example.com".to_string()],
            modes: vec!["EdgeHostTerminate".to_string()],
        }),
        mail: Some(super::proto::MailOffer {
            available: true,
            ..Default::default()
        }),
        xmpp: Some(super::proto::XmppOffer {
            available: true,
            port: 5222,
            ..Default::default()
        }),
        calls: Some(super::proto::CallsOffer {
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    // plans/Edge-Node2.md §4.3: a front node web access and ports only, an edge node behind a front none of them
    let mut front = full();
    super::offer::for_role(&mut front, NodeRole::Front);
    assert!(front.web.as_ref().is_some_and(|web| !web.modes.is_empty()));
    assert!(front.xmpp.as_ref().is_some_and(|xmpp| xmpp.available));
    assert!(!front.mail.as_ref().is_some_and(|mail| mail.available));
    assert!(!front.calls.as_ref().is_some_and(|calls| calls.available));
    assert!(front.not_available.iter().any(|area| area == "mail"));
    let mut edge = full();
    super::offer::for_role(&mut edge, NodeRole::Edge);
    assert!(edge
        .web
        .as_ref()
        .is_some_and(|web| web.modes.is_empty() && web.public_names.is_empty()));
    assert!(!edge.xmpp.as_ref().is_some_and(|xmpp| xmpp.available));
    assert!(edge.mail.as_ref().is_some_and(|mail| mail.available));
    assert!(edge.not_available.iter().any(|area| area == "web"));
    for role in [NodeRole::FrontEdge, NodeRole::AllInOne] {
        let mut both = full();
        super::offer::for_role(&mut both, role);
        assert_eq!(both, full(), "{role:?}");
    }
}

#[test]
fn a_member_status_crosses_the_link_and_back() {
    use moqui_core::edge::{ClusterMemberInfo, MemberState};
    let members = vec![ClusterMemberInfo {
        instance_id: "member-a".to_string(),
        heartbeat_age_ms: 1200,
        live: true,
        version: "0.1.0".to_string(),
    }];
    let status = super::status_to_proto(MemberState::Draining, &members);
    assert_eq!(status.state, "draining");
    assert_eq!(
        super::status_from_proto(&status),
        (Some(MemberState::Draining), members)
    );
    let unknown = super::proto::MemberStatus {
        state: "asleep".to_string(),
        members: Vec::new(),
    };
    assert_eq!(super::status_from_proto(&unknown).0, None);
}

#[test]
fn a_member_key_proves_one_connection_and_is_kept_by_its_instance() {
    use super::member_key::{fingerprint, verify, MemberKey};
    let key = MemberKey::generate().unwrap();
    let binding = [7_u8; 32];
    let proof = key.prove(&binding);
    assert!(verify(&key.public(), &binding, &proof));
    assert!(!verify(&key.public(), &[8_u8; 32], &proof), "another connection's");
    let other = MemberKey::generate().unwrap();
    assert!(!verify(&other.public(), &binding, &proof), "another key's");
    assert!(!verify(&[], &binding, &proof));
    assert_eq!(fingerprint(&key.public()), key.fingerprint());
    // kept under the runtime, the same each time it is read, never in .env
    let runtime = std::env::temp_dir().join(format!("moqui-member-key-{}", std::process::id()));
    let made = MemberKey::load_or_create(&runtime).unwrap();
    let read = MemberKey::load_or_create(&runtime).unwrap();
    assert_eq!(made.fingerprint(), read.fingerprint());
    assert!(runtime.join("db/edge_member_key").is_file());
    let _ = std::fs::remove_dir_all(&runtime);
}

#[test]
fn a_member_key_file_that_is_not_one_names_itself_and_the_remedy() {
    use super::member_key::MemberKey;
    let runtime = std::env::temp_dir().join(format!("moqui-member-key-bad-{}", std::process::id()));
    std::fs::create_dir_all(runtime.join("db")).unwrap();
    std::fs::write(runtime.join("db/edge_member_key"), "not a key\n").unwrap();
    let err = MemberKey::load_or_create(&runtime).unwrap_err();
    assert!(
        err.contains("edge_member_key") && err.contains("move it aside"),
        "{err}"
    );
    // a new one is written whole, owner only, and read back the same
    std::fs::remove_file(runtime.join("db/edge_member_key")).unwrap();
    let made = MemberKey::load_or_create(&runtime).unwrap();
    assert_eq!(
        MemberKey::load_or_create(&runtime).unwrap().fingerprint(),
        made.fingerprint()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(runtime.join("db/edge_member_key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    assert!(
        std::fs::read_dir(runtime.join("db")).unwrap().count() == 1,
        "no part file left"
    );
    let _ = std::fs::remove_dir_all(&runtime);
}
