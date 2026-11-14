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

//! The down connection's server over an in-memory HTTP/2 connection, with an app that echoes where it thinks a request
//! came from: the edge node's client header is believed and removed, forwarding headers a client wrote are removed,
//! and a passthrough tunnel's PROXY header names the client of the TLS connection inside it.

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::routing::get;
use axum::{Extension, Router};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::*;
use crate::guard::Guard;
use crate::http::{ClientAddr, ListenLimits, TlsConnection, WebApp};

async fn echo(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
    tls: Option<Extension<TlsConnection>>,
    headers: http::HeaderMap,
) -> String {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-")
            .to_string()
    };
    format!(
        "peer={peer} client={client} tls={} host={} xff={} edge={}",
        tls.is_some(),
        header("host"),
        header("x-forwarded-for"),
        header(CLIENT_HEADER)
    )
}

/// A local web app with the echo route and the TLS settings of a generated `localhost` certificate, and a client that
/// trusts it.
fn local(label: &str) -> (LocalWeb, tokio_rustls::TlsConnector, tokio::sync::watch::Sender<bool>) {
    let runtime = crate::tls::tests::temp_dir(label);
    crate::tls::ensure_dev_cert(&runtime.join("cert")).unwrap();
    let store = crate::tls::CertStore::new(runtime.join("cert"));
    let tls = Arc::new(crate::tls::server_config(store).unwrap());
    let connector = crate::tls::tests::client_trusting(&runtime.join("cert").join("localhost.crt"));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let router = Router::new().route("/echo", get(echo));
    let web = LocalWeb {
        serve: DownServe {
            app: WebApp::new(router, ListenLimits::defaults(), Arc::new(Guard::disabled())),
            tls: Some(tls),
            shutdown,
        },
        factory: Weak::new(),
        relay: None,
    };
    (web, connector, stop)
}

/// The down connection's server on one end of an in-memory stream, and the edge node's HTTP/2 client on the other.
async fn connect(web: LocalWeb) -> hyper::client::conn::http2::SendRequest<tonic::body::Body> {
    let (edge, local) = tokio::io::duplex(1 << 16);
    let grpc = tower::service_fn(|_request: http::Request<Incoming>| async {
        Ok::<_, Infallible>(http::Response::new(tonic::body::Body::empty()))
    });
    let server = builder(Duration::from_secs(30), Some(&web));
    tokio::spawn(async move {
        let _ = server
            .serve_connection(TokioIo::new(local), service(grpc, Some(web)))
            .await;
    });
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .handshake(TokioIo::new(edge))
        .await
        .unwrap();
    tokio::spawn(connection);
    send
}

