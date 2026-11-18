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

//! The `sms-twilio` adapter (providers/twilio.rs) on the SMS test bed (`crate::tests`) against a fake Twilio on
//! loopback: form-encoded sends with Basic auth and their status callbacks in any order; inbound texts and files
//! fetched with Basic auth, never sent on to another host; Twilio's refusals (an opt-out marked on the pair, a bad
//! number, a rate tried again) and a lost answer; an opt-out keyword once; the signature over the URL (good, bad,
//! missing, replayed, the default port either way, a `webhookUrl`, a query string, a JSON body's `bodySHA256`); a
//! request an edge node queued, verified on the local node; and a group text through a fake Conversations API.

use super::*;
use crate::providers::twilio::{self, Twilio};
use crate::sms::SmsProvider as _;
use moqui_core::service::chat_bridge::ChatBridgeAdapter as _;

pub(super) const ACCOUNT: &str = concat!("AC", "0123456789abcdef", "0123456789abcdef");
pub(super) const API_KEY: &str = concat!("SK", "0123456789abcdef", "0123456789abcdef");
pub(super) const TOKEN: &str = "twilio-auth-token";
const WEBHOOK: &str = "https://moqui.example.test/channels/SMS1/webhook";

/// A Twilio SID of `prefix` and number `n`.
pub(super) fn sid(prefix: &str, n: u64) -> String {
    format!("{prefix}{n:032x}")
}

const TWILIO: Provider = Provider {
    adapter_name: "sms-twilio",
    bridge_name: "Twilio",
    config: |base, _keys| {
        json!({"accountSid": ACCOUNT, "apiKeySid": API_KEY, "apiKeySecret": "key-secret", "authToken": TOKEN,
            "apiBaseUrl": base, "conversationsBaseUrl": base, "mediaContentBaseUrl": base,
            "allowInternalAddresses": true, "maxMediaCount": 3, "maxMediaBytes": 2000})
    },
    accepted: |n, path| {
        let prefix = if path.contains("/Messages.json") {
            "SM"
        } else if path.ends_with("/Participants") {
            "MB"
        } else if path.ends_with("/Webhooks") {
            "WH"
        } else if path.ends_with("/Messages") {
            "IM"
        } else {
            "CH"
        };
        Reply::Status(201, json!({"sid": sid(prefix, n), "status": "queued"}).to_string())
    },
    sign: |_keys, body, _at_seconds, url| {
        let pairs = twilio::Form::parse(body).map(|form| form.pairs).unwrap_or_default();
        vec![
            ("x-twilio-signature".to_string(), twilio::sign(TOKEN, url, &pairs)),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]
    },
};

fn twilio_bed() -> Bed {
    bed_for(TWILIO, "")
}

pub(super) fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    crate::sms::form_encode(&owned).into_bytes()
}

fn adapter() -> crate::sms::SmsAdapter<Twilio> {
    crate::sms::SmsAdapter::new(Twilio::default())
}

/// The form pairs a send posted.
pub(super) fn posted(seen: &Seen) -> Vec<(String, String)> {
    twilio::Form::parse(seen.body.as_bytes()).unwrap().pairs
}

pub(super) fn value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn header<'a>(seen: &'a Seen, name: &str) -> Option<&'a str> {
    seen.headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn basic(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    )
}

impl Bed {
    /// An inbound text from `from` as Twilio posts it, `extra` its other parameters; the response and its MessageSid.
    fn twilio_in(&self, from: &str, body: &str, extra: &[(&str, &str)]) -> (WebhookResponse, String) {
        let message = sid("SM", 1000 + self.next());
        let mut pairs = vec![
            ("MessageSid", message.as_str()),
            ("SmsSid", message.as_str()),
            ("AccountSid", ACCOUNT),
            ("From", from),
            ("To", OURS),
            ("Body", body),
            ("NumSegments", "1"),
        ];
        pairs.extend_from_slice(extra);
        if !extra.iter().any(|(name, _)| *name == "NumMedia") {
            pairs.push(("NumMedia", "0"));
        }
        (self.hook(&self.signed_bytes(form(&pairs), now() / 1000)), message)
    }

    /// A status callback of `message`.
    fn twilio_status(&self, message: &str, status: &str, error_code: Option<&str>) -> WebhookResponse {
        self.hook(&self.twilio_status_request(message, status, error_code))
    }

    /// A status callback's request, signed now.
    fn twilio_status_request(&self, message: &str, status: &str, error_code: Option<&str>) -> WebhookRequest {
        let mut pairs = vec![
            ("MessageSid", message),
            ("SmsSid", message),
            ("AccountSid", ACCOUNT),
            ("From", OURS),
            ("To", THEIRS),
            ("MessageStatus", status),
            ("SmsStatus", status),
        ];
        if let Some(code) = error_code {
            pairs.push(("ErrorCode", code));
        }
        self.signed_bytes(form(&pairs), now() / 1000)
    }

    /// `request` straight to the adapter's apply in the bridge's webhook, past the replay cache (a copy after its
    /// window: Twilio's signature has no time in it).
    fn applied_again(&self, request: &WebhookRequest) -> WebhookResponse {
        let context = self.ctx.factory().new_context();
        let _entered = context.enter();
        moqui_core::context::ExecutionContext::with_current(|ec| {
            ec.shared_map()
                .insert(moqui_core::service::chat_bridge::WEBHOOK_MARK.to_string(), text(BRIDGE));
        });
        adapter().apply_webhook(&bridge_info(self), request)
    }
}

fn is_twiml(response: &WebhookResponse) -> bool {
    response.status == 200
        && response.content_type == "text/xml"
        && String::from_utf8_lossy(&response.body).ends_with("<Response/>")
}

