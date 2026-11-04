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

use super::listen_port;

#[test]
fn the_https_body_limit_is_shared_and_rejected_before_reading_an_oversized_body() {
    let max = moqui_core::cluster::MAX_CLUSTER_BODY;
    let head = format!(
        "POST /cluster/collab-update HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        max + 1
    );
    let split = head.len() - 4;
    let mut buf = head.into_bytes();
    assert!(matches!(
        super::take_exchange(&mut std::io::empty(), &mut buf, split),
        Err(super::IoFail::TooLarge)
    ));
    let body = vec![b'x'; max];
    let head = format!("POST /cluster/collab-update HTTP/1.1\r\nContent-Length: {max}\r\n\r\n");
    let split = head.len() - 4;
    let mut buf = head.into_bytes();
    buf.extend_from_slice(&body);
    let exchange = super::take_exchange(&mut std::io::empty(), &mut buf, split)
        .ok()
        .flatten()
        .unwrap();
    assert!(exchange.start.starts_with("POST "));
    assert!(buf.is_empty(), "the whole body was taken");
}

#[test]
fn empty_cluster_https_port_listens_on_8444_and_zero_is_ephemeral() {
    assert_eq!(listen_port("").unwrap(), 8444);
    assert_eq!(listen_port("0").unwrap(), 0);
    assert!(listen_port("nope").is_err());
}

/// The listener and the senders on a real port, with a stand-in for applying: gRPC where TLS chose h2, HTTP/1.1 for a
/// client that offers no ALPN and for a member that keeps to `rest`.
#[cfg(feature = "grpc")]
mod on_the_port {
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

    use moqui_core::cluster::{PostOutcome, Sender, MAX_CLUSTER_BODY};
    use moqui_lib::sync::lock;
    use sha2::{Digest as _, Sha256};

    use super::super::listen::{listen_with, Apply};
    use super::super::{client_config, server_config, ClusterEdge, CoreError, Material, PeerSender, Protocol};

