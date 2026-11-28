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

use serde_json::json;

use super::{answer_lines, listener_lines, listener_request};

#[test]
fn a_listener_command_names_its_state_and_its_options() {
    let off = listener_request("HTTPS", "off", &["--now", "--reason", "incident 12", "--no-persist"]).unwrap();
    assert_eq!(
        off,
        json!({"command": "listener", "name": "HTTPS", "enabled": false, "now": true, "reason": "incident 12",
            "persist": false})
    );
    let moved = listener_request(
        "SSH",
        "on",
        &[
            "--port",
            "2223",
            "--host",
            "0.0.0.0",
            "--access",
            "key",
            "--confirm-dependents",
            "--confirm-public-http",
        ],
    )
    .unwrap();
    assert_eq!(
        moved,
        json!({"command": "listener", "name": "SSH", "enabled": true, "port": "2223", "host": "0.0.0.0",
            "access": "key", "confirm_dependents": true, "confirm_public_http": true})
    );
    assert!(listener_request("SSH", "maybe", &[]).is_err(), "on or off");
    assert!(listener_request("SSH", "on", &["--port"]).is_err(), "a value missing");
    assert!(
        listener_request("SSH", "on", &["--confirm-lockout"]).is_err(),
        "no lockout guard to confirm"
    );
}

#[test]
fn the_listeners_and_an_answer_print_a_line_each_and_what_else_they_say() {
    let rows = json!([
        {"name": "HTTPS", "state": "on", "address": "0.0.0.0:8443", "detail": "tcp 0.0.0.0:8443 TLS",
            "since": "2026-11-27T10:00:00Z", "cause": "boot", "connections": 3, "pending": null, "lastError": null},
        {"name": "SSH", "state": "off", "address": null, "detail": "ssh-server@enabled is false",
            "cause": "request by operator socket (uid 0): incident", "lastError": "2222 in use\u{1b}[2J"},
    ]);
    assert_eq!(
        listener_lines(&rows),
        [
            "HTTPS        on           0.0.0.0:8443",
            "    tcp 0.0.0.0:8443 TLS",
            "    since 2026-11-27T10:00:00Z",
            "    by boot",
            "    connections 3",
            "SSH          off",
            "    ssh-server@enabled is false",
            "    by request by operator socket (uid 0): incident",
            "    last error: 2222 in use?[2J",
        ]
    );
    let answer = json!({
        "message": "HTTP set off; stopped",
        "settingList": [{"selector": "webapp-list/webapp[webroot]@http-listen-enabled", "value": "false",
            "note": "kept in runtime/.env; it applies now"}],
        "outcomeList": [{"name": "HTTP", "action": "stopped", "detail": ""}],
        "noteList": ["No way in over the network is left. The ways back: ..."],
    });
    assert_eq!(
        answer_lines(&answer),
        [
            "HTTP set off; stopped",
            "  webapp-list/webapp[webroot]@http-listen-enabled = false: kept in runtime/.env; it applies now",
            "  HTTP stopped",
            "  No way in over the network is left. The ways back: ...",
        ]
    );
}
