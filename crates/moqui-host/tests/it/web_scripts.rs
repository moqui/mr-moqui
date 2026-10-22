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

//! The shipped `ProxyServlet.js` as a conf servlet on a real server (docs/Web-Scripts.md): the request's method, path,
//! query and body go to a local upstream, and an answer larger than a script's 64 MiB heap, sent in chunks, streams back
//! whole, so it never passed through the script.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use crate::common::{exchange_bytes, header, http_request, start_with_conf};

/// The size of the large answer: over the 64 MiB a script engine may hold.
const LARGE: usize = 80 * 1024 * 1024;

/// One upstream exchange: `GET /large` is [`LARGE`] bytes in 1 MiB chunks, anything else echoes the request line, the
/// headers it passed on and the body.
fn serve_upstream(listener: TcpListener) {
    for stream in listener.incoming().take(2) {
        let mut stream = stream.unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut length = 0;
        let mut seen = Vec::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            let header = header.trim_end().to_string();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            let name = name.trim().to_ascii_lowercase();
            if name == "content-length" {
                length = value.trim().parse::<usize>().unwrap();
            }
            seen.push(name);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        if line.starts_with("GET /large") {
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
                )
                .unwrap();
            let chunk: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
            for _ in 0..LARGE / chunk.len() {
                stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).unwrap();
                stream.write_all(&chunk).unwrap();
                stream.write_all(b"\r\n").unwrap();
            }
            stream.write_all(b"0\r\n\r\n").unwrap();
        } else {
            let text = format!(
                "{}|{}|{}",
                line.trim_end(),
                seen.join(","),
                String::from_utf8_lossy(&body)
            );
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 202 Accepted\r\nContent-Type: text/plain\r\nX-Upstream: yes\r\nContent-Length: {}\r\n\r\n{text}",
                        text.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
        }
    }
}

/// The body of a chunked response.
fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let end = raw.windows(2).position(|pair| pair == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&raw[..end]).unwrap().trim(), 16).unwrap();
        raw = &raw[end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

#[test]
fn the_proxy_servlet_passes_requests_on_and_streams_a_large_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream = listener.local_addr().unwrap();
    let serving = std::thread::spawn(move || serve_upstream(listener));
    let conf = format!(
        r#"<webapp-list><webapp name="webroot">
            <servlet name="TestProxy" location="component://webroot/script/ProxyServlet.js">
                <init-param name="proxyTo" value="http://{upstream}/"/>
                <init-param name="prefix" value="/upstream"/>
                <url-pattern>/upstream/*</url-pattern>
            </servlet>
        </webapp></webapp-list>"#
    );
    let gate = start_with_conf("web-scripts", &conf, &[]);
    let address = gate.server.address();

    let (status, headers, body) = exchange_bytes(
        address,
        &http_request(
            "POST",
            "/upstream/echo/path?a=1&b=two",
            &[
                ("Content-Type", "text/plain"),
                ("Cookie", "moqui.session=secret"),
                ("X-Pass", "on"),
            ],
            "the body",
        ),
    );
    // the body streams back with its length left to the chunks, as every proxied body
    let body = String::from_utf8(dechunk(&body)).unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(header(&headers, "x-upstream").as_deref(), Some("yes"));
    let (line, rest) = body.split_once('|').unwrap();
    let (passed, sent) = rest.split_once('|').unwrap();
    assert_eq!(line, "POST /echo/path?a=1&b=two HTTP/1.1");
    assert_eq!(sent, "the body");
    assert!(passed.split(',').any(|name| name == "x-pass"), "{passed}");
    assert!(
        !passed.split(',').any(|name| name == "cookie"),
        "the client's cookie stays here: {passed}"
    );

    let (status, headers, large) = exchange_bytes(address, &http_request("GET", "/upstream/large", &[], ""));
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "transfer-encoding").as_deref(), Some("chunked"));
    let large = dechunk(&large);
    assert_eq!(large.len(), LARGE);
    assert!(large.chunks(1024 * 1024).all(|chunk| chunk
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte == (index % 251) as u8)));
    serving.join().unwrap();
    drop(gate);
}
