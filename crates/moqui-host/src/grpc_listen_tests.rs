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

/// SEC-CLU-88: `Track` counts a call in flight in its connection's activity from the call until its answer's body is
/// done, so a download in progress is not idle; a call with no connection activity (the HTTPS listener's) is passed
/// through.
#[tokio::test]
async fn track_counts_a_call_until_its_answers_body_ends() {
    use tower::Service as _;
    let node = moqui_lib::node::MNode::parse_text("web-protection", "<web-protection/>").unwrap();
    let guard = Arc::new(crate::guard::Guard::new(crate::guard::GuardConf::from_node(
        Some(&node),
        Arc::new(moqui_core::web::address::ClientAddressing::new("", Vec::new())),
    )));
    let activity = Arc::new(crate::http::Activity::new());
    let caller = |activity: Option<Arc<crate::http::Activity>>| Caller {
        #[cfg(feature = "edge")]
        peer: "127.0.0.1:4317".parse().unwrap(),
        client: "127.0.0.1".parse().unwrap(),
        guard: Arc::clone(&guard),
        activity,
    };
    let inner = tower::service_fn(|_request: http::Request<()>| async {
        Ok::<_, std::convert::Infallible>(http::Response::new(tonic::body::Body::new(String::from("part"))))
    });
    let mut track = Track { inner };
    let mut request = http::Request::new(());
    request.extensions_mut().insert(caller(Some(Arc::clone(&activity))));
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(!activity.idle_for().is_zero(), "idle before the call");
    let response = track.call(request).await.unwrap();
    assert!(
        activity.idle_for().is_zero(),
        "in flight while the answer's body is open"
    );
    drop(response);
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(!activity.idle_for().is_zero(), "idle once the body is done");
    let mut plain = http::Request::new(());
    plain.extensions_mut().insert(caller(None));
    assert!(track.call(plain).await.is_ok());
}

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
    assert!(!pass_allowed(public, false, false, false));
    assert!(pass_allowed(public, false, true, false), "told to serve in clear");
    assert!(pass_allowed(public, true, false, false));
    assert!(pass_allowed(loopback, false, false, false));
    assert!(pass_allowed("[::1]:4317".parse().unwrap(), false, false, false));
}
