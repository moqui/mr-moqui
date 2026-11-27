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

use std::io::{Read, Write};
use std::net::TcpListener;

use moqui_client::proxy::{Proxy, Request};

use super::blocking;

/// A stub server on a local port that answers one request `204`; its base URL.
fn stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let base = format!("http://{}", listener.local_addr().expect("its address"));
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    base
}

/// The helper run as a command runs it: awaited in a task on Tauri's multi-thread runtime, a worker's context.
fn on_runtime<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::block_on(async move {
        tauri::async_runtime::spawn(async move { blocking(work).await })
            .await
            .expect("the task ended")
    })
}

#[test]
fn a_blocking_request_answers_from_a_runtime_task() {
    let base = stub();
    // reqwest's blocking client panics in a debug build when it runs on a runtime worker: the helper runs it off one
    let answered = on_runtime(move || {
        let proxy = Proxy::new().map_err(|err| err.to_string())?;
        let request = Request {
            method: "GET".to_string(),
            path: "/".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        proxy
            .send_within(
                &base,
                None,
                moqui_client::SHELL_ORIGIN,
                request,
                Some(std::time::Duration::from_secs(10)),
            )
            .map(|upstream| upstream.status)
            .map_err(|err| err.to_string())
    });
    assert_eq!(answered, Ok(204));
}

#[test]
fn a_panic_in_the_work_answers_an_error() {
    let answered: Result<u16, String> = on_runtime(|| panic!("the work failed"));
    assert!(answered.is_err(), "a panic is an error: {answered:?}");
}
