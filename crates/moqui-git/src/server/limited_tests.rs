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

//! The request limits (SEC-GIT-03): a gzip body is held to `max-request` as inflated, an SSH command's input to the
//! same limit, and an upload-pack request to `fetch-max-lines`.

use std::io::{Read, Write};

use super::super::tests::{define, fixture_with, Fixture, PASSWORD, WRITER};
use super::super::{GitHttpRequest, GitService, HttpResponder};
use super::Capped;

struct Captured {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponder for Captured {
    fn head(&mut self, status: u16, _headers: &[(&str, String)]) -> std::io::Result<()> {
        self.status = status;
        Ok(())
    }

    fn body(&mut self) -> &mut dyn Write {
        &mut self.body
    }
}

/// `count` want lines of upload-pack, then a flush.
fn wants(count: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for _ in 0..count {
        body.extend_from_slice(format!("0032want {}\n", "0".repeat(40)).as_bytes());
    }
    body.extend_from_slice(b"0000");
    body
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// An upload-pack request as the writer, served on this thread: the response body as text.
fn upload_pack(fixture: &Fixture, body: &[u8], gzipped: bool) -> String {
    let context = fixture.context.factory().new_context();
    let _entered = context.enter();
    let request = GitHttpRequest {
        repository: "lim".into(),
        service: GitService::UploadPack,
        advertise: false,
        git_protocol: None,
        gzip: gzipped,
        authorization: Some(format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("{WRITER}:{PASSWORD}")
            )
        )),
        api_key: None,
        secure: true,
        client_ip: "127.0.0.1".into(),
    };
    let mut out = Captured {
        status: 0,
        body: Vec::new(),
    };
    super::super::serve_http(&request, &mut &body[..], &mut out);
    assert_eq!(out.status, 200);
    String::from_utf8_lossy(&out.body).into_owned()
}

#[test]
fn capped_refuses_past_its_limit_and_passes_up_to_it() {
    let mut whole = Vec::new();
    Capped::new(&b"0123456789"[..], 10).read_to_end(&mut whole).unwrap();
    assert_eq!(whole, b"0123456789");
    let mut over = Vec::new();
    let err = Capped::new(&b"0123456789x"[..], 10).read_to_end(&mut over).unwrap_err();
    assert!(err.to_string().contains("over 10 bytes"), "{err}");
}

/// A gzip body of a few hundred bytes that inflates past `max-request` is refused, as inflated, and so is an SSH
/// command's input past it.
#[test]
fn a_gzip_body_and_ssh_input_are_held_to_max_request() {
    let fixture = fixture_with(
        "limits-request",
        std::path::PathBuf::from("."),
        r#"<git-server max-request="4096"/>"#,
    );
    define(&fixture, "lim", "db");
    let body = wants(2000);
    let gzipped = gzip(&body);
    assert!(gzipped.len() < 4096 && body.len() > 4096);
    let answer = upload_pack(&fixture, &gzipped, true);
    assert!(answer.contains("over 4096 bytes"), "{answer}");
    // under the limit the request is read whole (and its unknown wants refused)
    let answer = upload_pack(&fixture, &gzip(&wants(4)), true);
    assert!(!answer.contains("over 4096 bytes"), "{answer}");

    let context = fixture.context.factory().new_context();
    let _entered = context.enter();
    let mut errors = Vec::new();
    let status = super::super::serve_ssh(
        WRITER,
        "git-upload-pack 'lim'",
        None,
        &mut &body[..],
        &mut Vec::new(),
        &mut errors,
    );
    assert_ne!(status, 0);
    let errors = String::from_utf8_lossy(&errors);
    assert!(errors.contains("over 4096 bytes"), "{errors}");
}

/// An upload-pack request of more lines than `fetch-max-lines` is refused before they are kept.
#[test]
fn an_upload_pack_request_is_held_to_fetch_max_lines() {
    let fixture = fixture_with(
        "limits-lines",
        std::path::PathBuf::from("."),
        r#"<git-server fetch-max-lines="16"/>"#,
    );
    define(&fixture, "lim", "db");
    let answer = upload_pack(&fixture, &wants(17), false);
    assert!(answer.contains("GitFetchRequestTooLarge"), "{answer}");
    let answer = upload_pack(&fixture, &wants(16), false);
    assert!(!answer.contains("GitFetchRequestTooLarge"), "{answer}");
}
