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

//! Raw requests of a client spec (plans/archive/ClientTestHarness.md 5.3): the request text sent as it is over TCP, or TLS
//! for an `https` server, for paths that must not be normalized and requests no client library would send. Also the
//! TLS stream the WebSocket client uses.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::http::{Answer, Server, Session, MAX_BODY};

/// A stream a raw request or a socket reads and writes: plain, or TLS for https.
pub(crate) trait Duplex: Read + Write + Send {}

impl<T: Read + Write + Send> Duplex for T {}

/// A connection to the server: TCP, then TLS over it for https. The TCP stream is kept to set read timeouts.
pub(crate) fn connect(
    server: &Server,
    insecure: bool,
    timeout: Duration,
) -> Result<(Box<dyn Duplex>, TcpStream), String> {
    use std::net::ToSocketAddrs as _;
    let address = (server.host.as_str(), server.port)
        .to_socket_addrs()
        .map_err(|err| format!("{}:{}: {err}", server.host, server.port))?
        .next()
        .ok_or_else(|| format!("{}:{}: no address", server.host, server.port))?;
    let tcp = TcpStream::connect_timeout(&address, timeout).map_err(|err| format!("{address}: {err}"))?;
    tcp.set_read_timeout(Some(timeout)).map_err(|err| err.to_string())?;
    tcp.set_write_timeout(Some(timeout)).map_err(|err| err.to_string())?;
    let kept = tcp.try_clone().map_err(|err| err.to_string())?;
    if !server.secure {
        return Ok((Box::new(tcp), kept));
    }
    let name = rustls::pki_types::ServerName::try_from(server.host.clone()).map_err(|err| err.to_string())?;
    let connection = rustls::ClientConnection::new(tls_config(insecure)?, name).map_err(|err| err.to_string())?;
    Ok((Box::new(rustls::StreamOwned::new(connection, tcp)), kept))
}

/// The client TLS settings: the web's root certificates, or with `--insecure` any certificate.
fn tls_config(insecure: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?;
    let config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
            .with_no_client_auth()
    } else {
        builder
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth()
    };
    Ok(Arc::new(config))
}

/// `--insecure`: any server certificate, with the signatures still checked against it.
#[derive(Debug)]
struct AnyCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Send `text` as it is, with the session's `Cookie` header added after the request line when `cookies`, and read
/// the answer: to its `Content-Length` or chunked end, to the connection's close, or to the timeout.
pub(crate) fn send(
    server: &Server,
    session: &Session,
    text: &str,
    cookies: bool,
    insecure: bool,
    timeout: Duration,
) -> Result<Answer, String> {
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let url = server.join(path);
    let request = match session.cookie_header(&url).filter(|_| cookies) {
        Some(cookie) => with_header(text, "Cookie", &cookie),
        None => text.to_string(),
    };
    let started = Instant::now();
    let (mut stream, _tcp) = connect(server, insecure, timeout)?;
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|err| format!("raw request: {err}"))?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    let head_only = request.starts_with("HEAD ");
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => buf.extend_from_slice(&chunk[..count]),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if find_header_end(&buf).is_some() {
                    break;
                }
                return Err(format!("raw request: no answer in {}s", timeout.as_secs_f64()));
            }
            // A TLS server that closes without close_notify, or a reset after the answer, ends it.
            Err(_) if find_header_end(&buf).is_some() => break,
            Err(err) => return Err(format!("raw request: {err}")),
        }
        if complete(&buf, head_only) {
            break;
        }
    }
    let (status, headers, body) = parse(&buf);
    let length = body.len();
    let mut body = body;
    body.truncate(MAX_BODY);
    Ok(Answer {
        status,
        url,
        headers,
        body,
        length,
        elapsed: started.elapsed(),
    })
}

/// The request with a header line after its request line.
fn with_header(text: &str, name: &str, value: &str) -> String {
    match text.find('\n') {
        Some(at) => {
            let eol = if text[..at].ends_with('\r') { "\r\n" } else { "\n" };
            format!("{}{name}: {value}{eol}{}", &text[..=at], &text[at + 1..])
        }
        None => text.to_string(),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn head_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Whether the answer in `buf` is whole.
fn complete(buf: &[u8], head_only: bool) -> bool {
    let Some(split) = find_header_end(buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..split]);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    if head_only || status == 204 || status == 304 || (100..200).contains(&status) {
        return true;
    }
    let body = &buf[split + 4..];
    if head_value(&head, "transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        return body.ends_with(b"0\r\n\r\n");
    }
    match head_value(&head, "content-length").and_then(|value| value.parse::<usize>().ok()) {
        Some(length) => body.len() >= length,
        None => false,
    }
}

/// The status, the headers and the body (chunks joined) of an answer.
fn parse(buf: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let split = find_header_end(buf);
    let head = String::from_utf8_lossy(split.map_or(buf, |at| &buf[..at])).to_string();
    let raw_body = split.map_or(&[][..], |at| &buf[at + 4..]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked"));
    let body = if chunked { dechunk(raw_body) } else { raw_body.to_vec() };
    (status, headers, body)
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = data.windows(2).position(|window| window == b"\r\n") {
        let size_text = String::from_utf8_lossy(&data[..eol]);
        let Ok(size) = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16) else {
            break;
        };
        data = &data[eol + 2..];
        if size == 0 || data.len() < size {
            out.extend_from_slice(&data[..size.min(data.len())]);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_goes_after_the_request_line() {
        assert_eq!(
            with_header("GET / HTTP/1.1\r\nHost: x\r\n\r\n", "Cookie", "a=1"),
            "GET / HTTP/1.1\r\nCookie: a=1\r\nHost: x\r\n\r\n"
        );
        assert_eq!(
            with_header("GET / HTTP/1.0\n\n", "Cookie", "a=1"),
            "GET / HTTP/1.0\nCookie: a=1\n\n"
        );
    }

    #[test]
    fn answers_parse_with_a_length_or_chunks() {
        let fixed = b"HTTP/1.1 404 Not Found\r\nContent-Length: 5\r\nX-A: 1\r\n\r\nnope!";
        assert!(complete(fixed, false));
        assert!(!complete(&fixed[..fixed.len() - 1], false));
        let (status, headers, body) = parse(fixed);
        assert_eq!((status, body.as_slice()), (404, &b"nope!"[..]));
        assert_eq!(headers[1], ("X-A".to_string(), "1".to_string()));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\n\r\n";
        assert!(complete(chunked, false));
        assert_eq!(parse(chunked).2, b"Wikipedia");
        assert!(complete(b"HTTP/1.1 204 No Content\r\n\r\n", false));
        assert!(complete(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n", true));
    }
}
