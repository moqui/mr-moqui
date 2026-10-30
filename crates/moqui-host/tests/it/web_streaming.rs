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

//! Files sent by `sendResourceResponse` stream on the seeded embedded Postgres: a large `git://` file (a content blob
//! in the database) read a window at a time as the client takes it, one byte range of it, its `ETag` and a 304, no
//! compression of video, and downloads that a client does not read do not hold the request workers.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "git"))]

use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::common::{basic, exchange, exchange_bytes, header, http_request, start_with_data};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.resource.GitRepository repositoryId="stream-db" repositoryKind="db" description="Streamed files"/>
</entity-facade-xml>"#;
const ROOT: &str = "git://stream-db";

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The elFinder command that sends the file at `path` of the repository.
fn file_url(path: &str) -> String {
    let hash = format!("db_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path));
    format!(
        "/System/Resource/ElFinder/command?resourceRoot={}&cmd=file&target={hash}&download=1",
        encode(ROOT)
    )
}

#[test]
fn large_files_stream_with_ranges_and_validators() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_data("web-streaming", DATA);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let clip: Vec<u8> = (0..9 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let text: String = (0..20_000)
        .map(|line| format!("line {line} of a long text file\n"))
        .collect();
    {
        let factory = Arc::clone(gate.server.factory());
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let resources = factory.resource();
        resources.write_bytes(&format!("{ROOT}/media/clip.mp4"), &clip).unwrap();
        resources
            .write_bytes(&format!("{ROOT}/docs/big.txt"), text.as_bytes())
            .unwrap();
        moqui_core::user::logout().unwrap();
    }
    let get = |path: &str, extra: &[(&str, &str)]| {
        let mut headers = vec![("Authorization", john.as_str())];
        headers.extend_from_slice(extra);
        exchange_bytes(address, &http_request("GET", path, &headers, ""))
    };

    // the whole file, as video is: not compressed, its length known
    let clip_url = file_url("media/clip.mp4");
    let (status, headers, body) = get(&clip_url, &[("Accept-Encoding", "gzip")]);
    assert_eq!(status, 200, "{headers:?}");
    assert_eq!(header(&headers, "content-length").as_deref(), Some("9437184"));
    assert_eq!(header(&headers, "content-encoding"), None);
    assert_eq!(header(&headers, "accept-ranges").as_deref(), Some("bytes"));
    assert_eq!(body.len(), clip.len());
    assert!(body == clip, "the streamed body is the file");
    let etag = header(&headers, "etag").expect("an ETag");
    assert!(etag.starts_with("\"sha256-"), "{etag}");

    // one range of it, the last bytes, past the end
    let (status, headers, body) = get(&clip_url, &[("Range", "bytes=5000000-5000099")]);
    assert_eq!(status, 206);
    assert_eq!(
        header(&headers, "content-range").as_deref(),
        Some("bytes 5000000-5000099/9437184")
    );
    assert_eq!(body, clip[5_000_000..5_000_100]);
    let (status, _, body) = get(&clip_url, &[("Range", "bytes=-10")]);
    assert_eq!(status, 206);
    assert_eq!(body, clip[clip.len() - 10..]);
    let (status, headers, _) = get(&clip_url, &[("Range", "bytes=99999999-")]);
    assert_eq!(status, 416);
    assert_eq!(header(&headers, "content-range").as_deref(), Some("bytes */9437184"));
    // a range of a large part, streamed
    let (status, _, body) = get(&clip_url, &[("Range", "bytes=1000000-8999999")]);
    assert_eq!(status, 206);
    assert!(body == clip[1_000_000..9_000_000]);

    // the validators
    let (status, _, body) = get(&clip_url, &[("If-None-Match", etag.as_str())]);
    assert_eq!(status, 304);
    assert!(body.is_empty());
    let (status, headers, _) = get(&clip_url, &[("Range", "bytes=0-9"), ("If-Range", "\"another\"")]);
    assert_eq!(status, 200, "a range of another version is the whole file");
    assert_eq!(header(&headers, "content-length").as_deref(), Some("9437184"));

    // text is compressed for a client that takes it
    let (status, headers, _) = get(&file_url("docs/big.txt"), &[("Accept-Encoding", "gzip")]);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-encoding").as_deref(), Some("gzip"));
    let (status, headers, body) = get(&file_url("docs/big.txt"), &[]);
    assert_eq!(status, 200);
    assert!(
        header(&headers, "etag").is_some_and(|tag| tag.starts_with("\"git-")),
        "{headers:?}"
    );
    assert_eq!(String::from_utf8(body).unwrap(), text);

    // downloads the clients do not read leave the request workers free
    let mut slow = Vec::new();
    for _ in 0..12 {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(http_request("GET", &clip_url, &[("Authorization", john.as_str())], "").as_bytes())
            .unwrap();
        slow.push(stream);
    }
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    let opened = format!(
        "/System/Resource/ElFinder/command?resourceRoot={}&cmd=open&init=1",
        encode(ROOT)
    );
    let (status, _, body) = exchange(
        address,
        &http_request("GET", &opened, &[("Authorization", john.as_str())], ""),
    );
    assert_eq!(status, 200, "{body}");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    drop(slow);

    // an upload is read as it arrives, spooled, and streamed into the repository
    let upload: Vec<u8> = clip.iter().chain(clip.iter()).chain(clip.iter()).copied().collect();
    let boundary = "moqui-upload-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"upload[]\"; filename=\"upload.mp4\"\r\n\
             Content-Type: video/mp4\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&upload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let media = format!(
        "db_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("media")
    );
    let path = format!(
        "/System/Resource/ElFinder/command?resourceRoot={}&cmd=upload&target={media}",
        encode(ROOT)
    );
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {john}\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let started = Instant::now();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    for piece in body.chunks(256 * 1024) {
        stream.write_all(piece).unwrap();
    }
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response).unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    assert!(text.contains("upload.mp4"), "{text}");
    eprintln!("uploaded {} bytes in {:?}", upload.len(), started.elapsed());
    let (status, headers, read) = get(&file_url("media/upload.mp4"), &[]);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-length"), Some(upload.len().to_string()));
    assert!(read == upload, "the uploaded file reads back");
    let spool = gate.runtime.join("tmp/upload");
    let left = std::fs::read_dir(&spool).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(left, 0, "no spool file is left");
}
