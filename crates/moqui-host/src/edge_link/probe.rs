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

//! The wizard's XMPP check on a local node (plans/archive/Edge-Node.md §5.7, the Test step, E10): this node's XMPP server
//! takes tunneled connections of the kind (§5.4, E9), and a client connection to the edge node's tunnel port, on the
//! host of `edge_node_url`, comes back to it through the link. The connection is opened as a client's would be
//! (direct TLS with ALPN `xmpp-client`, or the stream in the clear, its features, then STARTTLS), and its TLS must
//! present this node's own certificate for the domain (pinned to the whole certificate, from `runtime/cert`): only
//! this node's server has its key, so a handshake that completes is the proof it came back here. Nothing more is
//! sent, so no credential crosses and no sign-in is counted.
//!
//! The calls check ([`turn_probe`], E10 calls) is an allocation on the edge node's TURN with the credential
//! `make#IceServers` signed with the secret the link handed this node (`rtc/turn_probe.rs`, under the rtc feature).

use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// How long the probe may take: connecting, the stream's features and the TLS.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of the stream read while looking for what it waits for.
const MAX_READ: usize = 16 * 1024;

/// The kind's name in a message.
fn kind(direct: bool) -> &'static str {
    if direct {
        "direct TLS"
    } else {
        "STARTTLS"
    }
}

/// The Test step's TURN check: a UDP allocation on the edge node's TURN at `url` with `username` and `credential`,
/// a permission for its media address's IP `peer`, and the allocation deleted; blocking, a few seconds at most.
///
/// # Errors
///
/// Returns why: the URL is not TURN over UDP, no answer, or the allocation or the permission refused.
pub fn turn_probe(url: &str, username: &str, credential: &str, peer: &str) -> Result<String, String> {
    crate::rtc::turn_probe::probe(url, username, credential, peer)
}

/// Check that a client connection to the edge node's tunnel `port` (direct TLS when `direct`) reaches this node's
/// XMPP server: what it reached, or why not.
///
/// # Errors
///
/// Returns why: this node runs no XMPP server for the kind or has no certificate for the domain, no edge node
/// address, or the connection did not come back here.
pub async fn xmpp_probe(
    factory: Arc<ExecutionContextFactory>,
    edge_url: String,
    port: u16,
    direct: bool,
) -> Result<String, String> {
    let domain = moqui_lib::env::get_prop_or_env("xmpp_domain")
        .trim()
        .trim_end_matches('.')
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
    let Some(leaf) = crate::tls::leaf_for(&factory.runtime_dir().join("cert"), &domain) else {
        return Err(format!("This node has no certificate for {domain} in runtime/cert"));
    };
    drop(factory);
    if port == 0 {
        return Err(format!("The edge node's {} tunnel port is off", kind(direct)));
    }
    let Some(host) = moqui_core::edge::claims::url_host(&edge_url) else {
        return Err("The edge node's address is not known".to_string());
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let pin: [u8; 32] = Sha256::digest(leaf.as_ref()).into();
    match tokio::time::timeout(PROBE_TIMEOUT, open(&host, port, &domain, direct, pin)).await {
        Ok(Ok(())) => Ok(format!(
            "A client connection to {host}:{port} ({}) reached this node's XMPP server for {domain}",
            kind(direct)
        )),
        Ok(Err(why)) => Err(format!("{host}:{port} ({}): {why}", kind(direct))),
        Err(_) => Err(format!(
            "{host}:{port} ({}): no answer in {} seconds",
            kind(direct),
            PROBE_TIMEOUT.as_secs()
        )),
    }
}

async fn open(host: &str, port: u16, domain: &str, direct: bool, pin: [u8; 32]) -> Result<(), String> {
    let mut tcp = TcpStream::connect((host, port))
        .await
        .map_err(|err| format!("not reached: {err}"))?;
    let _ = tcp.set_nodelay(true);
    if !direct {
        // the stream in the clear: its features from the domain, with STARTTLS, then the go-ahead
        open_stream(&mut tcp, domain).await?;
        let features = read_until(&mut tcp, &["</stream:features>", "</features>"]).await?;
        if !features.contains(&format!("from='{domain}'")) && !features.contains(&format!("from=\"{domain}\"")) {
            return Err(format!("an XMPP server answered, but not for {domain}"));
        }
        if !features.contains("urn:ietf:params:xml:ns:xmpp-tls") {
            return Err("the XMPP server offered no STARTTLS".to_string());
        }
        tcp.write_all(b"<starttls xmlns='urn:ietf:params:xml:ns:xmpp-tls'/>")
            .await
            .map_err(|err| format!("STARTTLS was not sent: {err}"))?;
        let answer = read_until(&mut tcp, &["<proceed", "<failure"]).await?;
        if !answer.contains("<proceed") {
            return Err("the XMPP server refused STARTTLS".to_string());
        }
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = crate::tls_pin::PinVerifier::new(crate::tls_pin::Pin::Certificate(pin), false, &provider);
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("no TLS: {err}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    if direct {
        config.alpn_protocols = vec![b"xmpp-client".to_vec()];
    }
    let name = rustls::pki_types::ServerName::try_from(domain.to_string())
        .map_err(|err| format!("{domain} is not a TLS name: {err}"))?;
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|err| {
            format!("the TLS was not this node's own for {domain}, so the tunnel did not come back here: {err}")
        })?;
    let _ = tls.shutdown().await;
    Ok(())
}

/// Send the stream header to `domain`.
async fn open_stream<S: AsyncWrite + Unpin>(stream: &mut S, domain: &str) -> Result<(), String> {
    let header = format!(
        "<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' \
         to='{domain}' version='1.0'>"
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|err| format!("the stream was not sent: {err}"))
}

/// Read the stream until one of `ends` is in it: what was read; a stream error, the end of the stream or too much
/// is why not.
async fn read_until<S: AsyncRead + Unpin>(stream: &mut S, ends: &[&str]) -> Result<String, String> {
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
        if let Some((_, error)) = text.split_once("<stream:error") {
            return Err(format!(
                "a stream error: <stream:error{}",
                error.chars().take(200).collect::<String>()
            ));
        }
        if ends.iter().any(|end| text.contains(end)) {
            return Ok(text.into_owned());
        }
        if seen.len() > MAX_READ {
            return Err("not answered in the first 16 KB".to_string());
        }
    }
}