#[test]
fn a_twilio_bridge_needs_its_account_and_token_and_carries_groups_of_nine() {
    let bridge = |config: Json| moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: "sms-twilio".to_string(),
        name: "Twilio".to_string(),
        config,
    };
    let adapter = adapter();
    assert_eq!(adapter.name(), "sms-twilio");
    let caps = adapter.capabilities(&bridge(json!({})));
    assert_eq!(caps.group_max, 9, "10 participants, our number one of them");
    assert_eq!(caps.encryption, moqui_core::service::chat_bridge::Encryption::None);
    assert!(!caps.idempotent);
    assert!(
        adapter.start(&bridge(json!({"authToken": TOKEN}))).is_err(),
        "no account"
    );
    assert!(
        adapter.start(&bridge(json!({"accountSid": ACCOUNT}))).is_err(),
        "no token"
    );
    assert!(adapter
        .start(&bridge(
            json!({"accountSid": ACCOUNT, "authToken": TOKEN, "apiKeySid": API_KEY})
        ))
        .is_err());
    assert!(adapter
        .start(&bridge(json!({"accountSid": "AC/../x", "authToken": TOKEN})))
        .is_err());
    assert!(adapter
        .start(&bridge(json!({"accountSid": ACCOUNT, "authToken": TOKEN})))
        .is_ok());
    // the Conversations and file addresses carry the credentials too: https, or http only for a test's fake off
    // production
    for key in ["conversationsBaseUrl", "mediaContentBaseUrl"] {
        let mut plain = json!({"accountSid": ACCOUNT, "authToken": TOKEN});
        plain[key] = json!("http://twilio.example.test");
        assert!(adapter.start(&bridge(plain.clone())).unwrap_err().contains(key));
        plain["allowInternalAddresses"] = json!(true);
        assert!(adapter.start(&bridge(plain.clone())).is_ok(), "{key}");
        moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(Some(true)));
        let on_production = adapter.start(&bridge(plain));
        moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(None));
        assert!(on_production.is_err(), "{key} on production");
    }
    // the signature is the local node's to check: the edge node never holds the token
    assert_eq!(
        adapter.webhook_verification(),
        moqui_core::service::chat_bridge::WebhookVerification::Core {
            signature_header: "x-twilio-signature"
        }
    );
}

/// A text in, a reply out on the Messages API (form-encoded, Basic auth with the API key, `From` our number, its
/// status callback the bridge's webhook URL), and its status callbacks in any order: a late `sent` does not take a
/// delivered text back, the steps before the carrier are left, and an undelivered one fails with Twilio's code.
#[test]
fn a_twilio_reply_goes_out_and_its_status_callbacks_come_back_in_any_order() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (answer, _) = bed.twilio_in(THEIRS, "Are you open?", &[]);
    assert!(is_twiml(&answer), "{answer:?}");
    let room = bed.room(THEIRS).expect("the pair room");
    assert_eq!(bed.messages(&room)[0].1, "Are you open?");

    let seq = bed.send(&room, "Until six", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].path, format!("/2010-04-01/Accounts/{ACCOUNT}/Messages.json"));
    assert_eq!(
        header(&sends[0], "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        header(&sends[0], "authorization"),
        Some(basic(API_KEY, "key-secret").as_str())
    );
    let pairs = posted(&sends[0]);
    assert_eq!(value(&pairs, "To"), Some(THEIRS));
    assert_eq!(value(&pairs, "From"), Some(OURS));
    assert_eq!(value(&pairs, "Body"), Some("Until six"));
    assert_eq!(value(&pairs, "StatusCallback"), Some(WEBHOOK));
    assert_eq!(value(&pairs, "MessagingServiceSid"), None);
    let (state, _, _, remote, _) = bed.delivery(&room, seq);
    assert_eq!(
        (state.as_str(), remote.as_str()),
        ("CdsAccepted", sid("SM", 1).as_str())
    );

    // delivered first, then a late sent and a queued: still delivered; read is delivered
    for status in ["sending", "delivered", "sent", "queued", "read"] {
        assert!(is_twiml(&bed.twilio_status(&remote, status, None)), "{status}");
    }
    let (state, _, _, _, receipt) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), receipt.as_str()), ("CdsAccepted", "delivered"));

    // undelivered, with the carrier's code: failed, the reason Twilio's
    let second = bed.send(&room, "Or seven", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, second).3;
    bed.twilio_status(&remote, "sent", None);
    bed.twilio_status(&remote, "undelivered", Some("30003"));
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(second))],
    );
    assert_eq!(
        (row["statusEnumId"].as_str(), row["reasonCode"].as_str()),
        ("CdsFailed", "deliveryFailed")
    );
    assert!(row["lastErrorText"].contains("30003"), "{row:?}");
}

/// A bridge with a Messaging Service sends from the service (its sender pool), not `From`.
#[test]
fn a_twilio_bridge_with_a_messaging_service_sends_from_it() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    {
        let _authz = bed.ctx.disable_authz();
        let mut row = bed
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        let mut config = bridge_config(&bed);
        config["messagingServiceSid"] = json!(sid("MG", 7));
        row.set("configJson", text(&config.to_string()));
        row.update(&bed.facade).unwrap();
    }
    bed.twilio_in(THEIRS, "Hi", &[]);
    let room = bed.room(THEIRS).unwrap();
    bed.send(&room, "Hello", &[]);
    bed.round(0);
    let pairs = posted(&bed.fake.sends()[0]);
    assert_eq!(value(&pairs, "MessagingServiceSid"), Some(sid("MG", 7).as_str()));
    assert_eq!(value(&pairs, "From"), None);
}

/// Twilio's refusals: 21610 (the person sent STOP) fails the text and marks the pair opted out, so the next is refused
/// before Twilio; 21211 a bad number; 20429 and a 429 are tried again; an answer lost after the request went is
/// uncertain and never sent again.
#[test]
fn twilio_refusals_mark_an_opt_out_and_a_lost_answer_is_uncertain() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.twilio_in(THEIRS, "Hi", &[]);
    let room = bed.room(THEIRS).unwrap();
    let refused = |code: u32, status: u16| {
        Reply::Status(
            status,
            json!({"code": code, "message": format!("refused {code}"), "status": status}).to_string(),
        )
    };

    bed.fake.script(&[refused(21211, 400)]);
    let bad = bed.send(&room, "One", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, bad).1, "badNumber");

    bed.fake.script(&[refused(20429, 400), refused(20429, 429)]);
    let busy = bed.send(&room, "Two", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, busy).0, "CdsPending");
    bed.round(10_000);
    assert_eq!(bed.delivery(&room, busy).0, "CdsPending");
    bed.round(60_000);
    assert_eq!(bed.delivery(&room, busy).0, "CdsAccepted");

    bed.fake.script(&[Reply::Drop]);
    let lost = bed.send(&room, "Three", &[]);
    bed.round(120_000);
    assert_eq!(bed.delivery(&room, lost).0, "CdsUncertain");
    let sends = bed.fake.sends().len();
    bed.round(240_000);
    assert_eq!(bed.fake.sends().len(), sends, "never sent again on its own");

    bed.fake.script(&[refused(21610, 400)]);
    let stopped = bed.send(&room, "Four", &[]);
    bed.round(300_000);
    assert_eq!(bed.delivery(&room, stopped).1, "optedOut");
    let opt_out = bed.one(
        "moqui.call.SmsOptOut",
        &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
    );
    assert_eq!(opt_out["optedOut"], "Y");
    let sends = bed.fake.sends().len();
    let next = bed.send(&room, "Five", &[]);
    bed.round(360_000);
    assert_eq!(bed.delivery(&room, next).1, "optedOut");
    assert_eq!(bed.fake.sends().len(), sends, "refused before Twilio");
}

