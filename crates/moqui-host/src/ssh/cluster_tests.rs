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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::cluster::Sender;
use moqui_core::context::Error as CoreError;
use russh::keys::PrivateKey;

use aws_lc_rs::signature::Ed25519KeyPair;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{login_refused, prove_client, prove_server, PeerSlot, SshSender, PEER_IDLE};

#[test]
fn the_cluster_login_is_taken_only_on_the_ssh_transport_from_a_member() {
    let member: std::net::IpAddr = "10.0.0.5".parse().unwrap();
    let stranger: std::net::IpAddr = "203.0.113.9".parse().unwrap();
    let members = std::collections::HashSet::from([member]);
    assert_eq!(login_refused(true, false, member, &members), None);
    // an IPv4 member seen through a dual-stack socket
    let mapped: std::net::IpAddr = "::ffff:10.0.0.5".parse().unwrap();
    assert_eq!(login_refused(true, false, mapped, &members), None);
    assert!(
        login_refused(true, false, stranger, &members).is_some(),
        "not a member's address"
    );
    assert!(
        login_refused(false, false, member, &members).is_some(),
        "the transport is https"
    );
    assert!(
        login_refused(true, true, member, &members).is_some(),
        "through an edge tunnel"
    );
}

struct NoSender;

impl Sender for NoSender {
    fn post(
        &self,
        _base_url: &str,
        _topic: &str,
        _payload: &[u8],
    ) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        Err(CoreError::new("not in this test"))
    }
}

#[test]
fn a_peer_left_idle_is_dropped_on_the_next_post() {
    let sender = SshSender {
        factory: Weak::new(),
        https: Arc::new(NoSender),
        key: Arc::new(PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        )),
        proof: Arc::new(pair(7)),
        peers: Mutex::new(HashMap::new()),
    };
    let long_ago = Instant::now()
        .checked_sub(PEER_IDLE + Duration::from_secs(1))
        .expect("an instant before the idle limit");
    sender
        .peers
        .lock()
        .unwrap()
        .insert("ssh://gone:22".to_string(), (PeerSlot::default(), long_ago));
    let first = sender.slot("ssh://live:22");
    let again = sender.slot("ssh://live:22");
    assert!(Arc::ptr_eq(&first, &again));
    let peers = sender.peers.lock().unwrap();
    assert!(!peers.contains_key("ssh://gone:22"));
    assert!(peers.contains_key("ssh://live:22"));
}

fn pair(seed: u8) -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap()
}

const HOST: &str = "SHA256:the-server-host-key";

/// Run a channel's proof between a server with `server_key` that names its host key `server_host`, and a client with
/// `client_key` that saw `client_host`: each side's answer, and the server's next line after its proof (a frame's
/// length, when it took the client).
fn prove(
    server_key: Ed25519KeyPair,
    server_host: &'static str,
    client_key: Ed25519KeyPair,
    client_host: &'static str,
) -> (Result<(), String>, Result<(), String>, String) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let (server_end, client_end) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut stream = BufReader::new(server_end);
            let proven = prove_server(&mut stream, &server_key, server_host).await;
            let mut next = String::new();
            if proven.is_ok() {
                let _ = stream.read_line(&mut next).await;
            }
            (proven, next.trim().to_string())
        });
        let mut stream = BufReader::new(client_end);
        let client = prove_client(&mut stream, &client_key, client_host).await;
        if client.is_ok() {
            stream.get_mut().write_all(b"12\n").await.unwrap();
        }
        drop(stream);
        let (server, next) = server.await.unwrap();
        (server, client, next)
    })
}

/// SEC-CLU-04: both ends of a cluster channel prove the cluster key, bound to the server's host key, before any frame.
#[test]
fn a_cluster_channel_proves_the_cluster_key_both_ways_before_any_frame() {
    let (server, client, next) = prove(pair(1), HOST, pair(1), HOST);
    assert_eq!(server, Ok(()));
    assert_eq!(client, Ok(()));
    assert_eq!(next, "12", "the frames follow the proof");

    // a connecting peer with another key is refused, and the server reads no frame
    let (server, client, next) = prove(pair(1), HOST, pair(2), HOST);
    assert!(server.unwrap_err().contains("not of the cluster key"));
    assert!(client.unwrap_err().contains("refused this member's proof"));
    assert_eq!(next, "");

    // a server without the cluster key (a row naming another host, with that host's key) gets no frame
    let (_, client, _) = prove(pair(3), HOST, pair(1), HOST);
    assert!(client.is_err(), "the sender refuses a host that does not prove the key");

    // a proof made for another host (relayed through a host in the middle) is refused
    let (server, client, _) = prove(pair(1), HOST, pair(1), "SHA256:the-host-in-the-middle");
    assert!(server.unwrap_err().contains("not of the cluster key"));
    assert!(client.is_err());
}

/// SEC-CLU-04: a connecting peer that does not answer the challenge with a proof (a frame at once, as an older member
/// sends) is refused before its frame is read.
#[test]
fn a_cluster_peer_that_sends_no_proof_is_refused() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (proven, answer) = runtime.block_on(async {
        let (server_end, client_end) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut stream = BufReader::new(server_end);
            prove_server(&mut stream, &pair(1), HOST).await
        });
        let mut stream = BufReader::new(client_end);
        let mut challenge = String::new();
        stream.read_line(&mut challenge).await.unwrap();
        assert!(challenge.starts_with("challenge "), "{challenge}");
        stream.get_mut().write_all(b"2\n{}").await.unwrap();
        let mut answer = String::new();
        stream.read_line(&mut answer).await.unwrap();
        (server.await.unwrap(), answer)
    });
    assert!(proven.unwrap_err().contains("no proof"));
    assert_eq!(answer.trim(), "403");
}
