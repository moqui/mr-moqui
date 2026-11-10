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

//! A screen sent as it renders, over the listener (docs/ui/Screen-Output.md): a page past the screen facade's
//! `stream-threshold` arrives chunked, its first bytes before the render ends, compressed too; a small page keeps its
//! `Content-Length`; a failure after the head ends the body without its last chunk.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::common::{header, http_request, start_with_conf_files};

const CONF: &str = r#"<screen-facade stream-threshold="4096">
    <screen location="component://webroot/screen/webroot.xml">
        <subscreens-item name="streamtest" location="component://streamtest/screen/StreamTest.xml"/>
    </screen>
</screen-facade>"#;

const COMPONENT: &str = r#"<component name="streamtest"/>"#;

const ROOT: &str = r#"<screen require-authentication="false" standalone="true">
    <subscreens default-item="Small"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;

/// Rows enough to pass the threshold, then a second of work: the rows are on their way before it ends.
const SLOW: &str = r#"<screen require-authentication="false">
    <actions><script>rows = Array.from({length: 3000}, (_, i) => i)</script></actions>
    <widgets>
        <section-iterate name="Rows" list="rows" entry="row"><widgets><label text="row ${row}"/></widgets></section-iterate>
        <section name="Late"><actions><script>const until = Date.now() + 1000; while (Date.now() &lt; until) {}</script></actions>
            <widgets><label text="the end"/></widgets></section>
    </widgets>
</screen>"#;

/// Rows past the threshold, then a refused section, which fails the render after its head.
const FAILS: &str = r#"<screen require-authentication="false">
    <actions><script>rows = Array.from({length: 600}, (_, i) => i)</script></actions>
    <widgets>
        <section-iterate name="Rows" list="rows" entry="row"><widgets><label text="row ${row}"/></widgets></section-iterate>
        <section name="Late"><actions><script>const refusal = new Error("refused late"); refusal.moquiErrorKind = "Unauthorized"; throw refusal</script></actions>
            <widgets><label text="secret"/></widgets></section>
    </widgets>
</screen>"#;

const SMALL: &str =
    r#"<screen require-authentication="false"><widgets><label text="a small page"/></widgets></screen>"#;

/// A response read as it arrives: the head, then the body's chunks with the time each came.
struct Arrived {
    head: String,
    head_at: Duration,
    /// The decoded chunks and when each came.
    chunks: Vec<(Duration, Vec<u8>)>,
    /// Whether the body ended with its last chunk (`0\r\n\r\n`).
    complete: bool,
    /// The body, for a response with a `Content-Length`.
    whole: Option<Vec<u8>>,
}

impl Arrived {
    fn header(&self, name: &str) -> Option<String> {
        let headers: Vec<(String, String)> = self
            .head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .collect();
        header(&headers, name)
    }

    fn body(&self) -> Vec<u8> {
        self.whole
            .clone()
            .unwrap_or_else(|| self.chunks.iter().flat_map(|(_, bytes)| bytes.clone()).collect())
    }
}

/// Send `path` and read the response as it comes, decoding a chunked body.
fn fetch(address: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Arrived {
    let started = Instant::now();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut all = vec![("Accept", "text/html")];
    all.extend_from_slice(headers);
    stream
        .write_all(http_request("GET", path, &all, "").as_bytes())
        .unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0_u8; 16 * 1024];
    let mut read = |buf: &mut Vec<u8>| -> bool {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => false,
            Ok(count) => {
                buf.extend_from_slice(&tmp[..count]);
                true
            }
        }
    };
    let split = loop {
        if let Some(at) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        assert!(read(&mut buf), "the connection closed before the head");
    };
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let head_at = started.elapsed();
    let mut rest: Vec<u8> = buf[split + 4..].to_vec();
    let mut arrived = Arrived {
        head,
        head_at,
        chunks: Vec::new(),
        complete: false,
        whole: None,
    };
    if let Some(length) = arrived
        .header("content-length")
        .and_then(|text| text.parse::<usize>().ok())
    {
        while rest.len() < length && read(&mut rest) {}
        arrived.whole = Some(rest);
        arrived.complete = true;
        return arrived;
    }
    // chunked: a size line, the bytes, CRLF; a size of 0 ends the body
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            if !read(&mut rest) {
                return arrived;
            }
            continue;
        };
        let size_text = String::from_utf8_lossy(&rest[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).unwrap();
        if size == 0 {
            arrived.complete = true;
            return arrived;
        }
        while rest.len() < line_end + 2 + size + 2 {
            if !read(&mut rest) {
                return arrived;
            }
        }
        let bytes = rest[line_end + 2..line_end + 2 + size].to_vec();
        arrived.chunks.push((started.elapsed(), bytes));
        rest.drain(..line_end + 2 + size + 2);
    }
}