/// A 21610 that comes later, in a status callback (failed, `ErrorCode` 21610): the delivery fails and the pair is
/// marked opted out, once for that message (a copy after a START changes nothing).
#[test]
fn a_twilio_status_callback_with_21610_marks_the_pair_opted_out() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.twilio_in(THEIRS, "Hi", &[]);
    let room = bed.room(THEIRS).unwrap();
    let seq = bed.send(&room, "Hello", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, seq).3;
    let opted = || {
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
        )["optedOut"]
            .clone()
    };
    let callback = bed.twilio_status_request(&remote, "failed", Some("21610"));
    assert!(is_twiml(&bed.hook(&callback)));
    let (state, reason, ..) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "deliveryFailed"));
    assert_eq!(opted(), "Y");
    bed.twilio_in(THEIRS, "START", &[]);
    assert_eq!(opted(), "N");
    // the same callback again past the replay cache: the opt-out is not taken again
    assert!(is_twiml(&bed.applied_again(&callback)));
    assert_eq!(opted(), "N");
}

/// Advanced Opt-Out: Twilio answers STOP itself and still posts the text (with `OptOutType`); the opt-out is taken from
/// the text once, through the inbound message (its MessageSid), and the text is a message in the room. A copy of the
/// STOP after a START changes nothing. A keyword only the account knows (not one of Moqui's) is a text: Twilio's
/// block of a later send (21610) marks the pair then.
#[test]
fn a_twilio_opt_out_keyword_is_kept_once() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let opted = || {
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
        )
    };
    let stop = bed.signed_bytes(
        form(&[
            ("MessageSid", &sid("SM", 600)),
            ("From", THEIRS),
            ("To", OURS),
            ("Body", "STOP"),
            ("OptOutType", "STOP"),
        ]),
        now() / 1000,
    );
    assert!(is_twiml(&bed.hook(&stop)));
    assert_eq!(opted()["optedOut"], "Y");
    let room = bed.room(THEIRS).unwrap();
    assert_eq!(bed.messages(&room).len(), 1);
    bed.twilio_in(THEIRS, "START", &[("OptOutType", "START")]);
    assert_eq!(opted()["optedOut"], "N");
    // the STOP again (Twilio's signature has no time): answered, and the pair stays opted in
    assert!(is_twiml(&bed.hook(&stop)));
    assert_eq!(opted()["optedOut"], "N");
    // and past the replay cache (a copy after its window)
    let answer = bed.applied_again(&stop);
    assert!(is_twiml(&answer), "{answer:?}");
    assert_eq!(opted()["optedOut"], "N", "the STOP's message was applied once");
    assert_eq!(bed.messages(&room).len(), 2);
    bed.twilio_in(THEIRS, "Arret", &[("OptOutType", "STOP")]);
    assert_eq!(opted()["optedOut"], "N", "not a keyword of Moqui's");
    assert_eq!(bed.messages(&room).len(), 3);
}

// ---- files ----

/// A file server on loopback that answers `GET /{name}` with its file when the request carries `auth` (or `auth` is
/// none), a redirect for a name in `redirects`, and keeps each request's path and `Authorization`.
struct Files {
    base: String,
    seen: Arc<Mutex<Vec<Fetched>>>,
}

/// A request's path and its `Authorization`.
type Fetched = (String, Option<String>);

fn files(host: &str, auth: Option<String>, redirects: Vec<(&'static str, String)>) -> Files {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{host}:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            }
            let head = String::from_utf8_lossy(&buffer).to_string();
            let path = head.split(' ').nth(1).unwrap_or_default().to_string();
            let authorization = head
                .split("\r\n")
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_string());
            kept.lock().unwrap().push((path.clone(), authorization.clone()));
            let reply = if let Some((_, to)) = redirects.iter().find(|(name, _)| path == format!("/{name}")) {
                format!("HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .into_bytes()
            } else if auth.is_some() && authorization != auth {
                b"HTTP/1.1 401 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
            } else {
                let mut reply =
                    b"HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\ncontent-length: 3\r\nconnection: close\r\n\r\n"
                        .to_vec();
                reply.extend_from_slice(&[1, 2, 3]);
                reply
            };
            let _ = stream.write_all(&reply);
        }
    });
    Files { base, seen }
}

/// An MMS's files are fetched with the API's Basic auth (Twilio's media URLs need it); a redirect to the same host
/// keeps it, and one to another host (a file store) never gets it.
#[test]
fn twilio_files_are_fetched_with_basic_auth_never_sent_to_another_host() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let auth = basic(API_KEY, "key-secret");
    let store = files("localhost", None, Vec::new());
    let other = format!("{}/stored.jpg", store.base);
    let api = files(
        "127.0.0.1",
        Some(auth.clone()),
        vec![("away", other), ("here", "/pic.jpg".to_string())],
    );
    let (answer, _) = bed.twilio_in(
        THEIRS,
        "",
        &[
            ("NumMedia", "2"),
            ("MediaUrl0", &format!("{}/pic.jpg", api.base)),
            ("MediaContentType0", "image/jpeg"),
            ("MediaUrl1", &format!("{}/away", api.base)),
            ("MediaContentType1", "image/jpeg"),
        ],
    );
    assert!(is_twiml(&answer));
    let room = bed.room(THEIRS).unwrap();
    assert_eq!(
        bed.count("moqui.chat.ChatAttachment", &[("chatRoomId", text(&room))]),
        2,
        "{:?}",
        bed.messages(&room)
    );
    let api_seen = api.seen.lock().unwrap().clone();
    assert!(
        api_seen.iter().all(|(_, sent)| sent.as_deref() == Some(auth.as_str())),
        "{api_seen:?}"
    );
    let store_seen = store.seen.lock().unwrap().clone();
    assert_eq!(
        store_seen,
        vec![("/stored.jpg".to_string(), None)],
        "no Authorization to another host"
    );

    // a redirect on the same host keeps it
    let bridge = bridge_info(&bed);
    let fetched = adapter()
        .fetch_media(&bridge, &format!("{}/here", api.base), u64::MAX)
        .unwrap();
    assert_eq!(fetched.bytes, vec![1, 2, 3]);
    let last = api.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last, ("/pic.jpg".to_string(), Some(auth)));
}

