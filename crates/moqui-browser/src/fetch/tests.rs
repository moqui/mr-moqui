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

//! The browser host over a fake outbound sender: remote-rest results, and rollback of the caller's
//! write when the remote call fails. The real `fetch` path is `cargo test -p moqui-host --test it -- wasm_browser`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moqui_core::outbound::{self, HttpSender, Request, Response};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::Busy;
use crate::host::Browser;

/// Answers `/echo` with JSON and anything else with a transport error. Records each URL.
struct Fake {
    urls: Mutex<Vec<String>>,
}

impl HttpSender for Fake {
    fn send(&self, request: Request) -> Result<Response, Error> {
        self.urls.lock().unwrap().push(request.url.clone());
        if request.url.contains("/echo") {
            Ok(Response {
                status: 200,
                body: format!(r#"{{"echo":"ok","method":"{}","note":"n"}}"#, request.method),
                ..Default::default()
            })
        } else {
            Err(Error::new("connection refused"))
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn param(name: &str, value: &str) -> (String, Value) {
    (name.to_string(), Value::string(value))
}

#[test]
fn remote_rest_runs_on_the_installed_sender_and_a_failure_rolls_back_the_write() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = std::env::temp_dir().join(format!("moqui-browser-fetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);
    copy_dir(&crate_dir.join("runtime"), &runtime);
    let fake = Arc::new(Fake {
        urls: Mutex::new(Vec::new()),
    });
    outbound::install_http_sender(fake.clone());
    let browser = Browser::open(
        &runtime,
        &crate_dir.join("../../runtime/base-component/framework"),
        ":memory:",
    )
    .unwrap();
    browser.check_tables(&[]).unwrap();

    let echoed = browser
        .call_service(
            "moqui.browser.SmokeServices.get#Echo",
            &BTreeMap::from([param("port", "8080"), param("note", "hi")]),
        )
        .unwrap();
    assert_eq!(echoed.get("echo").and_then(Value::as_str), Some("ok"));
    assert_eq!(echoed.get("method").and_then(Value::as_str), Some("GET"));

    let stored = browser
        .call_service(
            "moqui.browser.SmokeServices.store#ThenFetch",
            &BTreeMap::from([param("testId", "KEEP"), param("port", "8080")]),
        )
        .unwrap();
    assert_eq!(stored.get("echo").and_then(Value::as_str), Some("ok"));

    let failed = browser.call_service(
        "moqui.browser.SmokeServices.store#ThenFetch",
        &BTreeMap::from([param("testId", "GONE"), param("port", "8080"), param("path", "down")]),
    );
    assert!(failed.is_err(), "{failed:?}");

    let kept = browser
        .find(
            "moqui.test.TestEntity",
            &BTreeMap::from([param("testId", "KEEP")]),
            &[],
            None,
        )
        .unwrap();
    let gone = browser
        .find(
            "moqui.test.TestEntity",
            &BTreeMap::from([param("testId", "GONE")]),
            &[],
            None,
        )
        .unwrap();
    outbound::clear_http_sender();
    drop(browser);
    let _ = std::fs::remove_dir_all(&runtime);
    assert_eq!(kept.len(), 1);
    assert!(gone.is_empty(), "the failed call's write was not rolled back");
    let urls = fake.urls.lock().unwrap();
    assert!(
        urls.contains(&"http://127.0.0.1:8080/echo?note=hi&path=echo&port=8080".to_string()),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|url| url.starts_with("http://127.0.0.1:8080/down")),
        "{urls:?}"
    );
}

#[test]
fn a_second_call_while_one_is_in_progress_is_refused() {
    let first = Busy::enter().unwrap();
    assert!(Busy::enter().is_err());
    drop(first);
    assert!(Busy::enter().is_ok());
}
