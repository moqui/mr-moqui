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
fn a_call_needs_an_operation_and_an_input_object() {
    let message = |input: &str| PassCall {
        operation: "getSiteInfo".to_string(),
        site_id: "S".to_string(),
        input_json: input.as_bytes().to_vec(),
        ..PassCall::default()
    };
    let call = call_from(message(r#"{"hostname":"h"}"#)).unwrap();
    assert_eq!(call.input["hostname"], "h");
    assert_eq!(call_from(message("")).unwrap().input, serde_json::json!({}));
    assert_eq!(
        call_from(message("[1]")).unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        call_from(message("{")).unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let none = PassCall {
        operation: String::new(),
        ..message("{}")
    };
    assert_eq!(call_from(none).unwrap_err().code(), tonic::Code::InvalidArgument);
    // and back, as the sender sends it
    let back = call_from(call_message(&call)).unwrap();
    assert_eq!(back, call);
}

#[test]
fn a_reply_and_its_error_envelope_cross_as_sent() {
    let reply = Reply {
        result: Ok(serde_json::json!({"credential": "c", "subject": {"handle": "h"}})),
        session_handle: "id.mac".to_string(),
    };
    assert_eq!(reply_from(result_message(reply.clone())), reply);
    let error = core::PassError {
        code: "Conflict".to_string(),
        message: "State changed; reload and compare".to_string(),
        field_errors: vec![core::FieldError {
            field: "expectedRevision".to_string(),
            message: "stale".to_string(),
        }],
        correlation_id: "abc".to_string(),
        status: 409,
    };
    let refused = Reply::error(error.clone());
    assert_eq!(reply_from(result_message(refused)).result, Err(error));
    // a status the message leaves out is the kind's; a result that is not JSON is the authority unavailable
    let bare = reply_from(PassResult {
        error: Some(PassError {
            code: "Tarpit".to_string(),
            ..PassError::default()
        }),
        ..PassResult::default()
    });
    assert_eq!(bare.result.unwrap_err().status, 429);
    let garbled = reply_from(PassResult {
        result_json: b"not json".to_vec(),
        ..PassResult::default()
    });
    assert_eq!(garbled.result.unwrap_err().status, 503);
}

#[test]
fn a_direct_call_names_a_client_and_its_secret_and_the_link_is_its_own_client() {
    let direct = Authority {
        factory: Weak::new(),
        mode: Mode::Direct,
        failures: Mutex::new(HashMap::new()),
    };
    let mut metadata = tonic::metadata::MetadataMap::new();
    assert_eq!(
        direct.client(&metadata, "").unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    metadata.insert(SECRET_METADATA, "s3cret".parse().unwrap());
    // the client from the call when the metadata names none
    assert_eq!(
        direct.client(&metadata, "frontend").unwrap(),
        Client::Secret {
            id: "frontend".to_string(),
            secret: "s3cret".to_string()
        }
    );
    metadata.insert(CLIENT_METADATA, "named".parse().unwrap());
    assert_eq!(
        direct.client(&metadata, "frontend").unwrap(),
        Client::Secret {
            id: "named".to_string(),
            secret: "s3cret".to_string()
        }
    );
    metadata.remove(SECRET_METADATA);
    assert_eq!(
        direct.client(&metadata, "frontend").unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    let link = Authority {
        factory: Weak::new(),
        mode: Mode::Link,
        failures: Mutex::new(HashMap::new()),
    };
    // on the link a client and secret in the call or metadata change nothing
    assert_eq!(link.client(&metadata, "frontend").unwrap(), Client::Link);
}

#[test]
fn wrong_secrets_from_one_address_are_refused_until_the_window_ends() {
    let mut failures = HashMap::new();
    let here: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
    let there: Option<IpAddr> = Some("198.51.100.1".parse().unwrap());
    let start = Instant::now();
    for n in 1..FAILURE_LIMIT {
        assert_eq!(record_failure(&mut failures, here, start), n);
        assert!(!throttled(&failures, here, start));
    }
    assert_eq!(record_failure(&mut failures, here, start), FAILURE_LIMIT);
    assert!(throttled(&failures, here, start));
    // another address is not held, and the window ends
    assert!(!throttled(&failures, there, start));
    let later = start + FAILURE_WINDOW;
    assert!(!throttled(&failures, here, later));
    assert_eq!(record_failure(&mut failures, here, later), 1);
}