// ---- the signature ----

/// A webhook request of `body` to `path` (and query), signed over `signed_url`.
fn request_to(path: &str, body: Vec<u8>, signed_url: &str) -> WebhookRequest {
    let pairs = twilio::Form::parse(&body).map(|form| form.pairs).unwrap_or_default();
    WebhookRequest {
        method: "POST".to_string(),
        url: path.to_string(),
        headers: vec![
            (
                "x-twilio-signature".to_string(),
                twilio::sign(TOKEN, signed_url, &pairs),
            ),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ],
        body,
        client_ip: "198.51.100.7".to_string(),
        now_millis: now(),
    }
}

/// The signature over the URL Twilio posted to and the sorted parameters: a good one is taken once, and a replay is a
/// duplicate; a wrong token, a changed parameter, none at all and another URL are the one unsigned answer.
#[test]
fn a_twilio_signature_is_over_the_url_and_the_sorted_parameters() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let path = format!("/channels/{BRIDGE}/webhook");
    let message = sid("SM", 77);
    let body = form(&[
        ("To", OURS),
        ("From", THEIRS),
        ("MessageSid", &message),
        ("Body", "Hello there"),
        ("NumMedia", "0"),
        ("AccountSid", ACCOUNT),
    ]);
    let good = request_to(&path, body.clone(), WEBHOOK);
    assert!(is_twiml(&bed.hook(&good)));
    let room = bed.room(THEIRS).unwrap();
    // the same request again: answered as done, not worked again, not counted against Twilio's address
    let replay = bed.hook(&good);
    assert!(is_twiml(&replay) && !replay.auth_failure);
    assert_eq!(bed.messages(&room).len(), 1);

    let mut wrong_token = good.clone();
    wrong_token.headers[0].1 = twilio::sign("another-token", WEBHOOK, &twilio::Form::parse(&body).unwrap().pairs);
    let mut tampered = good.clone();
    tampered.body = String::from_utf8(body.clone())
        .unwrap()
        .replace("Hello", "Send+money")
        .into_bytes();
    let mut unsigned = good.clone();
    unsigned.headers.retain(|(name, _)| name != "x-twilio-signature");
    let elsewhere = request_to(&path, body.clone(), "https://other.example.test/channels/SMS1/webhook");
    for bad in [&wrong_token, &tampered, &unsigned, &elsewhere] {
        let refused = bed.hook(bad);
        assert_eq!(refused, WebhookResponse::not_signed());
        assert!(refused.auth_failure);
    }
    assert_eq!(bed.messages(&room).len(), 1, "nothing refused was taken");

    // a query string is part of what is signed
    let with_query = request_to(
        &format!("{path}?source=sms&b=1"),
        form(&[
            ("From", THEIRS),
            ("To", OURS),
            ("MessageSid", &sid("SM", 78)),
            ("Body", "Q"),
        ]),
        &format!("{WEBHOOK}?source=sms&b=1"),
    );
    assert!(is_twiml(&bed.hook(&with_query)));
    let without_query = request_to(
        &format!("{path}?source=sms"),
        form(&[
            ("From", THEIRS),
            ("To", OURS),
            ("MessageSid", &sid("SM", 79)),
            ("Body", "Q"),
        ]),
        WEBHOOK,
    );
    assert_eq!(bed.hook(&without_query), WebhookResponse::not_signed());
    // the default port named or not: both are Twilio's
    let with_port = request_to(
        &path,
        form(&[
            ("From", THEIRS),
            ("To", OURS),
            ("MessageSid", &sid("SM", 80)),
            ("Body", "P"),
        ]),
        "https://moqui.example.test:443/channels/SMS1/webhook",
    );
    assert!(is_twiml(&bed.hook(&with_port)));
    let other_port = request_to(
        &path,
        form(&[
            ("From", THEIRS),
            ("To", OURS),
            ("MessageSid", &sid("SM", 81)),
            ("Body", "P"),
        ]),
        "https://moqui.example.test:8443/channels/SMS1/webhook",
    );
    assert_eq!(bed.hook(&other_port), WebhookResponse::not_signed());
    assert_eq!(bed.messages(&room).len(), 3);
}

#[test]
fn a_twilio_signature_takes_the_bridges_webhook_url_the_port_either_way_and_a_json_bodys_hash() {
    let pairs = vec![
        ("To".to_string(), OURS.to_string()),
        ("From".to_string(), THEIRS.to_string()),
        ("Body".to_string(), "a+b c&d=é".to_string()),
    ];
    let body = crate::sms::form_encode(&pairs).into_bytes();
    let check = |signed: &str, checked: &str| {
        twilio::signature_valid(TOKEN, checked, &body, &twilio::sign(TOKEN, signed, &pairs))
    };
    let hook = "https://hooks.example.test/twilio/sms?b=SMS1";
    assert!(check(hook, hook));
    // a connection override in the configured URL is not signed
    assert!(check(hook, &format!("{hook}#rc=3&rp=5xx")));
    assert!(check("https://hooks.example.test:443/twilio/sms?b=SMS1", hook));
    assert!(check(hook, "https://hooks.example.test:443/twilio/sms?b=SMS1"));
    assert!(check(
        "http://hooks.example.test:80/sms",
        "http://hooks.example.test/sms"
    ));
    assert!(!check(
        "https://hooks.example.test:80/sms",
        "https://hooks.example.test/sms"
    ));
    assert!(!check(hook, "https://hooks.example.test/twilio/sms?b=SMS2"));
    // credentials in the URL are not signed
    assert!(check(
        "https://hooks.example.test/sms",
        "https://user:pass@hooks.example.test/sms"
    ));
    // the names sorted case-sensitively: an upper-case name before a lower-case one
    let mixed = vec![("b".to_string(), "1".to_string()), ("A".to_string(), "2".to_string())];
    let signature = twilio::sign(TOKEN, hook, &mixed);
    let expected = {
        let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, TOKEN.as_bytes());
        let data = format!("{hook}A2b1");
        base64::engine::general_purpose::STANDARD.encode(aws_lc_rs::hmac::sign(&key, data.as_bytes()).as_ref())
    };
    assert_eq!(signature, expected);
    assert!(!twilio::signature_valid(TOKEN, hook, &body, "not base64!"));

    // a JSON body: its SHA-256 in the URL's query, the URL alone signed
    let json = br#"{"event":"x"}"#;
    let digest = moqui_lib::codec::hex_encode(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, json).as_ref());
    let url = format!("{hook}&bodySHA256={digest}");
    let signed = twilio::sign(TOKEN, &url, &[]);
    assert!(twilio::signature_valid(TOKEN, &url, json, &signed));
    assert!(!twilio::signature_valid(TOKEN, &url, br#"{"event":"y"}"#, &signed));

    // the bridge's webhookUrl is the URL checked, whatever path the request came to
    let bridge = moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: "sms-twilio".to_string(),
        name: "Twilio".to_string(),
        config: json!({"accountSid": ACCOUNT, "authToken": TOKEN, "webhookUrl": hook}),
    };
    let adapter = adapter();
    let proxied = request_to("/channels/SMS1/webhook", body.clone(), hook);
    assert_eq!(adapter.verify_webhook(&bridge, &proxied), None);
    let direct = request_to("/channels/SMS1/webhook", body, WEBHOOK);
    assert_eq!(
        adapter.verify_webhook(&bridge, &direct),
        Some(WebhookResponse::not_signed())
    );
}

