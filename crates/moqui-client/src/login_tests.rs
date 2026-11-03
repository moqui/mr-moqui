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
fn a_sign_in_with_no_key_is_an_error_not_a_session() {
    assert!(signed_in(200, &json!({"loggedIn": true}), false).is_err());
}

#[test]
fn a_second_factor_asks_for_its_code() {
    let answer = json!({
        "secondFactorRequired": true,
        "factorTypeDescriptions": ["Authenticator app", "Email"],
        "sendableFactors": [{"factorId": "F1", "factorOption": "j***@example.com"}, {"factorId": "F2"}],
    });
    let SignIn::Code(ask) = signed_in(200, &answer, false).unwrap() else {
        panic!("not a code ask");
    };
    assert_eq!(ask.text, "Enter the authentication code (Authenticator app, Email)");
    assert_eq!(ask.sendable.len(), 2);
    assert_eq!(ask.sendable[0].label, "j***@example.com");
    assert_eq!(ask.sendable[1].label, "F2");
    let SignIn::Code(again) = signed_in(200, &answer, true).unwrap() else {
        panic!("not a code ask");
    };
    assert_eq!(again.text, "Authentication code is not valid");
}

#[test]
fn a_refused_sign_in_carries_the_servers_reason() {
    let err = signed_in(401, &json!({"errors": "Password incorrect"}), false).unwrap_err();
    assert!(err.to_string().contains("Password incorrect"), "{err}");
    let err = signed_in(502, &serde_json::Value::Null, false).unwrap_err();
    assert!(err.to_string().contains("HTTP 502"), "{err}");
}
