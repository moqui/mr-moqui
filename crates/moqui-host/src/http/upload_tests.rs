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

//! The pace of an upload and the uploads arriving at once (SEC-RTE-91).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};

use super::{upload_wait, Uploads};
use crate::http::{BodyRefused, HttpState, ListenLimits};

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// An upload has its idle timeout, cut to when its average would fall under `upload-min-rate` once `body-timeout` has
/// passed, so a byte now and then does not hold a connection.
#[test]
fn an_upload_must_keep_a_minimum_rate() {
    let limits = ListenLimits {
        body: Some(Duration::from_secs(60)),
        upload_idle: Some(Duration::from_secs(60)),
        upload_min_rate: 1024,
        ..ListenLimits::defaults()
    };
    let wait = |elapsed: u64, total: usize| upload_wait(&limits, Duration::from_secs(elapsed), total);
    // within body-timeout the idle timeout alone
    assert_eq!(wait(0, 0).ok().flatten(), Some(Duration::from_secs(60)));
    // then the time the bytes so far earn at the rate
    assert_eq!(wait(50, 0).ok().flatten(), Some(Duration::from_secs(10)));
    assert_eq!(wait(100, 50 * 1024).ok().flatten(), Some(Duration::from_secs(10)));
    // a byte every 50 seconds falls behind
    assert!(matches!(wait(110, 2), Err(BodyRefused::TimedOut)));
    // 0 turns the rate off
    let off = ListenLimits {
        upload_min_rate: 0,
        ..limits
    };
    assert_eq!(
        upload_wait(&off, Duration::from_secs(1000), 0).ok().flatten(),
        Some(Duration::from_secs(60))
    );
}

/// One client has at most `max-uploads-per-client` arriving at once, whether or not web protection is on; an upload with
/// no user takes only a share of `max-uploads`, the rest kept for uploads by a user; a slot is given back when its upload
/// ends.
#[test]
fn a_client_and_anonymous_uploads_take_only_their_share_of_the_slots() {
    let limits = ListenLimits {
        max_uploads: 4,
        max_uploads_per_client: 2,
        ..ListenLimits::defaults()
    };
    let uploads = Arc::new(Uploads::default());
    let one = Some(ip("203.0.113.1"));
    let first = uploads.take(&limits, one, false).ok().expect("a slot");
    let _second = uploads.take(&limits, one, false).ok().expect("a slot");
    assert!(matches!(uploads.take(&limits, one, false), Err(BodyRefused::SpoolFull)));
    // an upload with a user is counted in the totals only
    let _with_user = uploads.take(&limits, None, true).ok().expect("a slot");
    // three of four arriving: an upload with no user finds no room past three quarters, one with a user does
    assert_eq!(uploads.arriving(), 3);
    assert!(!uploads.anonymous_room(&limits));
    assert!(matches!(
        uploads.take(&limits, Some(ip("203.0.113.9")), false),
        Err(BodyRefused::SpoolFull)
    ));
    let _last = uploads.take(&limits, None, true).ok().expect("a slot");
    assert!(matches!(uploads.take(&limits, None, true), Err(BodyRefused::SpoolFull)));
    // a slot is given back when its upload ends
    drop(first);
    assert_eq!(uploads.arriving(), 3);
    assert!(uploads.take(&limits, one, false).is_err(), "still three of four");
}

/// An [`HttpState`] whose guard is `guard`.
fn state_with(guard: Arc<crate::guard::Guard>) -> HttpState {
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let (_, shutdown) = tokio::sync::watch::channel(false);
    HttpState {
        factory: Arc::clone(context.factory()),
        shutdown,
        limits: ListenLimits::defaults(),
        guard,
        uploads: Arc::default(),
    }
}