/// An edge node keys a Twilio webhook by its message (a status callback by its message and status, each change its
/// own event; a Conversation's by its message or receipt), ordered by their number or the Conversation.
#[test]
fn an_edge_node_keys_a_twilio_webhook_by_its_message_and_status() {
    let key = |pairs: &[(&str, &str)]| {
        Twilio::default()
            .event_key(&request_to("/channels/SMS1/webhook", form(pairs), WEBHOOK))
            .unwrap()
    };
    let inbound = key(&[
        ("MessageSid", "SM1"),
        ("From", THEIRS),
        ("To", OURS),
        ("SmsStatus", "received"),
    ]);
    assert_eq!(
        (inbound.event_id.as_str(), inbound.conversation.as_deref()),
        ("SM1", Some(THEIRS))
    );
    let status = key(&[
        ("MessageSid", "SM1"),
        ("From", OURS),
        ("To", THEIRS),
        ("MessageStatus", "delivered"),
    ]);
    assert_eq!(
        (status.event_id.as_str(), status.conversation.as_deref()),
        ("SM1:delivered", Some(THEIRS))
    );
    let receipt = key(&[
        ("EventType", "onDeliveryUpdated"),
        ("ConversationSid", "CH1"),
        ("DeliveryReceiptSid", "DY1"),
        ("Status", "read"),
    ]);
    assert_eq!(
        (receipt.event_id.as_str(), receipt.conversation.as_deref()),
        ("DY1:read", Some("CH1"))
    );
    let added = key(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", "CH1"),
        ("MessageSid", "IM1"),
    ]);
    assert_eq!(added.event_id, "IM1");
    let none = key(&[("Other", "x")]);
    assert!(none.event_id.starts_with("sha256:"));
}

/// Behind an edge node: the edge node queued a Twilio webhook it could not verify; the local node verifies it with the
/// bridge's token over the URL it came to (a time later: Twilio's signature has no time), applies a good one and drops
/// a forged one, counted, not tried again.
#[test]
fn a_queued_twilio_webhook_is_verified_on_the_local_node() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserAccount",
        &[
            ("userId", text("EDGE_DELEGATE")),
            ("username", text(moqui_core::edge::delegate::EDGE_USERNAME)),
        ],
    );
    let queued = |item: &str, request: &WebhookRequest| {
        row(
            &bed.facade,
            moqui_core::edge::queue::RECEIPT,
            &[("pairedNodeId", text("edge-1")), ("queueItemId", text(item))],
        );
        let headers: Vec<[&str; 2]> = request
            .headers
            .iter()
            .map(|(name, value)| [name.as_str(), value.as_str()])
            .collect();
        let payload = json!({"chatBridgeId": BRIDGE, "method": "POST", "url": request.url, "headers": headers,
            "body": base64::engine::general_purpose::STANDARD.encode(&request.body), "clientIp": request.client_ip,
            "receivedMillis": now() - 3_600_000});
        let parameters = BTreeMap::from([
            ("queueItemId".to_string(), text(item)),
            ("pairedNodeId".to_string(), text("edge-1")),
            ("payload".to_string(), text(&payload.to_string())),
        ]);
        // the queue's apply runs it in its transaction; here none, as the in-memory facade's outer transaction would
        // not see what the services under it commit in their own
        let options = moqui_core::service::call::CallOptions {
            disable_authz: true,
            ignore_transaction: Some(true),
            ..moqui_core::service::call::CallOptions::default()
        };
        // the queue applies each item as its delegate user
        login(moqui_core::edge::delegate::EDGE_USERNAME);
        moqui_core::service::call::sync_checked(
            "org.moqui.chat.ChatBridgeServices.apply#EdgeWebhook",
            &parameters,
            &options,
        )
        .unwrap()
    };
    let path = format!("/channels/{BRIDGE}/webhook?edge=1");
    let pairs =
        |message: &str, words: &str| form(&[("MessageSid", message), ("From", THEIRS), ("To", OURS), ("Body", words)]);
    let good = request_to(
        &path,
        pairs(&sid("SM", 90), "Through the edge"),
        &format!("{WEBHOOK}?edge=1"),
    );
    let out = queued("item-1", &good);
    assert!(!out.contains_key("retryLater"), "{out:?}");
    let room = bed.room(THEIRS).expect("applied");
    assert_eq!(bed.messages(&room)[0].1, "Through the edge");

    let forged = request_to(&path, pairs(&sid("SM", 91), "Forged"), "https://evil.example.test/hook");
    let out = queued("item-2", &forged);
    // done, not tried again (the item's answer names no retryLater)
    assert!(!out.contains_key("retryLater"), "{out:?}");
    assert_eq!(bed.messages(&room).len(), 1, "nothing refused was applied");
    assert_eq!(
        moqui_core::service::chat_bridge::refused_webhooks(bed.ctx.factory(), BRIDGE),
        1
    );
    assert_eq!(
        bed.count(
            "moqui.chat.ChatBridgeWebhookRefused",
            &[("chatBridgeId", text(BRIDGE)), ("queueItemId", text("item-2"))]
        ),
        1,
        "kept for a replay once the configuration is fixed"
    );

    // a bridge with no authToken here cannot check it: the item waits, not refused as forged
    {
        let _authz = bed.ctx.disable_authz();
        let mut row = bed
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        let mut config = bridge_config(&bed);
        config.as_object_mut().unwrap().remove("authToken");
        row.set("configJson", text(&config.to_string()));
        row.update(&bed.facade).unwrap();
    }
    let waiting = request_to(&path, pairs(&sid("SM", 92), "Later"), &format!("{WEBHOOK}?edge=1"));
    let out = queued("item-3", &waiting);
    assert!(out_text(&out, "retryLater").contains("authToken"), "{out:?}");
    assert_eq!(
        moqui_core::service::chat_bridge::refused_webhooks(bed.ctx.factory(), BRIDGE),
        1
    );
}

