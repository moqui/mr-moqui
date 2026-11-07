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

/// A ranged download's range as the authority answered it, checked again against the file (Shop F88): a part, the
/// whole file, or outside it, with the status and `Content-Range` the frontend answers.
#[test]
fn a_download_range_is_checked_against_the_file() {
    use serde_json::json;
    assert_eq!(Ranged::of(&serde_json::Value::Null, 10), Ranged::Whole);
    let part = Ranged::of(&json!({"start": 2, "end": 5}), 10);
    assert_eq!(
        part,
        Ranged::Part {
            start: 2,
            end: 5,
            complete: 10
        }
    );
    assert_eq!(
        (part.status(), part.content_range().as_deref()),
        (206, Some("bytes 2-5/10"))
    );
    // an end past the file is cut to its last byte
    assert_eq!(
        Ranged::of(&json!({"start": 8, "end": 99}), 10)
            .content_range()
            .as_deref(),
        Some("bytes 8-9/10")
    );
    for outside in [
        json!({"unsatisfiable": true, "size": 10}),
        json!({"start": 10, "end": 12}),
        json!({"start": 5, "end": 4}),
        json!({"start": "2"}),
    ] {
        let ranged = Ranged::of(&outside, 10);
        assert_eq!(ranged, Ranged::Unsatisfiable { complete: 10 }, "{outside}");
        assert_eq!(
            (ranged.status(), ranged.content_range().as_deref()),
            (416, Some("bytes */10"))
        );
    }
    assert_eq!((Ranged::Whole.status(), Ranged::Whole.content_range()), (200, None));
}
