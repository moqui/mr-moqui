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

use std::cell::RefCell;

use super::*;

/// The RSA test key of the QZ signer, as a service account's private_key.
const KEY: &str = include_str!("../../../moqui-lib/testdata/qz-test-key-pkcs8.pem");
const TOKEN_URI: &str = "https://oauth2.fcm-test.invalid/token";
const BASE_URL: &str = "https://fcm.fcm-test.invalid";
const NOW: i64 = 1_800_000_000;

fn account() -> ServiceAccount {
    let json = serde_json::json!({
        "type": "service_account",
        "project_id": "demo-project",
        "client_email": "push@demo-project.iam.gserviceaccount.com",
        "private_key": KEY,
        "token_uri": TOKEN_URI,
    });
    ServiceAccount::parse(&json.to_string()).unwrap()
}

fn config() -> Config {
    Config {
        account: account(),
        project_id: "demo-project".to_string(),
        base_url: BASE_URL.to_string(),
    }
}

fn response(status: u16, body: &str) -> Response {
    Response {
        status,
        body: body.to_string(),
        ..Response::default()
    }
}

fn token_answer(token: &str) -> Response {
    response(
        200,
        &format!(r#"{{"access_token":"{token}","expires_in":3599,"token_type":"Bearer"}}"#),
    )
}

/// A stand-in for the token endpoint and FCM: the token endpoint answers `ya29.<n>` for its n-th request, and FCM the
/// answers given, in order, the last one again when they run out. Each request is kept.
struct Mock {
    fcm_answers: Vec<Response>,
    requests: RefCell<Vec<Request>>,
}

impl Mock {
    fn new(fcm_answers: Vec<Response>) -> Self {
        Self {
            fcm_answers,
            requests: RefCell::new(Vec::new()),
        }
    }

    fn send(&self, request: Request) -> Result<Response, Error> {
        let mut requests = self.requests.borrow_mut();
        requests.push(request.clone());
        if request.url == TOKEN_URI {
            let issued = requests.iter().filter(|sent| sent.url == TOKEN_URI).count();
            return Ok(token_answer(&format!("ya29.{issued}")));
        }
        let sent = requests.iter().filter(|sent| sent.url != TOKEN_URI).count();
        let answer = self.fcm_answers.get(sent - 1).or(self.fcm_answers.last());
        Ok(answer.cloned().unwrap_or_else(|| response(200, "{}")))
    }

    fn count(&self, url: &str) -> usize {
        self.requests.borrow().iter().filter(|sent| sent.url == url).count()
    }
}

fn deliver(mock: &Mock, tokens: &Tokens) -> Outcome {
    deliver_until(mock, tokens, None)
}

fn deliver_until(mock: &Mock, tokens: &Tokens, deadline: Option<i64>) -> Outcome {
    let config = config();
    let send = |request| mock.send(request);
    let sender = Sender {
        config: &config,
        tokens,
        send: &send,
        backoff: Duration::ZERO,
    };
    sender.deliver("100", &message("device-token", &Notice::default()), NOW, deadline)
}

fn segment(jwt: &str, index: usize) -> serde_json::Value {
    let part = jwt.split('.').nth(index).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[test]
fn the_jwt_asks_for_the_messaging_scope_at_the_token_endpoint_and_is_signed_rs256() {
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::signature::Verifier;

    let jwt = jwt(&account(), NOW).unwrap();
    assert_eq!(jwt.split('.').count(), 3, "{jwt}");
    assert_eq!(segment(&jwt, 0), serde_json::json!({"alg": "RS256", "typ": "JWT"}));
    let claims = segment(&jwt, 1);
    assert_eq!(claims["iss"], "push@demo-project.iam.gserviceaccount.com");
    assert_eq!(claims["scope"], "https://www.googleapis.com/auth/firebase.messaging");
    assert_eq!(claims["aud"], TOKEN_URI);
    assert_eq!(claims["iat"], NOW);
    assert_eq!(claims["exp"], NOW + 3600);

    let (signing_input, signature) = jwt.rsplit_once('.').unwrap();
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(KEY).unwrap();
    let verifying = rsa::pkcs1v15::VerifyingKey::<rsa::sha2::Sha256>::new(rsa::RsaPublicKey::from(&key));
    let signature = URL_SAFE_NO_PAD.decode(signature).unwrap();
    let signature = rsa::pkcs1v15::Signature::try_from(signature.as_slice()).unwrap();
    verifying
        .verify(signing_input.as_bytes(), &signature)
        .expect("the signature verifies with the account's public key");
}

#[test]
fn a_service_account_needs_its_email_and_key_and_defaults_the_token_endpoint() {
    let parsed = ServiceAccount::parse(r#"{"client_email":"a@b","private_key":"k","project_id":"p"}"#).unwrap();
    assert_eq!(parsed.token_uri, "https://oauth2.googleapis.com/token");
    assert_eq!(parsed.project_id, "p");
    assert!(ServiceAccount::parse(r#"{"client_email":"a@b"}"#).is_err());
    assert!(ServiceAccount::parse("not json").is_err());
}

#[test]
fn the_access_token_is_kept_until_shortly_before_it_expires() {
    let mock = Mock::new(Vec::new());
    let send = |request| mock.send(request);
    let tokens = Tokens::new();
    let account = account();
    assert_eq!(tokens.get(&account, NOW, &send).unwrap(), "ya29.1");
    // the same token for the next hour less the margin
    assert_eq!(tokens.get(&account, NOW + 3000, &send).unwrap(), "ya29.1");
    assert_eq!(mock.count(TOKEN_URI), 1);
    // five minutes before it expires a new one is taken
    assert_eq!(tokens.get(&account, NOW + 3599 - 299, &send).unwrap(), "ya29.2");
    assert_eq!(mock.count(TOKEN_URI), 2);
    // another account in the properties takes its own
    let mut other = account.clone();
    other.client_email = "other@demo-project.iam.gserviceaccount.com".to_string();
    assert_eq!(tokens.get(&other, NOW + 3599 - 299, &send).unwrap(), "ya29.3");

    let request = &mock.requests.borrow()[0];
    assert_eq!(request.method, Method::Post);
    let body = request.body.as_deref().unwrap();
    assert!(
        body.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="),
        "{body}"
    );
}

#[test]
fn a_refused_token_request_is_an_error_without_the_answer_body() {
    let tokens = Tokens::new();
    let send = |_request| {
        Ok(response(
            400,
            r#"{"error":"invalid_grant","error_description":"Invalid JWT Signature."}"#,
        ))
    };
    let err = tokens.get(&account(), NOW, &send).unwrap_err();
    assert!(err.contains("400") && err.contains("invalid_grant"), "{err}");
}

#[test]
fn a_ring_is_a_high_priority_data_message_with_the_call_and_its_ring_time() {
    let notice = Notice {
        title: "Call from Ann".to_string(),
        link: "/qapps/comms/Calls?callSessionId=CS1".to_string(),
        topic: "IncomingCall".to_string(),
        tag: "call-CS1".to_string(),
        kind: Kind::Ring(Ring {
            call_session_id: "CS1".to_string(),
            chat_room_id: "ROOM1".to_string(),
            caller: "Ann".to_string(),
            video: true,
            expires_at: 1_800_000_045_000,
        }),
        high: true,
        ttl: 45,
    };
    let body = message("device-token", &notice);
    assert_eq!(
        body,
        serde_json::json!({"message": {
            "token": "device-token",
            "data": {
                "kind": "ring", "title": "Call from Ann", "link": "/qapps/comms/Calls?callSessionId=CS1",
                "topic": "IncomingCall", "tag": "call-CS1", "callSessionId": "CS1", "chatRoomId": "ROOM1",
                "caller": "Ann", "video": "true", "expiresAt": "1800000045000",
            },
            "android": {"priority": "HIGH", "ttl": "45s", "collapse_key": "call-CS1"},
        }})
    );
    // FCM takes strings only in data, and 4 KB in all
    assert!(body["message"]["data"]
        .as_object()
        .unwrap()
        .values()
        .all(serde_json::Value::is_string));
    assert!(body.to_string().len() < 4096);

    let plain = message(
        "t",
        &Notice {
            ttl: 86_400,
            ..Notice::default()
        },
    );
    assert_eq!(plain["message"]["data"]["kind"], "notification");
    assert_eq!(
        plain["message"]["android"],
        serde_json::json!({"priority": "NORMAL", "ttl": "86400s"})
    );
    assert!(plain["message"]["data"].get("callSessionId").is_none());
}

#[test]
fn a_message_goes_to_the_project_with_the_access_token() {
    let mock = Mock::new(vec![response(200, r#"{"name":"projects/demo-project/messages/1"}"#)]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Delivered);
    let requests = mock.requests.borrow();
    let sent = requests.last().unwrap();
    assert_eq!(
        sent.url,
        "https://fcm.fcm-test.invalid/v1/projects/demo-project/messages:send"
    );
    assert!(sent
        .headers
        .contains(&("authorization".to_string(), "Bearer ya29.1".to_string())));
    let body: serde_json::Value = serde_json::from_str(sent.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["message"]["token"], "device-token");
}

/// Every kind of message, with every field set.
fn every_kind() -> Vec<Notice> {
    let base = Notice {
        title: "t".to_string(),
        link: "/l".to_string(),
        topic: "IncomingCall".to_string(),
        tag: "call-CS1".to_string(),
        high: true,
        ttl: 45,
        ..Notice::default()
    };
    let ring = Ring {
        call_session_id: "CS1".to_string(),
        chat_room_id: "R".to_string(),
        caller: "Ann".to_string(),
        video: false,
        expires_at: 1,
    };
    vec![
        base.clone(),
        Notice {
            kind: Kind::Ring(ring),
            ..base.clone()
        },
        Notice {
            kind: Kind::RingEnded {
                call_session_id: "CS1".to_string(),
                reason: "answered".to_string(),
            },
            ..base
        },
    ]
}

#[test]
fn no_message_has_a_key_fcm_reserves() {
    assert!(reserved_key("from") && reserved_key("message_type") && reserved_key("notification"));
    assert!(reserved_key("google.c.a.e") && reserved_key("gcm.notification.title"));
    assert!(!reserved_key("caller") && !reserved_key("kind"));
    for notice in every_kind() {
        let body = message("t", &notice);
        let data = body["message"]["data"].as_object().unwrap();
        let reserved: Vec<&String> = data.keys().filter(|key| reserved_key(key)).collect();
        assert!(reserved.is_empty(), "{reserved:?} in {data:?}");
        assert!(data.values().all(serde_json::Value::is_string), "{data:?}");
    }
}

#[test]
fn a_device_hears_its_registration_and_how_long_a_ring_rings() {
    let ring = Notice {
        kind: Kind::Ring(Ring {
            call_session_id: "CS1".to_string(),
            expires_at: 1_700_000_045_000,
            ..Ring::default()
        }),
        ..Notice::default()
    };
    let body = device_message("t", "100050", &ring, 1_700_000_000);
    assert_eq!(body["message"]["data"]["pushId"], "100050");
    assert_eq!(body["message"]["data"]["ringSeconds"], "45");
    // past its end, none left; a ring's end has the registration but no time
    let late = device_message("t", "100050", &ring, 1_700_000_100);
    assert_eq!(late["message"]["data"]["ringSeconds"], "0");
    let ended = device_message("t", "100050", &every_kind()[2], 1_700_000_000);
    assert_eq!(ended["message"]["data"]["pushId"], "100050");
    assert!(ended["message"]["data"].get("ringSeconds").is_none());
}

#[test]
fn a_ring_end_says_which_call_and_why_and_replaces_its_ring() {
    let ended = &every_kind()[2];
    let body = message("t", ended);
    assert_eq!(
        body["message"]["data"],
        serde_json::json!({"kind": "ringEnded", "topic": "IncomingCall", "tag": "call-CS1", "callSessionId": "CS1",
            "reason": "answered"})
    );
    assert_eq!(body["message"]["android"]["collapse_key"], "call-CS1");
    assert_eq!(body["message"]["android"]["priority"], "HIGH");
}

#[test]
fn the_data_is_cut_to_its_byte_budget_on_character_boundaries() {
    assert_eq!(cut_bytes("abc", 5), "abc");
    // é is two bytes: three bytes keep one of them
    assert_eq!(cut_bytes("éé", 3), "é");
    let notice = Notice {
        title: "é".repeat(2000),
        // quotes double in JSON
        link: "\"".repeat(3000),
        kind: Kind::Ring(Ring {
            caller: "名".repeat(1000),
            ..Ring::default()
        }),
        ..Notice::default()
    };
    let body = message("t", &notice);
    let data = body["message"]["data"].to_string();
    assert!(data.len() <= MAX_DATA_BYTES, "{}", data.len());
    assert!(body.to_string().len() < 4096);
    let title = body["message"]["data"]["title"].as_str().unwrap();
    assert!(!title.is_empty() && title.chars().all(|c| c == 'é'));
}

#[test]
fn unregistered_or_invalid_argument_means_the_token_is_gone() {
    let unregistered = r#"{"error":{"code":404,"message":"Requested entity was not found.","status":"NOT_FOUND",
        "details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError","errorCode":"UNREGISTERED"}]}}"#;
    let bad_token = r#"{"error":{"code":400,"message":"The registration token is not a valid FCM registration token",
        "status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError",
        "errorCode":"INVALID_ARGUMENT"},{"@type":"type.googleapis.com/google.rpc.BadRequest",
        "fieldViolations":[{"field":"message.token","description":"Invalid registration token"}]}]}}"#;
    // a 400 about the message itself, such as a reserved data key: every 400 is INVALID_ARGUMENT
    let bad_message = r#"{"error":{"code":400,"message":"Invalid data payload key: from","status":"INVALID_ARGUMENT",
        "details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError","errorCode":"INVALID_ARGUMENT"},
        {"@type":"type.googleapis.com/google.rpc.BadRequest","fieldViolations":[{"field":"message.data[0].key"}]}]}}"#;
    let mock = Mock::new(vec![response(404, unregistered)]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Gone);
    assert_eq!(mock.count(&config().send_url()), 1, "not tried again");
    assert_eq!(
        deliver(&Mock::new(vec![response(404, "")]), &Tokens::new()),
        Outcome::Gone
    );
    assert_eq!(
        deliver(&Mock::new(vec![response(400, bad_token)]), &Tokens::new()),
        Outcome::Gone
    );
    assert_eq!(
        deliver(&Mock::new(vec![response(400, bad_message)]), &Tokens::new()),
        Outcome::Rejected
    );
    assert_eq!(
        deliver(&Mock::new(vec![response(400, "")]), &Tokens::new()),
        Outcome::Rejected
    );
    // the token is of another Firebase project's app
    let mismatch =
        r#"{"error":{"code":403,"status":"PERMISSION_DENIED","details":[{"errorCode":"SENDER_ID_MISMATCH"}]}}"#;
    assert_eq!(
        deliver(&Mock::new(vec![response(403, mismatch)]), &Tokens::new()),
        Outcome::Gone
    );
    // a 403 with no per-token code is the server's own trouble: the device is left alone
    let denied = r#"{"error":{"code":403,"status":"PERMISSION_DENIED"}}"#;
    assert_eq!(
        deliver(&Mock::new(vec![response(403, denied)]), &Tokens::new()),
        Outcome::Unsent
    );
}