    fn material() -> Material {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["moqui-cluster".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let cert = cert.der().to_vec();
        let digest: [u8; 32] = Sha256::digest(&cert).into();
        Material {
            cert,
            key: key.serialized_der().to_vec(),
            digest,
        }
    }

    /// An apply that records `topic payload` and answers 503 for the payload `busy`.
    fn recording() -> (Apply, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let apply: Apply = Arc::new(move |topic: &str, body: &[u8]| {
            let text = String::from_utf8_lossy(body).into_owned();
            lock(&kept).push(format!("{topic} {text}"));
            if text == "busy" {
                503
            } else {
                204
            }
        });
        (apply, seen)
    }

    fn start(material: &Material, protocol: Protocol, apply: Apply) -> (ClusterEdge, String) {
        let edge = listen_with(
            "127.0.0.1:0".parse().unwrap(),
            server_config(material).unwrap(),
            protocol,
            apply,
            Arc::default(),
        )
        .unwrap();
        let base = format!("https://127.0.0.1:{}", edge.address().port());
        (edge, base)
    }

    /// The HTTP/1.1 sender the gRPC one falls back to, recording what it was given.
    #[derive(Default)]
    struct Rest(Mutex<Vec<String>>);

    impl Sender for Rest {
        fn post(&self, _base_url: &str, topic: &str, payload: &[u8]) -> Result<PostOutcome, CoreError> {
            lock(&self.0).push(format!("{topic} {}", String::from_utf8_lossy(payload)));
            Ok(PostOutcome::Accepted)
        }
    }

    #[test]
    fn a_batch_over_grpc_is_applied_in_order_and_stops_at_a_busy_message() {
        let material = material();
        let (apply, seen) = recording();
        let (_edge, base) = start(&material, Protocol::Grpc, apply);
        let rest = Arc::new(Rest::default());
        let sender = super::super::grpc_sender(&client_config(&material).unwrap(), rest.clone());
        let outcome = sender.post_batch(
            &base,
            &[
                ("collab-update", b"a"),
                ("session-end", b"b"),
                ("collab-update", b"busy"),
                ("collab-update", b"c"),
            ],
        );
        assert_eq!(outcome.outcomes, vec![PostOutcome::Accepted, PostOutcome::Accepted]);
        assert!(
            outcome.failed.is_some(),
            "the busy message and the one after it were not applied"
        );
        assert_eq!(*lock(&seen), ["collab-update a", "session-end b", "collab-update busy"]);
        // the rest are sent again, on the kept connection
        let outcome = sender.post_batch(&base, &[("collab-update", b"c")]);
        assert_eq!(outcome.outcomes, vec![PostOutcome::Accepted]);
        assert!(outcome.failed.is_none());
        assert_eq!(lock(&seen).last().map(String::as_str), Some("collab-update c"));
        assert!(lock(&rest.0).is_empty(), "posted over gRPC, not HTTP/1.1");
    }

    #[test]
    fn a_member_without_alpn_is_answered_over_http1() {
        let material = material();
        let (apply, seen) = recording();
        let (_edge, base) = start(&material, Protocol::Grpc, apply);
        let old = PeerSender {
            config: client_config(&material).unwrap(),
            live: Mutex::default(),
        };
        assert_eq!(old.post(&base, "collab-update", b"old").unwrap(), PostOutcome::Accepted);
        assert_eq!(
            old.post(&base, "not-a-topic", b"x").unwrap(),
            PostOutcome::Rejected(404)
        );
        assert!(
            old.post(&base, "collab-update", b"busy").is_err(),
            "a 503 is tried again"
        );
        // the HTTP/1.1 client sends a post that failed on a kept connection once more on a new one
        let seen = lock(&seen);
        assert_eq!(seen[..2], ["collab-update old", "collab-update busy"]);
        assert!(
            seen[2..].iter().all(|applied| applied == "collab-update busy"),
            "{seen:?}"
        );
    }

    #[test]
    fn a_member_that_keeps_to_rest_is_posted_to_over_http1() {
        let material = material();
        let (apply, seen) = recording();
        let (_edge, base) = start(&material, Protocol::Rest, apply);
        let rest = Arc::new(Rest::default());
        let sender = super::super::grpc_sender(&client_config(&material).unwrap(), rest.clone());
        let outcome = sender.post_batch(&base, &[("collab-update", b"one")]);
        assert_eq!(outcome.outcomes, vec![PostOutcome::Accepted]);
        assert_eq!(
            sender.post(&base, "collab-update", b"two").unwrap(),
            PostOutcome::Accepted
        );
        assert_eq!(*lock(&rest.0), ["collab-update one", "collab-update two"]);
        assert!(lock(&seen).is_empty(), "the stand-in took them, not the listener");
    }

    #[test]
    fn an_oversized_post_is_refused_before_its_body() {
        let material = material();
        let (apply, seen) = recording();
        let (_edge, base) = start(&material, Protocol::Grpc, apply);
        let config = client_config(&material).unwrap();
        let mut stream = super::super::connect_peer(&config, &base).unwrap();
        write!(
            stream,
            "POST /cluster/collab-update HTTP/1.1\r\nHost: peer\r\nContent-Length: {}\r\n\r\n",
            MAX_CLUSTER_BODY + 1
        )
        .unwrap();
        stream.flush().unwrap();
        let mut buf = Vec::new();
        let answer = super::super::read_exchange(&mut stream, &mut buf)
            .ok()
            .flatten()
            .unwrap();
        assert_eq!(super::super::status_code(&answer.start), 413);
        assert!(lock(&seen).is_empty());
    }

    #[test]
    fn a_member_without_the_cluster_certificate_is_refused() {
        let material = material();
        let (apply, seen) = recording();
        let (_edge, base) = start(&material, Protocol::Grpc, apply);
        let stranger = super::super::grpc_sender(&client_config(&self::material()).unwrap(), Arc::new(Rest::default()));
        let outcome = stranger.post_batch(&base, &[("collab-update", b"in")]);
        assert!(outcome.outcomes.is_empty());
        assert!(outcome.failed.is_some());
        assert!(lock(&seen).is_empty());
    }
}
