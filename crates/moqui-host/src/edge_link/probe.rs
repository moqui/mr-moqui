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

//! The wizard's XMPP check on a local node (plans/Edge-Node.md §5.7, the Test step, E10): this node's XMPP server
//! takes tunneled connections of the kind (§5.4, E9), and a client stream opened to the edge node's tunnel port, on
//! the host of `edge_node_url`, comes back to it through the link. The stream is opened as a client would (direct TLS
//! with ALPN `xmpp-client`, or in the clear before STARTTLS) and read up to its features; nothing more is sent, so no
//! credential crosses and no sign-in is counted. The TLS is this node's own, so any certificate is taken: the
//! stream's `from` naming this node's domain is what says it came back here.

use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// How long the probe may take: connecting, the TLS and the stream's features.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of the stream read while looking for its features.
const MAX_READ: usize = 16 * 1024;

/// The kind's name in a message.
fn kind(direct: bool) -> &'static str {
    if direct {
        "direct TLS"
    } else {
        "STARTTLS"
    }
}

/// Check that a client stream to the edge node's tunnel `port` (direct TLS when `direct`) reaches this node's XMPP
/// server: what it reached, or why not.
///
/// # Errors
///
/// Returns why: this node runs no XMPP server for the kind, no edge node address, or the stream did not come back.
pub async fn xmpp_probe(
    factory: Arc<ExecutionContextFactory>,
    edge_url: String,
    port: u16,
    direct: bool,
) -> Result<String, String> {
    let domain = moqui_lib::env::get_prop_or_env("xmpp_domain")
        .trim()
        .to_ascii_lowercase();
    if domain.is_empty() {
        return Err("This node runs no XMPP server: set xmpp_domain".to_string());
    }
    if !crate::xmpp::running(&factory, direct) {
        return Err(format!(
            "This node's XMPP server does not take {} connections: it is not running, or has no certificate for {domain}",
            kind(direct)
        ));
    }
    drop(factory);
    if port == 0 {
        return Err(format!("The edge node's {} tunnel port is off", kind(direct)));
    }
    let Some(host) = super::offer::url_host(&edge_url) else {
        return Err("The edge node's address is not known".to_string());
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    match tokio::time::timeout(PROBE_TIMEOUT, open(&host, port, &domain, direct)).await {
        Ok(Ok(())) => Ok(format!(
            "A client stream to {host}:{port} ({}) reached this node's XMPP server for {domain}",
            kind(direct)
        )),
        Ok(Err(why)) => Err(format!("{host}:{port} ({}): {why}", kind(direct))),
        Err(_) => Err(format!(
            "{host}:{port} ({}): no stream features in {} seconds",
            kind(direct),
            PROBE_TIMEOUT.as_secs()
        )),
    }
}

async fn open(host: &str, port: u16, domain: &str, direct: bool) -> Result<(), String> {
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|err| format!("not reached: {err}"))?;
    let _ = tcp.set_nodelay(true);
    if !direct {
        return features(tcp, domain, true).await;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = crate::tls_pin::PinVerifier::new(crate::tls_pin::Pin::Deferred, false, &provider);
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("no TLS: {err}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"xmpp-client".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(domain.to_string())
        .map_err(|err| format!("{domain} is not a TLS name: {err}"))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|err| format!("the TLS did not complete, so the tunnel did not reach an XMPP server: {err}"))?;
    features(tls, domain, false).await
}

/// Open the stream to `domain` and read up to its features: they must come from `domain`, and offer STARTTLS when
/// `starttls`.
async fn features<S>(mut stream: S, domain: &str, starttls: bool) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let header = format!(
        "<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' \
         to='{domain}' version='1.0'>"
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|err| format!("the stream was not sent: {err}"))?;
    let mut seen: Vec<u8> = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let read = stream
            .read(&mut buf)
            .await
            .map_err(|err| format!("the stream broke: {err}"))?;
        if read == 0 {
            return Err("closed with no stream: the link is down, or the local node refused the tunnel".to_string());
        }
        seen.extend_from_slice(&buf[..read]);
        let text = String::from_utf8_lossy(&seen);
        if text.contains("<stream:error") {
            let error = text.split_once("<stream:error").map_or("", |(_, error)| error);
            return Err(format!(
                "a stream error: <stream:error{}",
                error.chars().take(200).collect::<String>()
            ));
        }
        // the features always have children before sign-in: STARTTLS, or the mechanisms
        if text.contains("</stream:features>") || text.contains("</features>") {
            let from_ok = text.contains(&format!("from='{domain}'")) || text.contains(&format!("from=\"{domain}\""));
            if !from_ok {
                return Err(format!("an XMPP server answered, but not for {domain}"));
            }
            if starttls && !text.contains("urn:ietf:params:xml:ns:xmpp-tls") {
                return Err("the XMPP server offered no STARTTLS".to_string());
            }
            let _ = stream.write_all(b"</stream:stream>").await;
            let _ = stream.shutdown().await;
            return Ok(());
        }
        if seen.len() > MAX_READ {
            return Err("no stream features in the first 16 KB".to_string());
        }
    }
}
