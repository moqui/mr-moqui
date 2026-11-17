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

use super::{login_refused, PeerSlot, SshSender, PEER_IDLE};

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
