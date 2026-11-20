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

/// SEC-CLU-88: a gRPC connection that sends nothing ends after its first-byte wait; one that sent something ends once it
/// has had no call in flight for the idle period, and not while a call is in flight.
#[tokio::test]
async fn a_grpc_connection_with_nothing_sent_or_no_call_in_flight_closes() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let short = std::time::Duration::from_millis(150);
    let limit = std::time::Duration::from_secs(5);
    // nothing sent
    let (_client, server) = tokio::io::duplex(64);
    let mut silent = Idle::with_limits(server, Arc::new(crate::http::Activity::new()), short, short * 2);
    let mut buf = [0_u8; 8];
    let started = std::time::Instant::now();
    let read = tokio::time::timeout(limit, silent.read(&mut buf))
        .await
        .expect("closed")
        .unwrap();
    assert_eq!(read, 0, "EOF");
    assert!(started.elapsed() >= short);
    // a byte, then a call in flight past the idle period, then none
    let (mut client, server) = tokio::io::duplex(64);
    let activity = Arc::new(crate::http::Activity::new());
    let mut talking = Idle::with_limits(server, Arc::clone(&activity), short, short * 2);
    client.write_all(b"P").await.unwrap();
    assert_eq!(talking.read(&mut buf).await.unwrap(), 1);
    activity.started();
    let open = tokio::time::timeout(short * 4, talking.read(&mut buf)).await;
    assert!(open.is_err(), "still open with a call in flight");
    activity.ended();
    let read = tokio::time::timeout(limit, talking.read(&mut buf))
        .await
        .expect("closed")
        .unwrap();
    assert_eq!(read, 0, "EOF once idle");
}

#[test]
fn the_pass_authority_serves_plain_h2c_only_on_loopback_unless_told() {
    let public: SocketAddr = "0.0.0.0:4317".parse().unwrap();
    let loopback: SocketAddr = "127.0.0.1:4317".parse().unwrap();
    // with the property unset, as in every other test of this binary
    if moqui_lib::env::get_prop_or_env(crate::pass_authority::INSECURE_PROPERTY).trim() != "true" {
        assert!(!pass_allowed(public, false, false));
    }
    assert!(pass_allowed(public, true, false));
    assert!(pass_allowed(loopback, false, false));
    assert!(pass_allowed("[::1]:4317".parse().unwrap(), false, false));
}
