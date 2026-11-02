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

use super::*;

#[test]
fn an_envelope_round_trips_and_a_missing_status_is_its_kinds() {
    let error = PassError {
        code: "Unprocessable".to_string(),
        message: "Invalid request fields".to_string(),
        field_errors: vec![FieldError {
            field: "title".to_string(),
            message: "required".to_string(),
        }],
        correlation_id: "abc".to_string(),
        status: 422,
    };
    assert_eq!(PassError::from_json(&error.to_json()), error);
    let bare = PassError::from_json(&serde_json::json!({"code": "Tarpit", "message": "Too many requests"}));
    assert_eq!(bare.status, 429);
    assert!(bare.field_errors.is_empty());
    // no code is the authority being unavailable; a status out of range is the kind's
    let odd = PassError::from_json(&serde_json::json!({"status": 200}));
    assert_eq!((odd.code.as_str(), odd.status), ("Unavailable", 503));
}

#[test]
fn each_kind_has_the_rest_apis_status() {
    for (code, status) in [
        ("AuthenticationRequired", 401),
        ("Unauthorized", 403),
        ("NotFound", 404),
        ("Conflict", 409),
        ("Unprocessable", 422),
        ("Tarpit", 429),
        ("Unavailable", 503),
        ("Anything", 503),
    ] {
        assert_eq!(status_of(code), status, "{code}");
    }
}

#[test]
fn a_reply_for_a_script_has_a_result_or_an_error_and_the_handle() {
    let ok = Reply {
        result: Ok(serde_json::json!({"site": {"siteId": "S"}})),
        session_handle: "h.mac".to_string(),
    };
    assert_eq!(
        reply_json(&ok),
        serde_json::json!({"result": {"site": {"siteId": "S"}}, "sessionHandle": "h.mac"})
    );
    let refused = Reply::error(PassError::unavailable("local node offline"));
    let json = reply_json(&refused);
    assert_eq!(json["error"]["code"], "Unavailable");
    assert_eq!(json["error"]["status"], 503);
    assert!(json.get("result").is_none());
}

#[test]
fn with_no_sender_installed_a_call_is_not_built() {
    // the framework's own tests install none, as Lite and Dev do not
    let Err(err) = sender() else {
        panic!("no sender is installed in moqui-core's tests")
    };
    assert_eq!(err.message(), NOT_BUILT);
}
