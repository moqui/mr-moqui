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

//! The elFinder tools screen on the seeded embedded Postgres: the screen renders in html, and its `command` transition
//! runs the connector against a `dbresource://` volume, with a multipart upload and the `file` download through
//! `ec.web.sendResourceResponse`. The commands one by one are in `moqui-core/src/service/elfinder/tests.rs`.

#![cfg(all(unix, feature = "embedded-postgres", feature = "postgres", feature = "axum"))]

use std::net::SocketAddr;

use crate::common::{basic, exchange, header, http_request, start};
use base64::Engine as _;

const SCREEN: &str = "/System/Resource/ElFinder";
const ROOT: &str = "dbresource://gate-elfinder";

fn hash(path: &str) -> String {
    format!("db_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path))
}

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

/// POST one command as the client does, with `requestType: 'post'`. Returns the status, headers, and body.
fn command(address: SocketAddr, auth: &str, pairs: &[(&str, &str)]) -> (u16, Vec<(String, String)>, String) {
    let mut body = format!("resourceRoot={}", encode(ROOT));
    for (key, value) in pairs {
        body.push_str(&format!("&{}={}", encode(key), encode(value)));
    }
    let request = http_request(
        "POST",
        &format!("{SCREEN}/command"),
        &[
            ("Authorization", auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &body,
    );
    exchange(address, &request)
}

fn json(address: SocketAddr, auth: &str, pairs: &[(&str, &str)]) -> serde_json::Value {
    let (status, _, body) = command(address, auth, pairs);
    assert_eq!(status, 200, "{pairs:?}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("{pairs:?}: {err}: {body}"))
}

#[test]
fn the_elfinder_screen_renders_and_its_connector_runs_on_a_dbresource_volume() {
    let gate = start("elfinder");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    let (status, _, page) = exchange(
        address,
        &http_request("GET", &format!("{SCREEN}.html"), &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 200, "{page}");
    assert!(!page.contains("Error rendering"), "{page}");
    assert!(page.contains(r#"<div id="elfinder"></div>"#), "{page}");
    assert!(page.contains("Resource/ElFinder/command"), "{page}");
    assert!(page.contains("dbresource://mantle/content"), "{page}");

    let opened = json(address, &john, &[("cmd", "open"), ("init", "1"), ("tree", "1")]);
    assert_eq!(opened["api"], "2.0", "{opened}");
    assert_eq!(opened["cwd"]["hash"], "db_cm9vdA", "{opened}");
    assert_eq!(opened["cwd"]["name"], ROOT, "{opened}");

    let made = json(
        address,
        &john,
        &[("cmd", "mkdir"), ("target", "db_cm9vdA"), ("name", "docs")],
    );
    assert_eq!(made["added"][0]["hash"], hash("docs"), "{made}");

    // The client's upload: a multipart body with the command fields and the files as upload[].
    let boundary = "gateElFinderBoundary";
    let mut body = String::new();
    for (name, value) in [
        ("cmd", "upload"),
        ("target", hash("docs").as_str()),
        ("resourceRoot", ROOT),
    ] {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"upload[]\"; filename=\"../a.txt\"\r\n\
         Content-Type: text/plain\r\n\r\nhello gate\r\n--{boundary}--\r\n"
    ));
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let request = http_request(
        "POST",
        &format!("{SCREEN}/command"),
        &[("Authorization", &john), ("Content-Type", &content_type)],
        &body,
    );
    let (status, _, text) = exchange(address, &request);
    assert_eq!(status, 200, "{text}");
    let uploaded: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(uploaded["added"][0]["name"], "a.txt", "{uploaded}");
    assert_eq!(uploaded["added"][0]["hash"], hash("docs/a.txt"), "{uploaded}");

    let listed = json(address, &john, &[("cmd", "ls"), ("target", &hash("docs"))]);
    assert_eq!(listed["list"], serde_json::json!(["a.txt"]), "{listed}");

    let (status, headers, text) = command(
        address,
        &john,
        &[("cmd", "file"), ("target", &hash("docs/a.txt")), ("download", "1")],
    );
    assert_eq!(status, 200, "{text}");
    assert_eq!(text, "hello gate");
    let disposition = header(&headers, "Content-Disposition").unwrap_or_default();
    assert!(
        disposition.starts_with("attachment; filename=\"a.txt\""),
        "{disposition}"
    );
    assert_eq!(header(&headers, "Content-Type").as_deref(), Some("text/plain"));
    let (status, headers, _) = command(address, &john, &[("cmd", "file"), ("target", &hash("docs/a.txt"))]);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "Content-Disposition").as_deref(), Some("inline"));

    // A target that climbs out of the volume is an elFinder error, not the volume root.
    let refused = json(
        address,
        &john,
        &[("cmd", "file"), ("target", &hash("docs/../../../etc/passwd"))],
    );
    assert_eq!(refused, serde_json::json!({"error": "errFileNotFound"}), "{refused}");
    // rm with a garbled target or the root hash removes nothing.
    let refused = json(address, &john, &[("cmd", "rm"), ("targets[]", "garbage")]);
    assert_eq!(refused, serde_json::json!({"error": "errFileNotFound"}), "{refused}");
    let refused = json(address, &john, &[("cmd", "rm"), ("targets[]", "db_cm9vdA")]);
    assert_eq!(refused["error"], "Operation not allowed on the root", "{refused}");
    let listed = json(address, &john, &[("cmd", "ls"), ("target", &hash("docs"))]);
    assert_eq!(listed["list"], serde_json::json!(["a.txt"]), "{listed}");

    let renamed = json(
        address,
        &john,
        &[("cmd", "rename"), ("target", &hash("docs/a.txt")), ("name", "b.txt")],
    );
    assert_eq!(renamed["added"][0]["hash"], hash("docs/b.txt"), "{renamed}");
    let removed = json(address, &john, &[("cmd", "rm"), ("targets[]", &hash("docs"))]);
    assert_eq!(
        removed["removed"],
        serde_json::json!([hash("docs/b.txt"), hash("docs")]),
        "{removed}"
    );
    let listed = json(address, &john, &[("cmd", "ls"), ("target", "db_cm9vdA")]);
    assert_eq!(listed["list"], serde_json::json!([]), "{listed}");
}