/// An upload is counted per client only when its address is the client's: behind a reverse proxy (or an office NAT)
/// on loopback or a private network with no client addressing configured, the peer is every client's, so only the
/// totals apply; with addressing, the client the proxy names is counted, an IPv6 one by `client-ipv6-prefix`; an
/// upload with a user is never counted per client.
#[test]
fn an_upload_is_counted_per_client_only_by_an_address_that_is_the_clients() {
    use super::per_client_key;
    use moqui_core::web::address::{parse_cidrs, ClientAddressing};
    let node =
        moqui_lib::node::MNode::parse_text("web-protection", r#"<web-protection client-ipv6-prefix="48"/>"#).unwrap();
    let unaddressed = state_with(Arc::new(crate::guard::Guard::new(crate::guard::GuardConf::from_node(
        Some(&node),
        Arc::new(ClientAddressing::new("", Vec::new())),
    ))));
    // a proxy on this machine or the private network, believed by nobody: the whole site behind one address
    assert_eq!(
        per_client_key(&unaddressed, ip("127.0.0.1"), ip("127.0.0.1"), false),
        None
    );
    assert_eq!(
        per_client_key(&unaddressed, ip("10.0.0.5"), ip("10.0.0.5"), false),
        None
    );
    // a public address is a client's
    assert_eq!(
        per_client_key(&unaddressed, ip("8.8.8.8"), ip("8.8.8.8"), false),
        Some(ip("8.8.8.8"))
    );
    assert_eq!(per_client_key(&unaddressed, ip("8.8.8.8"), ip("8.8.8.8"), true), None);
    assert_eq!(
        per_client_key(
            &unaddressed,
            ip("2a00:1450:4001:82a::9"),
            ip("2a00:1450:4001:82a::9"),
            false
        ),
        Some(ip("2a00:1450:4001::"))
    );
    let addressed = state_with(Arc::new(crate::guard::Guard::new(crate::guard::GuardConf::from_node(
        Some(&node),
        Arc::new(ClientAddressing::new(
            "X-Forwarded-For",
            parse_cidrs("127.0.0.1", "test"),
        )),
    ))));
    assert_eq!(
        per_client_key(&addressed, ip("127.0.0.1"), ip("203.0.113.4"), false),
        Some(ip("203.0.113.4"))
    );
}
/// Through the screen route: a multipart body that falls under `upload-min-rate` once `body-timeout` has passed is
/// answered 408 while it arrives, and its slot is given back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_upload_is_cut_off_through_the_screen_route() {
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let factory = Arc::clone(context.factory());
    factory.request_pool().start(1, 4).unwrap();
    let (_, shutdown) = tokio::sync::watch::channel(false);
    let state = Arc::new(HttpState {
        factory: Arc::clone(&factory),
        shutdown,
        limits: ListenLimits {
            body: Some(Duration::from_millis(300)),
            upload_idle: Some(Duration::from_secs(30)),
            upload_min_rate: 1024,
            ..ListenLimits::defaults()
        },
        guard: crate::guard::tests::guard("<web-protection/>", ""),
        uploads: Arc::default(),
    });
    // ten bytes, then nothing for longer than they earn at 1 KiB a second past the 300 ms grace
    let chunks = futures_util::stream::unfold(0u8, |sent| async move {
        match sent {
            0 => Some((Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"--x\r\nConte")), 1)),
            1 => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Some((
                    Ok(bytes::Bytes::from_static(
                        b"nt-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--x--\r\n",
                    )),
                    2,
                ))
            }
            _ => None,
        }
    });
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/upload")
        .header("content-type", "multipart/form-data; boundary=x")
        .body(axum::body::Body::from_stream(chunks))
        .unwrap();
    let peer: SocketAddr = "127.0.0.1:40001".parse().unwrap();
    let started = std::time::Instant::now();
    let response = crate::http::dispatch_screen(State(Arc::clone(&state)), ConnectInfo(peer), request).await;
    assert_eq!(response.status().as_u16(), 408);
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    assert_eq!(state.uploads.arriving(), 0);
}