#[test]
fn a_429_or_5xx_is_tried_again_a_few_times() {
    let mock = Mock::new(vec![response(503, ""), response(200, "{}")]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Delivered);
    assert_eq!(mock.count(&config().send_url()), 2);
    assert_eq!(mock.count(TOKEN_URI), 1, "the token is kept between tries");

    // still refused after the tries: FCM is down, and no device is counted against
    let mock = Mock::new(vec![response(429, "")]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Unsent);
    assert_eq!(mock.count(&config().send_url()), 3);
}

#[test]
fn a_ring_is_not_tried_again_past_its_end() {
    let mock = Mock::new(vec![response(503, ""), response(200, "{}")]);
    assert_eq!(deliver_until(&mock, &Tokens::new(), Some(0)), Outcome::Unsent);
    assert_eq!(mock.count(&config().send_url()), 1);
}

#[test]
fn a_failed_token_request_is_not_repeated_for_half_a_minute() {
    let tokens = Tokens::new();
    let asked = RefCell::new(0);
    let send = |_request: Request| {
        *asked.borrow_mut() += 1;
        Ok(response(503, ""))
    };
    let account = account();
    assert!(tokens.get(&account, NOW, &send).is_err());
    assert!(tokens.get(&account, NOW + 29, &send).is_err());
    assert_eq!(*asked.borrow(), 1);
    assert!(tokens.get(&account, NOW + 30, &send).is_err());
    assert_eq!(*asked.borrow(), 2);
}