async fn body_text(response: http::Response<Incoming>) -> String {
    let bytes = axum::body::to_bytes(axum::body::Body::new(response.into_body()), 1 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn a_forwarded_request_is_from_the_client_the_edge_node_names() {
    let (web, _, _stop) = local("down-forward");
    let mut send = connect(web).await;
    let request = http::Request::get("https://erp.example.com/echo")
        .version(http::Version::HTTP_2)
        .header(CLIENT_HEADER, "203.0.113.9:40000")
        .header(PROTO_HEADER, "https")
        .header("x-forwarded-for", "198.51.100.1")
        .body(tonic::body::Body::empty())
        .unwrap();
    send.ready().await.unwrap();
    let response = send.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        body_text(response).await,
        "peer=203.0.113.9:40000 client=203.0.113.9 tls=true host=erp.example.com xff=- edge=-"
    );

    // with no client named the request never reaches the app, as from the edge node or anyone
    let request = http::Request::get("https://erp.example.com/echo")
        .version(http::Version::HTTP_2)
        .body(tonic::body::Body::empty())
        .unwrap();
    send.ready().await.unwrap();
    let response = send.send_request(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

/// A request on the down connection: its status and body.
async fn ask(
    send: &mut hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    path: &str,
    client: Option<&str>,
) -> (http::StatusCode, String) {
    let mut request = http::Request::post(format!("https://erp.example.com{path}"))
        .version(http::Version::HTTP_2)
        .header("content-type", "application/grpc");
    if let Some(client) = client {
        request = request.header(CLIENT_HEADER, client);
    }
    send.ready().await.unwrap();
    let response = send
        .send_request(request.body(tonic::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, body_text(response).await)
}

#[tokio::test]
async fn a_forwarded_request_never_reaches_the_link_calls_whatever_its_content_type() {
    let (web, _, _stop) = local("down-grpc");
    let mut send = connect(web).await;
    // forwarded: the app, which has no such route, for every call of the link's, the delegated calls and queue
    // delivery too, and its echo route
    for call in ["Hello", "Ping", "Deliver", "Delegate"] {
        let path = format!("/moqui.edge.v1.LocalLink/{call}");
        let (status, _) = ask(&mut send, &path, Some("203.0.113.9:1")).await;
        assert_eq!(status, 404, "{path}");
    }
    let (status, body) = ask(&mut send, "/echo", Some("203.0.113.9:1")).await;
    assert_eq!(status, 405, "{body}");
    // nor the PASS authority's calls, whose client must be the edge node (plans/archive/Edge-Node.md §5.5)
    for call in ["Call", "Download", "Upload"] {
        let path = format!("/moqui.pass.v1.PassAuthority/{call}");
        let (status, _) = ask(&mut send, &path, Some("203.0.113.9:1")).await;
        assert_eq!(status, 404, "{path}");
    }
    // the edge node's own calls: the link's service, and the PASS authority's
    for call in ["Ping", "Deliver", "Delegate"] {
        let path = format!("/moqui.edge.v1.LocalLink/{call}");
        let (status, _) = ask(&mut send, &path, None).await;
        assert_eq!(status, 200, "{path}");
    }
    for call in ["Call", "Download", "Upload"] {
        let path = format!("/moqui.pass.v1.PassAuthority/{call}");
        let (status, _) = ask(&mut send, &path, None).await;
        assert_eq!(status, 200, "{path}");
    }
    // the edge node's Link calls are not served here, forwarded or not
    let (status, _) = ask(&mut send, "/moqui.edge.v1.Link/Replicate", None).await;
    assert_eq!(status, 404);
    let (status, _) = ask(&mut send, "/moqui.edge.v1.Link/Deliver", Some("203.0.113.9:1")).await;
    assert_eq!(status, 404);
    // gRPC elsewhere, or anything else with no client named, is neither
    let (status, _) = ask(&mut send, "/echo", None).await;
    assert_eq!(status, 404);
    let (status, _) = ask(&mut send, "/moqui.pass.v1.Other/Call", None).await;
    assert_eq!(status, 404);
}

/// Open a tunnel of `kind` on `send`: its stream, or the status it was refused with.
async fn open(
    send: &mut hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    kind: &str,
) -> Result<TokioIo<hyper::upgrade::Upgraded>, http::StatusCode> {
    let request = http::Request::builder()
        .method(http::Method::CONNECT)
        .version(http::Version::HTTP_2)
        .uri("https://moqui-edge.link/")
        .header(TUNNEL_HEADER, kind)
        .extension(hyper::ext::Protocol::from_static(TUNNEL_PROTOCOL))
        .body(tonic::body::Body::empty())
        .unwrap();
    send.ready().await.unwrap();
    let response = send.send_request(request).await.unwrap();
    if !response.status().is_success() {
        return Err(response.status());
    }
    Ok(TokioIo::new(hyper::upgrade::on(response).await.unwrap()))
}

#[tokio::test]
async fn a_passthrough_tunnel_is_from_the_client_its_proxy_header_names() {
    let (web, connector, _stop) = local("down-tunnel");
    let mut send = connect(web).await;
    let mut tunnel = open(&mut send, TUNNEL_HTTPS).await.unwrap();
    let client: SocketAddr = "203.0.113.9:5000".parse().unwrap();
    let header = crate::proxy_header::v2(client, "192.0.2.1:443".parse().unwrap());
    tunnel.write_all(&header).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(name, tunnel).await.unwrap();
    tls.write_all(
        b"GET /echo HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 198.51.100.1\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut answer = String::new();
    let _ = tls.read_to_string(&mut answer).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(
        answer.ends_with("peer=203.0.113.9:5000 client=203.0.113.9 tls=true host=localhost xff=198.51.100.1 edge=-"),
        "{answer}"
    );

    // a tunnel with no PROXY header first is closed before any TLS
    let mut bare = open(&mut send, TUNNEL_HTTPS).await.unwrap();
    bare.write_all(b"GET /echo HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut rest = Vec::new();
    let _ = bare.read_to_end(&mut rest).await;
    assert!(rest.is_empty(), "{}", String::from_utf8_lossy(&rest));

    // a kind this node has no listener for, or does not know, is refused
    assert_eq!(
        open(&mut send, TUNNEL_SSH).await.err(),
        Some(http::StatusCode::NOT_FOUND)
    );
    // an SMTP tunnel is taken only on an edge-set node's front link (plans/Edge-Node2.md §4.7): this one relays nothing
    assert_eq!(open(&mut send, "smtp").await.err(), Some(http::StatusCode::NOT_FOUND));
    assert_eq!(
        open(&mut send, "telnet").await.err(),
        Some(http::StatusCode::BAD_REQUEST)
    );
}

#[tokio::test]
async fn an_xmpp_tunnel_is_refused_on_a_node_with_no_xmpp_server() {
    // the STARTTLS and direct TLS kinds (plans/archive/Edge-Node.md §5.4, E9) go only to this node's own XMPP server, which an
    // app with no factory has not; tests/it/edge_xmpp.rs serves them on a real one
    let (web, _, _stop) = local("down-xmpp");
    let mut send = connect(web).await;
    for kind in [TUNNEL_XMPP, TUNNEL_XMPPS] {
        assert_eq!(
            open(&mut send, kind).await.err(),
            Some(http::StatusCode::NOT_FOUND),
            "{kind}"
        );
    }
}
