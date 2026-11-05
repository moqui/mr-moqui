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

use super::*;

#[test]
fn a_sign_in_with_a_key_is_done() {
    let answer = json!({"loggedIn": true, "loginKey": "k123"});
    assert_eq!(signed_in(200, &answer, false).unwrap(), SignIn::Key("k123".to_string()));
}

#[test]
fn a_sign_in_with_no_key_is_refused_not_a_session() {
    for answer in [json!({"loggedIn": true}), json!({"loggedIn": true, "loginKey": ""})] {
        let err = signed_in(200, &answer, false).unwrap_err();
        assert_eq!(err.message(), NO_KEY);
    }
}

#[test]
fn a_second_factor_asks_for_its_code() {
    let answer = json!({
        "secondFactorRequired": true,
        "factorTypeDescriptions": ["Authenticator app", "Email"],
        "sendableFactors": [{"factorId": "F1", "factorOption": "j***@example.com"}, {"factorId": "F2"}, {"x": 1}],
    });
    let SignIn::Code(ask) = signed_in(200, &answer, false).unwrap() else {
        panic!("not a code ask");
    };
    assert_eq!(ask.text, "Enter the authentication code (Authenticator app, Email)");
    assert_eq!(
        ask.sendable,
        vec![
            Sendable {
                id: "F1".to_string(),
                label: "j***@example.com".to_string()
            },
            Sendable {
                id: "F2".to_string(),
                label: "F2".to_string()
            },
        ]
    );
    let SignIn::Code(again) = signed_in(200, &answer, true).unwrap() else {
        panic!("not a code ask");
    };
    assert_eq!(again.text, "Authentication code is not valid");
    let SignIn::Code(plain) = signed_in(200, &json!({"secondFactorRequired": true}), false).unwrap() else {
        panic!("not a code ask");
    };
    assert_eq!(
        plain,
        CodeAsk {
            text: "Enter the authentication code".to_string(),
            sendable: Vec::new()
        }
    );
}

#[test]
fn a_refused_sign_in_carries_the_servers_reason() {
    let err = signed_in(401, &json!({"errors": "Password incorrect"}), false).unwrap_err();
    assert_eq!(err.message(), "Password incorrect");
    let err = signed_in(401, &json!({"message": "Locked"}), false).unwrap_err();
    assert_eq!(err.message(), "Locked");
    let err = signed_in(401, &json!({"errors": ""}), false).unwrap_err();
    assert_eq!(err.message(), "HTTP 401");
    let err = signed_in(502, &serde_json::Value::Null, false).unwrap_err();
    assert_eq!(err.message(), "HTTP 502");
    // a 4xx that says it logged in is still refused
    assert!(signed_in(403, &json!({"loggedIn": true, "loginKey": "k"}), false).is_err());
}

#[test]
fn a_send_code_answer_is_refused_by_its_errors_or_status() {
    assert_eq!(send_refusal(200, &json!({})), None);
    assert_eq!(
        send_refusal(200, &json!({"errors": "No such factor"})).as_deref(),
        Some("No such factor")
    );
    assert_eq!(send_refusal(500, &serde_json::Value::Null).as_deref(), Some("HTTP 500"));
}