// ---- group texts ----

const OTHER: &str = "+15553334444";

impl Bed {
    /// A group room of THEIRS and OTHER on our number; its id and its mapping's external room id.
    fn twilio_group(&self) -> (String, String) {
        login("alice");
        let out = ok(
            "org.moqui.call.SmsServices.ensure#PairRoom",
            &[
                ("ourNumber", text(OURS)),
                ("theirNumberList", Value::List(vec![text(THEIRS), text(OTHER)])),
            ],
        );
        (out_text(&out, "chatRoomId"), format!("{OURS}/{OTHER},{THEIRS}"))
    }

    /// A Conversations webhook of `pairs`, signed now.
    fn conversation_event(&self, pairs: &[(&str, &str)]) -> WebhookResponse {
        self.hook(&self.signed_bytes(form(pairs), now() / 1000))
    }

    /// The mapping's thread of `room`.
    fn thread(&self, room: &str) -> String {
        self.one(
            "moqui.chat.ChatRoomBridge",
            &[("chatRoomId", text(room)), ("chatBridgeId", text(BRIDGE))],
        )
        .get("externalThreadId")
        .cloned()
        .unwrap_or_default()
    }

    /// The fake's participants of `conversation`: ours (projected) and each of `theirs`, `MB{base + n}`.
    fn twilio_roster(&self, conversation: &str, base: u64, theirs: &[&str]) {
        let mut participants =
            vec![json!({"sid": sid("MB", base), "messaging_binding": {"type": "sms", "projected_address": OURS}})];
        for (n, number) in theirs.iter().enumerate() {
            participants.push(json!({"sid": sid("MB", base + 1 + n as u64),
                "messaging_binding": {"type": "sms", "address": number}}));
        }
        self.fake.file(
            &format!("/v1/Conversations/{conversation}/Participants?PageSize=50"),
            "application/json",
            json!({"participants": participants}).to_string().as_bytes(),
        );
    }

    /// The fake's Conversation `conversation` in `state`.
    fn twilio_state(&self, conversation: &str, state: &str) {
        self.fake.file(
            &format!("/v1/Conversations/{conversation}"),
            "application/json",
            json!({"sid": conversation, "state": state}).to_string().as_bytes(),
        );
    }

    /// The paths of the sends the fake took that end with `tail`.
    fn twilio_sends(&self, tail: &str) -> Vec<String> {
        self.fake
            .sends()
            .into_iter()
            .map(|seen| seen.path)
            .filter(|path| path.ends_with(tail))
            .collect()
    }
}

/// A group room's first send makes its Conversation in one request with all its numbers (our number by its projected
/// address, each other by its address, the room's unique name), keeps its SID on the mapping, adds the bridge's
/// webhook, and posts; later sends only post. Its receipts are each recipient's, a reply in it comes into the group
/// room, a group text with files is refused, and an answer lost on the post is uncertain.
#[test]
fn a_twilio_group_text_goes_through_one_conversation() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (room, external) = bed.twilio_group();
    let seq = bed.send(&room, "Dinner at eight?", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    let conversation = sid("CH", 1);
    let paths: Vec<&str> = sends.iter().map(|seen| seen.path.as_str()).collect();
    let under = |tail: &str| format!("/v1/Conversations/{conversation}/{tail}");
    assert_eq!(
        paths,
        vec![
            "/v1/ConversationWithParticipants".to_string(),
            under("Webhooks"),
            under("Messages"),
        ]
    );
    let made = posted(&sends[0]);
    assert_eq!(
        value(&made, "UniqueName"),
        Some(twilio::group::unique_name(BRIDGE, &external).as_str())
    );
    let participants: Vec<Json> = made
        .iter()
        .filter(|(name, _)| name == "Participant")
        .map(|(_, value)| serde_json::from_str(value).unwrap())
        .collect();
    assert_eq!(
        participants,
        vec![
            json!({"messaging_binding": {"projected_address": OURS}}),
            json!({"messaging_binding": {"address": OTHER}}),
            json!({"messaging_binding": {"address": THEIRS}}),
        ]
    );
    let hook = posted(&sends[1]);
    assert_eq!(value(&hook, "Configuration.Url"), Some(WEBHOOK));
    assert_eq!(value(&hook, "Configuration.Method"), Some("post"));
    let filters: Vec<&str> = hook
        .iter()
        .filter(|(name, _)| name == "Configuration.Filters")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(filters, vec!["onMessageAdded", "onDeliveryUpdated"]);
    let message = posted(&sends[2]);
    assert_eq!(value(&message, "Author"), Some(OURS));
    assert_eq!(value(&message, "Body"), Some("Dinner at eight?"));
    assert!(sends
        .iter()
        .all(|seen| header(seen, "authorization") == Some(basic(API_KEY, "key-secret").as_str())));
    let (state, _, _, remote, _) = bed.delivery(&room, seq);
    assert_eq!(
        (state.as_str(), remote.as_str()),
        ("CdsAccepted", sid("IM", 3).as_str())
    );
    assert_eq!(bed.thread(&room), conversation, "the mapping keeps the Conversation");
    assert_eq!(bed.sent_count("user:alice"), 2, "counted once a recipient");

    // the next send only posts to it
    let second = bed.send(&room, "Or nine", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 4);
    assert_eq!(sends[3].path, under("Messages"));
    assert_eq!(bed.delivery(&room, second).0, "CdsAccepted");

    // receipts, one a recipient, the participants read from Twilio (MB10 ours, MB11 OTHER, MB12 THEIRS)
    bed.twilio_roster(&conversation, 10, &[OTHER, THEIRS]);
    let receipt = |participant: u64, status: &str, receipt: u64| {
        bed.conversation_event(&[
            ("EventType", "onDeliveryUpdated"),
            ("AccountSid", ACCOUNT),
            ("ConversationSid", &conversation),
            ("MessageSid", &remote),
            ("ParticipantSid", &sid("MB", participant)),
            ("DeliveryReceiptSid", &sid("DY", receipt)),
            ("ChannelMessageSid", &sid("SM", 500 + receipt)),
            ("Status", status),
        ])
    };
    assert!(is_twiml(&receipt(11, "delivered", 1)));
    receipt(12, "sent", 2);
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(seq))],
    );
    let recipients: Json = serde_json::from_str(&row["recipientReceipts"]).unwrap();
    assert_eq!(
        (recipients[OTHER].as_str(), recipients[THEIRS].as_str()),
        (Some("delivered"), Some("sent"))
    );
    assert_eq!(row["receiptStatus"], "sent", "the least of the two");

    // a reply from one of them, into the group room; our own message's event is left
    let reply = bed.conversation_event(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", &conversation),
        ("MessageSid", &sid("IM", 900)),
        ("Author", OTHER),
        ("Body", "Eight works"),
        ("Source", "SMS"),
    ]);
    assert!(is_twiml(&reply));
    let texts = bed.messages(&room);
    assert_eq!(
        texts.last().map(|(_, body)| body.as_str()),
        Some("Eight works"),
        "{texts:?}"
    );
    bed.conversation_event(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", &conversation),
        ("MessageSid", &sid("IM", 901)),
        ("Author", OURS),
        ("Body", "an echo"),
    ]);
    assert_eq!(bed.messages(&room).len(), texts.len());
    assert!(bed.room(OTHER).is_none(), "not a pair room");

    // files: refused, saying why, before anything is posted
    let file = bed.upload(&room, "map.png", 100);
    let with_file = bed.send(&room, "A map", &[file]);
    bed.round(0);
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(with_file))],
    );
    assert_eq!(row["reasonCode"], "groupMediaNotSupported");
    assert_eq!(bed.fake.sends().len(), 4);

    // an answer lost on the post: uncertain, not sent again
    bed.fake.script(&[Reply::Drop]);
    let lost = bed.send(&room, "Lost", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, lost).0, "CdsUncertain");
    bed.round(120_000);
    assert_eq!(bed.fake.sends().len(), 5);
}

