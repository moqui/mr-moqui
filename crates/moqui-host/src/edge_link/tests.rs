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
fn the_edge_node_s_instance_id_is_the_pinned_one_and_never_this_node_s() {
    assert!(check_edge_id("", "edge-1", "local-1").is_ok(), "pinned on a first link");
    assert!(check_edge_id("edge-1", "edge-1", "local-1").is_ok());
    assert!(check_edge_id("edge-1", "member-2", "local-1").is_err_and(|err| err.contains("changed from edge-1")));
    assert!(check_edge_id("", "local-1", "local-1").is_err_and(|err| err.contains("own instance id")));
    assert!(check_edge_id("", "", "local-1").is_err());
}