#[test]
fn a_refused_access_token_is_taken_again_once() {
    let mock = Mock::new(vec![response(401, ""), response(200, "{}")]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Delivered);
    assert_eq!(mock.count(TOKEN_URI), 2);
    let requests = mock.requests.borrow();
    assert!(requests
        .last()
        .unwrap()
        .headers
        .contains(&("authorization".to_string(), "Bearer ya29.2".to_string())));
    drop(requests);

    let mock = Mock::new(vec![response(401, "")]);
    assert_eq!(deliver(&mock, &Tokens::new()), Outcome::Unsent);
    assert_eq!(mock.count(TOKEN_URI), 2);
}

#[test]
fn no_access_token_sends_nothing_and_leaves_the_device_alone() {
    let config = config();
    let tokens = Tokens::new();
    let sent = RefCell::new(0);
    let send = |_request: Request| {
        *sent.borrow_mut() += 1;
        Err(Error::new("connection refused"))
    };
    let sender = Sender {
        config: &config,
        tokens: &tokens,
        send: &send,
        backoff: Duration::ZERO,
    };
    assert_eq!(
        sender.deliver("100", &message("t", &Notice::default()), NOW, None),
        Outcome::Unsent
    );
    assert_eq!(*sent.borrow(), 1, "only the token endpoint was tried");
}