/// An answer lost when the Conversation is made: nothing reached a phone, so it is tried again, and the next try finds
/// the Conversation by its unique name instead of making a second, and puts the bridge's webhook on it.
#[test]
fn a_twilio_group_conversation_is_made_once() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (room, external) = bed.twilio_group();
    bed.fake.script(&[Reply::Drop]);
    let seq = bed.send(&room, "Hello all", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, seq).0, "CdsPending");
    // Twilio made it before the answer was lost
    let made = sid("CH", 1);
    bed.fake.file(
        &format!("/v1/Conversations/{}", twilio::group::unique_name(BRIDGE, &external)),
        "application/json",
        json!({"sid": made, "state": "active"}).to_string().as_bytes(),
    );
    bed.round(60_000);
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(
        bed.twilio_sends("/ConversationWithParticipants").len(),
        1,
        "one Conversation"
    );
    assert_eq!(
        bed.twilio_sends("/Webhooks"),
        vec![format!("/v1/Conversations/{made}/Webhooks")]
    );
    assert_eq!(
        bed.fake.sends().last().unwrap().path,
        format!("/v1/Conversations/{made}/Messages")
    );
    assert_eq!(bed.thread(&room), made);
}

/// The webhook is on the Conversation whenever a send uses it: one not added (Twilio's 5xx) is added on the next try,
/// which carries the kept Conversation and makes no other; one Twilio still initializes waits; one that has the
/// bridge's webhook already gets no second.
#[test]
fn a_twilio_group_conversations_webhook_is_added_on_a_retry() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (room, _) = bed.twilio_group();
    let made = sid("CH", 1);
    bed.fake.script(&[Reply::Status(
        201,
        json!({"sid": made, "state": "initializing"}).to_string(),
    )]);
    let seq = bed.send(&room, "Hello all", &[]);
    bed.round(0);
    let (state, reason, ..) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsPending", "{reason}");
    assert_eq!(bed.thread(&room), made, "kept at once");
    assert!(bed.twilio_sends("/Webhooks").is_empty(), "not while it initializes");

    // active now; the webhook's add fails once
    bed.twilio_state(&made, "active");
    bed.fake
        .script(&[Reply::Status(500, json!({"message": "busy"}).to_string())]);
    bed.round(10_000);
    assert_eq!(bed.delivery(&room, seq).0, "CdsPending");
    bed.round(70_000);
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(bed.twilio_sends("/ConversationWithParticipants").len(), 1);
    assert_eq!(bed.twilio_sends("/Webhooks").len(), 2, "tried again until added");
    assert_eq!(bed.twilio_sends("/Messages").len(), 1);

    // another process (a fresh adapter) that finds the bridge's webhook there adds none
    moqui_core::service::chat_bridge::register_adapter(bed.ctx.factory(), Arc::new(adapter()));
    bed.fake.file(
        &format!("/v1/Conversations/{made}/Webhooks"),
        "application/json",
        json!({"webhooks": [{"sid": sid("WH", 9), "target": "webhook", "configuration": {"url": WEBHOOK}}]})
            .to_string()
            .as_bytes(),
    );
    let again = bed.send(&room, "Again", &[]);
    bed.round(140_000);
    assert_eq!(bed.delivery(&room, again).0, "CdsAccepted");
    assert_eq!(bed.twilio_sends("/Webhooks").len(), 2);
}

