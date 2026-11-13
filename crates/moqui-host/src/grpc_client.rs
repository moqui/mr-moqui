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

//! gRPC clients on an HTTP/2 connection of our own: the [`H2Channel`] the generated clients take, and [`dial`], which
//! opens such a connection over TCP, with TLS (ALPN `h2`) or without. The cluster's sender (`crate::cluster_grpc`),
//! the PASS authority's (`crate::pass_authority::client`) and the edge link (plans/archive/Edge-Node.md §5.1) use them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::net::TcpStream;

/// The sending half of an HTTP/2 connection whose driver runs on the runtime.
pub(crate) type H2Send = hyper::client::conn::http2::SendRequest<tonic::body::Body>;

/// An HTTP/2 connection's sender as a tower service, which the generated gRPC clients take: a connection [`dial`]
/// opened, or one of the edge link's, which the other end dialed or answered.
#[derive(Clone, Debug)]
pub(crate) struct H2Channel(pub(crate) H2Send);

impl tower::Service<http::Request<tonic::body::Body>> for H2Channel {
    type Response = http::Response<hyper::body::Incoming>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        Box::pin(self.0.send_request(request))
    }
}

/// Where and how [`dial`] connects.
pub(crate) struct Dial<'a> {
    /// What the other end is, for messages: "the PASS authority", "cluster peer".
    pub what: &'a str,
    pub host: &'a str,
    pub port: u16,
    /// The TLS settings, which offer ALPN `h2`, and the server name to send; `None` is plain HTTP/2 (h2c).
    pub tls: Option<(Arc<rustls::ClientConfig>, rustls::pki_types::ServerName<'static>)>,
    /// For the TCP connection and the TLS handshake together.
    pub timeout: Duration,
    /// HTTP/2 PINGs this often, and the connection is closed when one is not answered in `keep_alive_timeout`.
    pub keep_alive: Duration,
    pub keep_alive_timeout: Duration,
}

/// Why [`dial`] gave no connection.
#[derive(Debug)]
pub(crate) enum DialError {
    /// The TLS handshake chose a protocol other than `h2`, or none: the other end does not speak gRPC there.
    NotH2,
    Failed(String),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotH2 => f.write_str("the other end did not choose HTTP/2 (ALPN h2)"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// An HTTP/2 connection to `target`, its driver spawned on the current runtime.
///
/// # Errors
///
/// [`DialError::NotH2`] when TLS did not choose `h2`; else what failed, with `target.what` and its address.
pub(crate) async fn dial(target: Dial<'_>) -> Result<H2Send, DialError> {
    let Dial {
        what,
        host,
        port,
        tls,
        timeout,
        keep_alive,
        keep_alive_timeout,
    } = target;
    let place = format!("{host}:{port}");
    let connect = async {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|err| DialError::Failed(format!("could not connect to {what} at {place}: {err}")))?;
        let _ = tcp.set_nodelay(true);
        let io: Box<dyn Io> = match tls {
            Some((config, name)) => {
                let stream = tokio_rustls::TlsConnector::from(config)
                    .connect(name, tcp)
                    .await
                    .map_err(|err| DialError::Failed(format!("TLS with {what} at {place} failed: {err}")))?;
                if stream.get_ref().1.alpn_protocol() != Some(b"h2".as_slice()) {
                    return Err(DialError::NotH2);
                }
                Box::new(stream)
            }
            None => Box::new(tcp),
        };
        Ok(io)
    };
    let io = tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| DialError::Failed(format!("{what} at {place} did not answer in time")))??;
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(keep_alive)
        .keep_alive_timeout(keep_alive_timeout)
        .handshake(hyper_util::rt::TokioIo::new(io))
        .await
        .map_err(|err| DialError::Failed(format!("HTTP/2 with {what} at {place} failed: {err}")))?;
    let what = what.to_string();
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!("the connection to {what} at {place} ended: {err}");
        }
    });
    Ok(send)
}
