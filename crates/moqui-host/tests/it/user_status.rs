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

//! The first heartbeats and status changes of a user with no `UserStatus` row yet, at once (a page load's heartbeat
//! beside a status change, or two tabs): each answers 200 and there is one row, rather than one of them failing on the
//! row's primary key (`ChatServices.heartbeat#UserStatus` and `set#UserStatus` lock the `UserAccount` row before they
//! create it).

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::net::SocketAddr;
use std::sync::{Arc, Barrier};

use moqui_lib::object::Value;

use crate::common::{exchange, header, http_request, merge_cookies, session_token, sign_in};

/// Racing rounds; each starts with the row gone.
const ROUNDS: usize = 5;
/// The requests of a round, half heartbeats and half status changes, over two sessions.
const REQUESTS: usize = 8;

#[test]
fn racing_first_heartbeats_and_status_changes_all_succeed() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start("user-status-race");
    let address = gate.server.address();
    let factory = gate.server.factory();
    let sessions: Vec<(String, String)> = (0..2).map(|_| browser(address)).collect();
    let user_id = {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let row = factory
            .entity()
            .unwrap()
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("username", Value::string("john.doe"))
            .one()
            .unwrap()
            .unwrap();
        row.get("userId").and_then(Value::as_str).unwrap().to_string()
    };
    let take_rows = || {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let facade = factory.entity().unwrap();
        let rows = facade
            .find("moqui.security.UserStatus")
            .unwrap()
            .eq("userId", Value::string(user_id.clone()))
            .list()
            .unwrap();
        for row in rows.iter() {
            row.delete(&facade).unwrap();
        }
        rows.len()
    };
    for round in 0..ROUNDS {
        take_rows();
        let barrier = Arc::new(Barrier::new(REQUESTS));
        let threads: Vec<_> = (0..REQUESTS)
            .map(|index| {
                let (cookie, token) = sessions[index % 2].clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let (path, body) = if index % 4 < 2 {
                        ("/userStatusHeartbeat", "idleSeconds=0")
                    } else {
                        ("/setUserStatus", "status=online")
                    };
                    let request = http_request(
                        "POST",
                        path,
                        &[
                            ("Cookie", cookie.as_str()),
                            ("Accept", "application/json"),
                            ("Content-Type", "application/x-www-form-urlencoded"),
                            ("X-CSRF-Token", token.as_str()),
                        ],
                        body,
                    );
                    barrier.wait();
                    let (status, _, text) = exchange(address, &request);
                    (path, status, text)
                })
            })
            .collect();
        for thread in threads {
            let (path, status, text) = thread.join().unwrap();
            assert_eq!(status, 200, "round {round}: {path} answered {status}: {text}");
            assert!(text.contains("\"myStatus\""), "round {round}: {path}: {text}");
            assert!(!text.contains("\"myStatus\":null"), "round {round}: {path}: {text}");
        }
        assert_eq!(take_rows(), 1, "round {round}: one UserStatus row");
    }
}

/// A browser session of john.doe: its cookie and the session token of a page.
fn browser(address: SocketAddr) -> (String, String) {
    let cookie = sign_in(address, "john.doe", "moqui");
    let (status, headers, html) = exchange(
        address,
        &http_request("GET", "/qapps", &[("Cookie", &cookie), ("Accept", "text/html")], ""),
    );
    let token = session_token(&html)
        .or_else(|| header(&headers, "moquiSessionToken"))
        .unwrap_or_else(|| {
            panic!(
                "no session token: {status} {}",
                html.chars().take(500).collect::<String>()
            )
        });
    (merge_cookies(&cookie, &headers), token)
}
