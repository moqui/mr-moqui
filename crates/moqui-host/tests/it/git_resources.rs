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

//! `git://` files in a database repository on the seeded embedded Postgres: elFinder writes them (each write a commit,
//! the objects `bytea` rows of `GitObject`), the file download reads one back, `export#GitRepository` writes the
//! repository to disk, and the git command line, when installed, reads the export. The provider over each storage is
//! `moqui-git/src/tests.rs`.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "git"))]

use std::net::SocketAddr;

use crate::common::{basic, exchange, http_request, start_with_data};
use base64::Engine as _;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.resource.GitRepository repositoryId="gate-git" repositoryKind="db" description="Gate git"/>
</entity-facade-xml>"#;
const ROOT: &str = "git://gate-git";

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

fn post(address: SocketAddr, auth: &str, path: &str, body: &str) -> (u16, String) {
    let request = http_request(
        "POST",
        path,
        &[
            ("Authorization", auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        body,
    );
    let (status, _, text) = exchange(address, &request);
    (status, text)
}

fn elfinder(address: SocketAddr, auth: &str, pairs: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("resourceRoot={}", encode(ROOT));
    for (key, value) in pairs {
        body.push_str(&format!("&{}={}", encode(key), encode(value)));
    }
    post(address, auth, "/System/Resource/ElFinder/command", &body)
}

fn json(address: SocketAddr, auth: &str, pairs: &[(&str, &str)]) -> serde_json::Value {
    let (status, body) = elfinder(address, auth, pairs);
    assert_eq!(status, 200, "{pairs:?}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("{pairs:?}: {err}: {body}"))
}

fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn a_database_git_repository_is_written_through_elfinder_and_exported() {
    let gate = start_with_data("git-resources", DATA);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    let opened = json(address, &john, &[("cmd", "open"), ("init", "1")]);
    assert_eq!(opened["cwd"]["mime"], "directory", "{opened}");
    let made = json(
        address,
        &john,
        &[("cmd", "mkdir"), ("target", "db_cm9vdA"), ("name", "docs")],
    );
    assert_eq!(made["added"][0]["hash"], hash("docs"), "{made}");
    let file = json(
        address,
        &john,
        &[("cmd", "mkfile"), ("target", &hash("docs")), ("name", "a.md")],
    );
    assert_eq!(file["added"][0]["hash"], hash("docs/a.md"), "{file}");
    for content in ["# A\n\nfirst\n", "# A\n\nsecond, with ünïcode\n"] {
        let put = json(
            address,
            &john,
            &[("cmd", "put"), ("target", &hash("docs/a.md")), ("content", content)],
        );
        assert!(put["changed"].is_array(), "{put}");
    }
    let got = json(address, &john, &[("cmd", "get"), ("target", &hash("docs/a.md"))]);
    assert_eq!(got["content"], "# A\n\nsecond, with ünïcode\n", "{got}");
    let (status, text) = elfinder(
        address,
        &john,
        &[("cmd", "file"), ("target", &hash("docs/a.md")), ("download", "1")],
    );
    assert_eq!(status, 200, "{text}");
    assert_eq!(text, "# A\n\nsecond, with ünïcode\n");

    let (status, _, page) = exchange(
        address,
        &http_request(
            "GET",
            "/System/Resource/GitRepositoryDetail?repositoryId=gate-git",
            &[("Authorization", &john)],
            "",
        ),
    );
    assert_eq!(status, 200, "{page}");
    assert!(!page.contains("Error rendering"), "{page}");
    assert!(page.contains("git://gate-git/"), "{page}");

    let target = gate.runtime.join("git-export");
    let (status, body) = post(
        address,
        &john,
        "/Tools/Service/ServiceRun/runJson",
        &format!(
            "serviceName={}&repositoryId=gate-git&targetPath={}",
            encode("org.moqui.impl.GitServices.export#GitRepository"),
            encode(&target.to_string_lossy())
        ),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("exportPath"), "{body}");
    assert!(target.join("HEAD").is_file(), "{body}");
    let target_dir = target.to_string_lossy().into_owned();
    if let Some(log) = git(&["--git-dir", &target_dir, "log", "--format=%s", "main"]) {
        // mkdir (.gitkeep), mkfile, and two puts
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            [
                "Update docs/a.md",
                "Update docs/a.md",
                "Create docs/a.md",
                "Create directory docs"
            ],
            "{log}"
        );
        assert_eq!(
            git(&["--git-dir", &target_dir, "show", "main:docs/a.md"]).as_deref(),
            Some("# A\n\nsecond, with ünïcode\n")
        );
        assert!(git(&["--git-dir", &target_dir, "fsck", "--strict"]).is_some());
    }
}