#[test]
fn a_screen_streams_past_the_threshold_and_a_small_one_comes_whole() {
    if !crate::common::own_process() {
        return;
    }
    let files = [
        ("component/streamtest/component.xml", COMPONENT),
        ("component/streamtest/screen/StreamTest.xml", ROOT),
        ("component/streamtest/screen/StreamTest/Slow.xml", SLOW),
        ("component/streamtest/screen/StreamTest/Fails.xml", FAILS),
        ("component/streamtest/screen/StreamTest/Small.xml", SMALL),
    ];
    let gate = start_with_conf_files("screen-stream", None, CONF, &[], &files);
    let address = gate.server.address();

    // the rows arrive while the last section still works, with the session in the head
    let slow = fetch(address, "/streamtest/Slow", &[]);
    assert!(slow.head.starts_with("HTTP/1.1 200"), "{}", slow.head);
    assert!(slow.header("content-length").is_none(), "{}", slow.head);
    assert_eq!(
        slow.header("transfer-encoding").as_deref(),
        Some("chunked"),
        "{}",
        slow.head
    );
    assert!(
        slow.header("set-cookie")
            .is_some_and(|cookie| cookie.contains("moqui.session=")),
        "{}",
        slow.head
    );
    assert!(slow.complete, "the body ends with its last chunk");
    let body = String::from_utf8_lossy(&slow.body()).into_owned();
    assert!(
        body.contains("row 0") && body.contains("row 2999") && body.contains("the end"),
        "{body}"
    );
    let (first_at, _) = slow.chunks.first().expect("a chunk").clone();
    let (last_at, _) = slow.chunks.last().expect("a chunk").clone();
    assert!(
        last_at.saturating_sub(first_at) >= Duration::from_millis(700),
        "the first bytes came before the render ended: head {:?}, first {first_at:?}, last {last_at:?}",
        slow.head_at
    );

    // compressed, it still streams
    let gzip = fetch(address, "/streamtest/Slow", &[("Accept-Encoding", "gzip")]);
    assert_eq!(
        gzip.header("content-encoding").as_deref(),
        Some("gzip"),
        "{}",
        gzip.head
    );
    assert!(gzip.complete);
    let (first_at, _) = gzip.chunks.first().expect("a chunk").clone();
    let (last_at, _) = gzip.chunks.last().expect("a chunk").clone();
    assert!(
        last_at.saturating_sub(first_at) >= Duration::from_millis(700),
        "{first_at:?} {last_at:?}"
    );
    assert!(gzip.body().starts_with(&[0x1f, 0x8b]), "a gzip stream");

    // a failure after the head: where it stopped is written, and the body has no last chunk
    let fails = fetch(address, "/streamtest/Fails", &[]);
    assert!(fails.head.starts_with("HTTP/1.1 200"), "{}", fails.head);
    let body = String::from_utf8_lossy(&fails.body()).into_owned();
    assert!(
        body.contains("row 599") && body.contains("Error rendering page"),
        "{body}"
    );
    assert!(!body.contains("secret"), "{body}");
    assert!(!fails.complete, "a failed body must not end as if it were whole");

    // a small page is sent whole
    let small = fetch(address, "/streamtest/Small", &[]);
    assert!(small.head.starts_with("HTTP/1.1 200"), "{}", small.head);
    assert!(small.header("content-length").is_some(), "{}", small.head);
    assert!(String::from_utf8_lossy(&small.body()).contains("a small page"));
}