/// A kept Conversation that is closed or gone (404): let go from the mapping, tried again, and the next try makes a new
/// one (a closed one of the room's unique name is deleted first, freeing the name).
#[test]
fn a_closed_or_gone_twilio_conversation_is_made_again() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (room, external) = bed.twilio_group();
    let first = bed.send(&room, "One", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, first).0, "CdsAccepted");
    let made = sid("CH", 1);
    assert_eq!(bed.thread(&room), made);

    // gone: the post is refused; the next send checks it, lets it go, and makes another
    bed.fake.script(&[Reply::Status(
        404,
        json!({"code": 20404, "message": "The requested resource was not found"}).to_string(),
    )]);
    let refused = bed.send(&room, "Two", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, refused).0, "CdsFailed");
    let next = bed.send(&room, "Three", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, next).0, "CdsPending");
    assert_eq!(bed.thread(&room), "", "let go");
    bed.round(60_000);
    assert_eq!(bed.delivery(&room, next).0, "CdsAccepted");
    let remade = bed.thread(&room);
    assert!(!remade.is_empty() && remade != made, "{remade}");
    assert_eq!(bed.twilio_sends("/ConversationWithParticipants").len(), 2);

    // closed, in another process: let go; its unique name's Conversation is the closed one, deleted, and made again
    moqui_core::service::chat_bridge::register_adapter(bed.ctx.factory(), Arc::new(adapter()));
    bed.twilio_state(&remade, "closed");
    bed.fake.file(
        &format!("/v1/Conversations/{}", twilio::group::unique_name(BRIDGE, &external)),
        "application/json",
        json!({"sid": remade, "state": "closed"}).to_string().as_bytes(),
    );
    let last = bed.send(&room, "Four", &[]);
    bed.round(120_000);
    assert_eq!(bed.thread(&room), "");
    bed.round(180_000);
    assert_eq!(bed.delivery(&room, last).0, "CdsAccepted");
    assert!(
        bed.twilio_sends(&format!("/Conversations/{remade}")).len() == 1,
        "the closed one deleted"
    );
    assert_eq!(bed.twilio_sends("/ConversationWithParticipants").len(), 3);
}

/// A group text someone else started (Twilio made its Conversation): its numbers are read from Twilio once, the text
/// is the group's room, and its files are fetched from the Media Content Service. A Conversation of one number (Twilio's
/// autocreation: `proxy_address` our number) is the pair's room; one with no number of ours is left, logged.
#[test]
fn a_twilio_group_text_from_a_conversation_twilio_made_comes_into_the_group_room() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let conversation = sid("CH", 7);
    let service = sid("IS", 1);
    let media = sid("ME", 1);
    bed.twilio_roster(&conversation, 71, &[THEIRS, OTHER]);
    bed.fake.file(
        &format!("/v1/Services/{service}/Media/{media}/Content"),
        "image/jpeg",
        &[9, 8, 7],
    );
    let media_list = json!([{"Sid": media, "ContentType": "image/jpeg", "Filename": "a.jpg", "Size": 3}]).to_string();
    let answer = bed.conversation_event(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", &conversation),
        ("ChatServiceSid", &service),
        ("MessageSid", &sid("IM", 70)),
        ("Author", THEIRS),
        ("Body", "Who is in?"),
        ("Media", &media_list),
    ]);
    assert!(is_twiml(&answer), "{answer:?}");
    let mut sorted = [THEIRS, OTHER];
    sorted.sort_unstable();
    let room = bed.room(&sorted.join(",")).expect("the group's room");
    assert_eq!(bed.messages(&room)[0].1, "Who is in?");
    assert_eq!(
        bed.count("moqui.chat.ChatAttachment", &[("chatRoomId", text(&room))]),
        1
    );

    // one number, our number its proxy address: the pair's room
    let pair = sid("CH", 8);
    bed.fake.file(
        &format!("/v1/Conversations/{pair}/Participants?PageSize=50"),
        "application/json",
        json!({"participants": [{"sid": sid("MB", 81),
            "messaging_binding": {"type": "sms", "address": THEIRS, "proxy_address": OURS}}]})
        .to_string()
        .as_bytes(),
    );
    bed.conversation_event(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", &pair),
        ("MessageSid", &sid("IM", 80)),
        ("Author", THEIRS),
        ("Body", "Just me"),
    ]);
    let pair_room = bed.room(THEIRS).expect("the pair's room");
    assert_eq!(bed.messages(&pair_room)[0].1, "Just me");

    // no number of ours among its participants: left (logged), nothing made
    let foreign = sid("CH", 9);
    bed.fake.file(
        &format!("/v1/Conversations/{foreign}/Participants?PageSize=50"),
        "application/json",
        json!({"participants": [{"sid": sid("MB", 91), "messaging_binding": {"type": "sms", "address": OTHER}}]})
            .to_string()
            .as_bytes(),
    );
    let left = bed.conversation_event(&[
        ("EventType", "onMessageAdded"),
        ("ConversationSid", &foreign),
        ("MessageSid", &sid("IM", 90)),
        ("Author", OTHER),
        ("Body", "Hello?"),
    ]);
    assert!(is_twiml(&left));
    assert!(bed.room(OTHER).is_none());
}

/// Twilio answers that another Conversation has the group's numbers (50438: one it made for a group text that came
/// in): that one, found by its participants on the second page of the first number's Conversations, is the room's,
/// with the bridge's webhook added to it.
#[test]
fn a_twilio_group_room_takes_the_conversation_twilio_has_of_its_numbers() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let (room, _) = bed.twilio_group();
    let theirs = sid("CH", 42);
    let second_page = format!(
        "{}/v1/ParticipantConversations?Address=%2B15553334444&PageSize=50&Page=1",
        bed.fake.base
    );
    bed.fake.file(
        "/v1/ParticipantConversations?Address=%2B15553334444&PageSize=50",
        "application/json",
        json!({"conversations": [{"conversation_sid": sid("CH", 41), "conversation_state": "closed"}],
            "meta": {"next_page_url": second_page}})
        .to_string()
        .as_bytes(),
    );
    bed.fake.file(
        "/v1/ParticipantConversations?Address=%2B15553334444&PageSize=50&Page=1",
        "application/json",
        json!({"conversations": [{"conversation_sid": theirs, "conversation_state": "active"}],
            "meta": {"next_page_url": null}})
        .to_string()
        .as_bytes(),
    );
    bed.twilio_roster(&theirs, 81, &[THEIRS, OTHER]);
    bed.fake.script(&[Reply::Status(
        400,
        json!({"code": 50438, "message": "Group conversation with given participant list already exists"}).to_string(),
    )]);
    let seq = bed.send(&room, "Hello again", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(bed.thread(&room), theirs);
    assert_eq!(
        bed.twilio_sends("/Webhooks"),
        vec![format!("/v1/Conversations/{theirs}/Webhooks")]
    );
    assert_eq!(
        bed.fake.sends().last().map(|seen| seen.path.clone()),
        Some(format!("/v1/Conversations/{theirs}/Messages"))
    );
}
