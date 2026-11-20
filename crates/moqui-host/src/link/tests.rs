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

use super::{new_key, read_answer, write_answer, Answer, Backoff, Identity};

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
fn a_node_key_gives_the_same_fingerprint_each_time_and_a_bad_one_is_refused() {
    let key = new_key().unwrap();
    let first = Identity::from_key(&key, "local-1").unwrap();
    let again = Identity::from_key(&key, "local-1").unwrap();
    assert_eq!(first.fingerprint, again.fingerprint);
    assert_eq!(first.fingerprint.len(), 64);
    let other = Identity::from_key(&new_key().unwrap(), "local-1").unwrap();
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
