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

//! The XMPP domain a client connection names, read before it goes anywhere (plans/Edge-Node2.md §4.1 "XMPP", E16),
//! so one pair of ports serves every pairing's domain: on the direct TLS port the ClientHello's server name (SNI,
//! XEP-0368), on the STARTTLS port the `to` of the stream header the client sends in the clear (RFC 6120 §4.7.2). What
//! was read is kept and handed on first ([`Prefixed`]), so the node that serves the stream sees it whole and ends the
//! TLS itself. The edge node reads no further: no stanza, no credential.

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, ReadBuf};

/// The most bytes read for the domain: a ClientHello, or the XML declaration and the stream header.
const MAX_READ: usize = 16 * 1024;

/// Read from `stream` the XMPP domain it names, and every byte read: its TLS server name when `direct`, else the `to`
/// of its stream header. `None` for a stream that names none (no SNI, no `to`), or one that is not a host name.
///
/// # Errors
///
/// The stream closed or failed first, or what came is not a ClientHello or a stream header within [`MAX_READ`].
pub(crate) async fn sniff<S>(stream: &mut S, direct: bool) -> std::io::Result<(Option<String>, Vec<u8>)>
where
    S: AsyncRead + Unpin,
{
    let mut seen = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 4096];
    let mut hello = rustls::server::Acceptor::default();
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let fresh = &chunk[..read];
        seen.extend_from_slice(fresh);
        if seen.len() > MAX_READ {
            return Err(invalid("the stream's start is too long"));
        }
        if direct {
            let mut rest = fresh;
            while !rest.is_empty() {
                if hello.read_tls(&mut rest)? == 0 {
                    break;
                }
            }
            match hello.accept() {
                Ok(Some(accepted)) => {
                    let name = accepted.client_hello().server_name().and_then(host_name);
                    return Ok((name, seen));
                }
                Ok(None) => {}
                Err((err, _)) => return Err(invalid(&err.to_string())),
            }
        } else if let Some(header) = stream_to(&seen) {
            return Ok((header?, seen));
        }
    }
}

fn invalid(why: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string())
}

/// `name` as a domain: lower case with no trailing dot, `None` when it is not a host name.
pub(crate) fn host_name(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    let fine = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    fine.then_some(name)
}

/// The `to` of the stream header at the start of `bytes`: `None` while the header has not all come, `Some(Ok(None))`
/// for a header with no `to` (or one that is not a host name), and an error for a start that is not a stream header.
/// An XML declaration may come first; the header is the first start tag, of local name `stream`.
pub(crate) fn stream_to(bytes: &[u8]) -> Option<std::io::Result<Option<String>>> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        // a character cut at the end of what came so far
        Err(err) if err.error_len().is_none() => std::str::from_utf8(&bytes[..err.valid_up_to()]).ok()?,
        Err(_) => return Some(Err(invalid("the stream is not UTF-8"))),
    };
    let mut rest = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if rest.starts_with("<?") {
        let end = rest.find("?>")?;
        rest = rest[end + 2..].trim_start_matches([' ', '\t', '\r', '\n']);
    }
    if rest.is_empty() {
        return None;
    }
    let Some(tag) = rest.strip_prefix('<') else {
        return Some(Err(invalid("the stream does not start with a stream header")));
    };
    // the tag ends at the first '>' outside a quoted value
    let mut quote = None;
    let mut end = None;
    for (at, c) in tag.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => {
                end = Some(at);
                break;
            }
            _ => {}
        }
    }
    let tag = &tag[..end?];
    let name = tag
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or_default();
    if name.rsplit(':').next() != Some("stream") {
        return Some(Err(invalid("the stream's first element is not a stream header")));
    }
    Some(Ok(attribute(tag, "to").and_then(|to| host_name(&to))))
}

/// The value of the attribute `name` in a start tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + name.len()..].trim_start();
        rest = &rest[at + name.len()..];
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value = &value[1..];
        return value.find(quote).map(|end| value[..end].to_string());
    }
    None
}

/// A stream whose first bytes were read already: they are read again first, then the rest of the stream.
pub(crate) struct Prefixed<S> {
    head: Vec<u8>,
    at: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub(crate) fn new(head: Vec<u8>, inner: S) -> Self {
        Prefixed { head, at: 0, inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.at < self.head.len() {
            let take = (self.head.len() - self.at).min(buf.remaining());
            let at = self.at;
            buf.put_slice(&self.head[at..at + take]);
            self.at += take;
            if self.at == self.head.len() {
                self.head = Vec::new();
                self.at = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "xmpp_domain_tests.rs"]
mod tests;
