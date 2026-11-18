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

//! The `sms-sinch` adapter (`providers/sinch.rs`) against a fake Sinch on loopback, its token service and its
//! Conversation API on one address: the access token fetched once, kept until it runs out and dropped on a 401; a text
//! out on the SMS channel and a file on the MMS channel (refused without `mmsSender`, and more than one); Sinch's answers
//! (429, a lost answer, the credit limit, the wrong region); texts, files, delivery reports, opt-outs and opt-ins in;
//! the webhook signature (good, bad, missing, old, another algorithm, replayed); no group texts; and a webhook an edge
//! node queued, verified on the local node.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::providers::sinch::Sinch;

pub(super) const SECRET: &str = "sinch-webhook-secret";
const TOKEN_PATH: &str = "/oauth2/token";
const SEND_PATH: &str = "/v1/projects/proj-1/messages:send";

/// Each signature's own nonce.
static NONCE: AtomicU64 = AtomicU64::new(0);

/// The headers Sinch signs `body` with at `at_seconds`, under `secret`, by `algorithm`.
pub(super) fn sinch_headers(secret: &str, body: &[u8], at_seconds: i64, algorithm: &str) -> Vec<(String, String)> {
    let nonce = format!("01NONCE{}", NONCE.fetch_add(1, Ordering::Relaxed));
    let stamp = at_seconds.to_string();
    let mut message = body.to_vec();
    message.extend_from_slice(format!(".{nonce}.{stamp}").as_bytes());
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(aws_lc_rs::hmac::sign(&key, &message).as_ref());
    vec![
        ("x-sinch-webhook-signature".to_string(), signature),
        ("x-sinch-webhook-signature-nonce".to_string(), nonce),
        ("x-sinch-webhook-signature-timestamp".to_string(), stamp),
        ("x-sinch-webhook-signature-algorithm".to_string(), algorithm.to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ]
}

const SINCH: Provider = Provider {
    adapter_name: "sms-sinch",
    bridge_name: "Sinch",
    config: |base, _keys| {
        json!({"projectId": "proj-1", "appId": "app-1", "keyId": "key-1", "keySecret": "key-secret", "region": "us",
            "apiBaseUrl": base, "authBaseUrl": base, "webhookSecret": SECRET, "smsSender": OURS, "mmsSender": OURS,
            "maxMediaCount": 2, "allowInternalAddresses": true})
    },
    accepted: |n, path| {
        if path == TOKEN_PATH {
            Reply::Status(
                200,
                json!({"access_token": format!("tok-{n}"), "expires_in": 3600, "token_type": "bearer"}).to_string(),
            )
        } else {
            Reply::Status(
                200,
                json!({"message_id": format!("sinch-{n}"), "accepted_time": "2026-11-01T12:00:00Z"}).to_string(),
            )
        }
    },
    sign: |_keys, body, at_seconds, _url| sinch_headers(SECRET, body, at_seconds, "HmacSHA256"),
};

/// Our number and theirs as Sinch gives them: digits, no `+`.
pub(super) fn digits(number: &str) -> &str {
    number.trim_start_matches('+')
}

/// A `MESSAGE_INBOUND` of `content` (`{"text_message": ...}`) from `from`, message `id`.
fn inbound(id: &str, from: &str, content: Json) -> Json {
    json!({"app_id": "app-1", "project_id": "proj-1", "accepted_time": "2026-11-01T12:00:00.100Z",
        "event_time": "2026-11-01T12:00:00Z", "message": {"id": id, "direction": "TO_APP", "contact_message": content,
            "channel_identity": {"channel": "SMS", "identity": digits(from), "app_id": ""}, "conversation_id": "conv-1",
            "contact_id": "contact-1", "metadata": "", "accept_time": "2026-11-01T12:00:00Z", "sender_id": digits(OURS),
            "processing_mode": "DISPATCH"}})
}

/// A `MESSAGE_DELIVERY` of message `id` in `status`.
fn delivery_report(id: &str, status: &str) -> Json {
    let mut report = json!({"message_id": id, "conversation_id": "conv-1", "status": status,
        "channel_identity": {"channel": "SMS", "identity": digits(THEIRS), "app_id": ""}, "contact_id": "contact-1",
        "metadata": "", "processing_mode": "DISPATCH"});
    if status == "FAILED" {
        report["reason"] = json!({"code": "RECIPIENT_NOT_REACHABLE", "description": "The phone is off",
            "sub_code": "UNSPECIFIED_SUB_CODE"});
    }
    json!({"app_id": "app-1", "project_id": "proj-1", "accepted_time": "2026-11-01T12:00:05Z",
        "event_time": "2026-11-01T12:00:04Z", "message_delivery_report": report})
}

/// An `OPT_OUT` or `OPT_IN` notification (`key`) of their number.
fn opt_notice(key: &str, request: &str, status: &str) -> Json {
    json!({"app_id": "app-1", "project_id": "proj-1", "accepted_time": "2026-11-01T12:00:05Z",
        "event_time": "2026-11-01T12:00:05Z", key: {"request_id": request, "contact_id": "", "channel": "SMS",
            "identity": digits(THEIRS), "status": status, "processing_mode": "DISPATCH"}})
}

impl Bed {
    /// A Sinch text from their number, signed now: the response and the message id.
    fn sinch_text_in(&self, body: &str) -> (WebhookResponse, String) {
        let id = format!("01IN{}", self.next());
        let event = inbound(&id, THEIRS, json!({"text_message": {"text": body}}));
        (self.hook(&self.signed(&event, now() / 1000)), id)
    }

    /// The fake's requests to `path`.
    fn sinch_calls(&self, path: &str) -> Vec<Seen> {
        self.fake.sends().into_iter().filter(|seen| seen.path == path).collect()
    }

    /// Change the bridge's configuration.
    fn reconfigure(&self, change: impl FnOnce(&mut Json)) {
        let _authz = self.ctx.disable_authz();
        let mut row = self
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        let mut config: Json = serde_json::from_str(&row.get("configJson").map(to_plain_string).unwrap()).unwrap();
        change(&mut config);
        row.set("configJson", text(&config.to_string()));
        row.update(&self.facade).unwrap();
    }
}

fn header(seen: &Seen, name: &str) -> Option<String> {
    seen.headers
        .iter()
        .find(|(known, _)| known == name)
        .map(|(_, value)| value.clone())
}

/// The bridge's Sinch settings, as the adapter reads them.
fn sinch_config(bed: &Bed) -> crate::providers::sinch::Config {
    use crate::sms::SmsProvider as _;
    Sinch::default().config(&bridge_config(bed))
}

/// The access token is fetched once with the key (Basic, client credentials) and kept for the sends after it, fetched
/// again when it runs out, dropped when Sinch refuses it (401: the text sent once more with a new one); a token service
/// that does not give one is `Retry` (nothing sent).
#[test]
fn a_sinch_token_is_fetched_once_kept_until_it_runs_out_and_dropped_on_a_401() {
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    bed.sinch_text_in("Hi");
    let room = bed.room(THEIRS).expect("the pair room");
    let first = bed.send(&room, "One", &[]);
    let second = bed.send(&room, "Two", &[]);
    bed.round(0);
    let tokens = bed.sinch_calls(TOKEN_PATH);
    assert_eq!(tokens.len(), 1, "one token for both sends");
    let basic = base64::engine::general_purpose::STANDARD.encode("key-1:key-secret");
    assert_eq!(header(&tokens[0], "authorization"), Some(format!("Basic {basic}")));
    assert_eq!(tokens[0].body, "grant_type=client_credentials");
    let sends = bed.sinch_calls(SEND_PATH);
    assert_eq!(sends.len(), 2);
    for sent in &sends {
        assert_eq!(header(sent, "authorization").as_deref(), Some("Bearer tok-1"));
    }
    for seq in [first, second] {
        assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    }

    // Sinch no longer takes the token: dropped, a new one fetched, and the text sent once more, taken
    bed.fake.script(&[Reply::Status(
        401,
        r#"{"error":{"code":401,"message":"Unauthorized","status":"UNAUTHENTICATED"}}"#.to_string(),
    )]);
    let third = bed.send(&room, "Three", &[]);
    bed.round(0);
    let (state, _, attempts, ..) = bed.delivery(&room, third);
    assert_eq!((state.as_str(), attempts.as_str()), ("CdsAccepted", "1"));
    assert_eq!(bed.sinch_calls(TOKEN_PATH).len(), 2);
    let last = bed.sinch_calls(SEND_PATH).pop().unwrap();
    assert_ne!(header(&last, "authorization").as_deref(), Some("Bearer tok-1"));

    // the token kept until a minute before it runs out (3600 s), then another
    let client = moqui_lib::http::RestClient::new();
    let config = sinch_config(&bed);
    let sinch = Sinch::default();
    let at = now();
    let fetched = bed.sinch_calls(TOKEN_PATH).len();
    let token = sinch.token(&client, &config, at).unwrap();
    assert_eq!(sinch.token(&client, &config, at + 3_000_000).unwrap(), token);
    assert_eq!(bed.sinch_calls(TOKEN_PATH).len(), fetched + 1);
    assert_ne!(sinch.token(&client, &config, at + 3_550_000).unwrap(), token);
    assert_eq!(bed.sinch_calls(TOKEN_PATH).len(), fetched + 2);
    // no expires_in: an hour; one shorter than the minute kept back: fetched again for the next send
    bed.fake
        .script(&[Reply::Status(200, r#"{"access_token": "tok-plain"}"#.to_string())]);
    let plain = Sinch::default();
    assert_eq!(plain.token(&client, &config, at).unwrap(), "tok-plain");
    assert_eq!(plain.token(&client, &config, at + 3_500_000).unwrap(), "tok-plain");
    bed.fake.script(&[Reply::Status(
        200,
        r#"{"access_token": "tok-short", "expires_in": 30}"#.to_string(),
    )]);
    let short = Sinch::default();
    assert_eq!(short.token(&client, &config, at).unwrap(), "tok-short");
    assert_ne!(short.token(&client, &config, at + 1).unwrap(), "tok-short");
    // another keySecret under the same key id is another token
    let mut rotated = config.clone();
    rotated.key_secret = "rotated".to_string();
    let fetched = bed.sinch_calls(TOKEN_PATH).len();
    assert_ne!(sinch.token(&client, &rotated, at).unwrap(), token);
    assert_eq!(bed.sinch_calls(TOKEN_PATH).len(), fetched + 1);
    // the secrets are not in a Debug
    let shown = format!("{config:?}");
    assert!(!shown.contains("key-secret") && !shown.contains(SECRET), "{shown}");
    // the token service refusing the key, or away: Retry, the bridge not connected
    for status in [401, 500] {
        bed.fake.script(&[Reply::Status(status, "{}".to_string())]);
        let outcome = Sinch::default().token(&client, &config, at).unwrap_err();
        assert!(
            matches!(
                outcome,
                DeliveryOutcome::Retry {
                    not_connected: true,
                    ..
                }
            ),
            "{status}: {outcome:?}"
        );
    }
}

use moqui_core::service::chat_bridge::DeliveryOutcome;

/// Words go on the SMS channel from our number (`SMS_SENDER`), a file on the MMS channel (`MMS_SENDER`): words and a
/// file one card message; the delivery's id and the bridge's webhook URL with each; the accepted id is Sinch's
/// `message_id`. A file is refused without `mmsSender`, or from another number than it, and more than one file.
#[test]
fn a_sinch_text_goes_on_the_sms_channel_and_a_file_on_the_mms_channel() {
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    bed.sinch_text_in("Open?");
    let room = bed.room(THEIRS).unwrap();
    let words = bed.send(&room, "Until six", &[]);
    bed.round(0);
    let sent: Json = serde_json::from_str(&bed.sinch_calls(SEND_PATH)[0].body).unwrap();
    assert_eq!(sent["app_id"], "app-1");
    assert_eq!(
        sent["recipient"]["identified_by"]["channel_identities"],
        json!([{"channel": "SMS", "identity": THEIRS}])
    );
    assert_eq!(sent["message"], json!({"text_message": {"text": "Until six"}}));
    assert_eq!(sent["channel_properties"], json!({"SMS_SENDER": OURS}));
    // no callback_url: the reports come through the app's webhook, signed with its secret
    assert!(sent.get("callback_url").is_none(), "{sent}");
    let delivery_id = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(words))],
    )["chatBridgeDeliveryId"]
        .clone();
    assert_eq!(sent["correlation_id"], delivery_id.as_str());
    let (state, _, _, remote, _) = bed.delivery(&room, words);
    assert_eq!((state.as_str(), remote.as_str()), ("CdsAccepted", "sinch-2"));

    // words and a file: one card message on the MMS channel; a file alone, a media message
    let file = bed.upload(&room, "map.png", 100);
    let card = bed.send(&room, "Here", std::slice::from_ref(&file));
    let picture = bed.upload(&room, "pic.png", 100);
    let alone = bed.send(&room, "", std::slice::from_ref(&picture));
    bed.round(0);
    let sends = bed.sinch_calls(SEND_PATH);
    let card_sent: Json = serde_json::from_str(&sends[1].body).unwrap();
    assert_eq!(
        card_sent["recipient"]["identified_by"]["channel_identities"][0]["channel"],
        "MMS"
    );
    assert_eq!(card_sent["channel_properties"], json!({"MMS_SENDER": OURS}));
    assert_eq!(card_sent["message"]["card_message"]["description"], "Here");
    let url = card_sent["message"]["card_message"]["media_message"]["url"]
        .as_str()
        .unwrap();
    assert!(
        url.starts_with(&format!("https://moqui.example.test/channels/{BRIDGE}/media/{file}.")),
        "{url}"
    );
    let alone_sent: Json = serde_json::from_str(&sends[2].body).unwrap();
    assert!(
        alone_sent["message"]["media_message"]["url"].is_string(),
        "{alone_sent}"
    );
    assert_eq!(bed.delivery(&room, card).0, "CdsAccepted");
    assert_eq!(bed.delivery(&room, alone).0, "CdsAccepted");

    // two files: Sinch sends one a message
    let two = [bed.upload(&room, "a.png", 10), bed.upload(&room, "b.png", 10)];
    let many = bed.send(&room, "Two", &two);
    // no MMS channel set up: a file is refused, words still go
    bed.reconfigure(|config| {
        config.as_object_mut().unwrap().remove("mmsSender");
    });
    let unset = bed.send(&room, "No MMS", &[bed.upload(&room, "c.png", 10)]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, many).1, "tooManyFiles");
    let (state, reason, ..) = bed.delivery(&room, unset);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "mmsNotSetUp"));
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(unset))],
    );
    assert!(row["lastErrorText"].contains("no mmsSender"), "{row:?}");
    // the MMS sender another number: never from another number than the room's
    bed.reconfigure(|config| config["mmsSender"] = json!("+15550002000"));
    let other = bed.send(&room, "Other", &[bed.upload(&room, "d.png", 10)]);
    let still = bed.send(&room, "Words go", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, other).1, "mmsNotSetUp");
    assert_eq!(bed.delivery(&room, still).0, "CdsAccepted");
    assert_eq!(bed.sinch_calls(SEND_PATH).len(), 4);
}

/// Sinch's answers: 429 tried again and taken; an answer lost after the request went uncertain, never sent again; the
/// project's credit limit, a key refused for the project (403) and the wrong region (404) held with the bridge not
/// connected, saying which, and sent once fixed; another 4xx refused.
#[test]
fn sinch_answers_are_read_by_their_class() {
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    bed.sinch_text_in("Hi");
    let room = bed.room(THEIRS).unwrap();
    // a first send fetches the token, so the script below is the sends'
    let first = bed.send(&room, "First", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, first).0, "CdsAccepted");

    bed.fake.script(&[Reply::Status(
        429,
        r#"{"error":{"code":429,"message":"Too many requests","status":"RESOURCE_EXHAUSTED"}}"#.to_string(),
    )]);
    let busy = bed.send(&room, "Busy", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, busy).0, "CdsPending");
    bed.round(10_000);
    assert_eq!(bed.delivery(&room, busy).0, "CdsAccepted");

    bed.fake.script(&[Reply::Drop]);
    let lost = bed.send(&room, "Lost", &[]);
    bed.round(20_000);
    assert_eq!(bed.delivery(&room, lost).0, "CdsUncertain");
    let sent = bed.sinch_calls(SEND_PATH).len();
    bed.round(140_000);
    assert_eq!(bed.sinch_calls(SEND_PATH).len(), sent, "not sent again");

    // the bridge's setup (the credit limit, the key's project, the region): held, the bridge not connected, and sent
    // once it is fixed; a text Sinch refuses for what it is fails
    let billing = r#"{"error":{"code":403,"message":"Credit limit breached","status":"PERMISSION_DENIED",
        "details":[{"type_url":"type.googleapis.com/google.rpc.ErrorInfo","value":"BILLING_CREDIT_LIMIT_BREACHED"}]}}"#;
    let denied = r#"{"error":{"code":403,"message":"Permission denied","status":"PERMISSION_DENIED"}}"#;
    let region = r#"{"error":{"code":404,"message":"App not found","status":"NOT_FOUND"}}"#;
    let mut at = 200_000;
    for (status, body, said) in [
        (403, billing, "billingLimit"),
        (403, denied, "providerRefused"),
        (404, region, "wrongRegion"),
    ] {
        bed.fake.script(&[Reply::Status(status, body.to_string())]);
        let held = bed.send(&room, said, &[]);
        bed.round(at);
        let row = bed.one(
            "moqui.chat.ChatBridgeDelivery",
            &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(held))],
        );
        assert_eq!(row["statusEnumId"], "CdsPending", "{said}: {row:?}");
        assert!(row["lastErrorText"].starts_with(said), "{row:?}");
        assert_eq!(
            bed.one("moqui.chat.ChatBridge", &[("chatBridgeId", text(BRIDGE))])["statusId"],
            "CbsNotConnected",
            "{said}"
        );
        at += 3_600_000;
        bed.round(at);
        assert_eq!(bed.delivery(&room, held).0, "CdsAccepted", "{said}: sent once fixed");
        at += 3_600_000;
    }
    let invalid = r#"{"error":{"code":400,"message":"Invalid identity","status":"INVALID_ARGUMENT"}}"#;
    bed.fake.script(&[Reply::Status(400, invalid.to_string())]);
    let refused = bed.send(&room, "Invalid", &[]);
    bed.round(at);
    let (state, reason, ..) = bed.delivery(&room, refused);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "providerRefused"));
}

/// In: a text (Sinch's numbers without `+` taken as E.164), a file fetched, delivery reports (delivered, failed with
/// its reason, a queued one left), an opt-out and an opt-in Sinch took, each once.
#[test]
fn sinch_texts_files_receipts_and_opt_outs_come_in() {
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    let (response, _) = bed.sinch_text_in("Hello there");
    assert_eq!(
        body_json(&response)["outcome"],
        "accepted",
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let room = bed.room(THEIRS).expect("the pair room of their number in E.164");
    assert_eq!(bed.messages(&room)[0].1, "Hello there");

    // a file, and a file with its caption (each file of an MMS comes as its own event, so each its own message)
    let picture = bed.fake.file("pic.jpg", "image/jpeg", &[1, 2, 3]);
    let media = inbound("01MEDIA1", THEIRS, json!({"media_message": {"url": picture}}));
    assert_eq!(
        body_json(&bed.hook(&bed.signed(&media, now() / 1000)))["outcome"],
        "accepted"
    );
    let captioned = inbound(
        "01MEDIA2",
        THEIRS,
        json!({"media_card_message": {"url": picture, "caption": "Our dog"}}),
    );
    assert_eq!(
        body_json(&bed.hook(&bed.signed(&captioned, now() / 1000)))["outcome"],
        "accepted"
    );
    assert_eq!(bed.messages(&room).len(), 3);
    assert_eq!(bed.messages(&room)[2].1, "Our dog");
    assert_eq!(
        bed.count("moqui.chat.ChatAttachment", &[("chatRoomId", text(&room))]),
        2
    );

    // receipts: queued left, delivered kept; a failed one fails the delivery with its reason
    let seq = bed.send(&room, "Until six", &[]);
    let other = bed.send(&room, "Or seven", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, seq).3;
    let queued = bed.hook(&bed.signed(&delivery_report(&remote, "QUEUED_ON_CHANNEL"), now() / 1000));
    assert_eq!(body_json(&queued)["ignored"], "QUEUED_ON_CHANNEL");
    let delivered = bed.hook(&bed.signed(&delivery_report(&remote, "DELIVERED"), now() / 1000));
    assert_eq!(body_json(&delivered)["found"], true);
    assert_eq!(bed.delivery(&room, seq).4, "delivered");
    let other_remote = bed.delivery(&room, other).3;
    bed.hook(&bed.signed(&delivery_report(&other_remote, "FAILED"), now() / 1000));
    let (state, reason, ..) = bed.delivery(&room, other);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "deliveryFailed"));
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(other))],
    );
    assert!(row["lastErrorText"].contains("The phone is off"), "{row:?}");

    // an opt-out Sinch took: STOP for the bridge's smsSender; the same request again is a duplicate
    let opt_out = bed.signed(
        &opt_notice("opt_out_notification", "req-1", "OPT_OUT_SUCCEEDED"),
        now() / 1000,
    );
    assert_eq!(body_json(&bed.hook(&opt_out))["keyword"], "STOP");
    assert_eq!(body_json(&bed.hook(&opt_out))["outcome"], "duplicate");
    let opted = |bed: &Bed| {
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
        )["optedOut"]
            .clone()
    };
    assert_eq!(opted(&bed), "Y");
    // Sinch's retry of it (the same request_id under a new signature) is applied once: START after it still wins below
    let resent = bed.signed(
        &opt_notice("opt_out_notification", "req-1", "OPT_OUT_SUCCEEDED"),
        now() / 1000,
    );
    let again = bed.hook(&resent);
    assert_eq!(status(&again), 200, "{}", String::from_utf8_lossy(&again.body));
    // one that failed is left; an opt-in opts back in
    let failed = bed.signed(
        &opt_notice("opt_out_notification", "req-2", "OPT_OUT_FAILED"),
        now() / 1000,
    );
    assert_eq!(body_json(&bed.hook(&failed))["ignored"], "OPT_OUT_FAILED");
    let opt_in = bed.signed(
        &opt_notice("opt_in_notification", "req-3", "OPT_IN_SUCCEEDED"),
        now() / 1000,
    );
    assert_eq!(body_json(&bed.hook(&opt_in))["keyword"], "START");
    assert_eq!(opted(&bed), "N");
    // a message on another channel of the app (Messenger) is no text: left, no room
    let mut messenger = inbound(
        "01MSGR1",
        "+15550004444",
        json!({"text_message": {"text": "Hi from Messenger"}}),
    );
    messenger["message"]["channel_identity"] =
        json!({"channel": "MESSENGER", "identity": "2734981234", "app_id": "app-1"});
    let left = bed.hook(&bed.signed(&messenger, now() / 1000));
    assert_eq!(body_json(&left)["ignored"], "a message on channel MESSENGER");
    assert!(bed.room("+2734981234").is_none() && bed.room("+15550004444").is_none());
    // another event is left
    let other_event = json!({"app_id": "app-1", "conversation_start_notification": {}});
    assert!(body_json(&bed.hook(&bed.signed(&other_event, now() / 1000)))["ignored"].is_string());
}

/// The signature: good; a changed body, another secret, none, no nonce, a timestamp out of the window either side, a
/// signature that is not base64, another algorithm are the one refusal; the same request again is a duplicate, not worked again.
#[test]
fn a_sinch_webhook_is_signed_with_its_secret_within_its_window() {
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    let event = inbound("01SIG1", THEIRS, json!({"text_message": {"text": "Signed"}}));
    let request = bed.signed(&event, now() / 1000);
    assert_eq!(body_json(&bed.hook(&request))["outcome"], "accepted");
    let replay = bed.hook(&request);
    assert_eq!(body_json(&replay)["outcome"], "duplicate");
    assert!(!replay.auth_failure);

    let mut tampered = request.clone();
    tampered.body = event.to_string().replace("Signed", "Send money").into_bytes();
    let mut forged = request.clone();
    forged.headers = sinch_headers("another-secret", &request.body, now() / 1000, "HmacSHA256");
    let mut unsigned = bed.signed(&event, now() / 1000);
    unsigned.headers.retain(|(name, _)| name != "x-sinch-webhook-signature");
    let mut no_nonce = bed.signed(&event, now() / 1000);
    no_nonce
        .headers
        .retain(|(name, _)| name != "x-sinch-webhook-signature-nonce");
    let stale = bed.signed(&event, now() / 1000 - 600);
    let ahead = bed.signed(&event, now() / 1000 + 600);
    let mut not_base64 = bed.signed(&event, now() / 1000);
    for (name, value) in &mut not_base64.headers {
        if name == "x-sinch-webhook-signature" {
            *value = "not base64 at all!".to_string();
        }
    }
    let mut other_algorithm = request.clone();
    other_algorithm.headers = sinch_headers(SECRET, &request.body, now() / 1000, "HmacSHA1");
    for bad in [
        &tampered,
        &forged,
        &unsigned,
        &no_nonce,
        &stale,
        &ahead,
        &not_base64,
        &other_algorithm,
    ] {
        assert_eq!(bed.hook(bad), WebhookResponse::not_signed());
    }
    let room = bed.room(THEIRS).unwrap();
    assert_eq!(bed.messages(&room).len(), 1, "nothing refused was taken");
}

/// The Conversation API has no group texts: the adapter says `group_max` 0, and a send to a group mapping is refused
/// saying why, before Sinch.
#[test]
fn a_sinch_bridge_refuses_a_group_room() {
    use moqui_core::service::chat_bridge::{ChatBridgeAdapter as _, DeliveryKind, DeliveryRequest};
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    let adapter = crate::sms::SmsAdapter::new(Sinch::default());
    let bridge = bridge_info(&bed);
    assert_eq!(adapter.capabilities(&bridge).group_max, 0);
    assert!(adapter.start(&bridge).is_ok());
    let request = DeliveryRequest {
        chat_bridge_delivery_id: "D1".to_string(),
        idempotency_key: "moqui-chat:D1".to_string(),
        chat_room_id: "R1".to_string(),
        event_seq: 1,
        external_room_id: format!("{OURS}/{THEIRS},+15553334444"),
        external_thread_id: None,
        kind: DeliveryKind::Message,
        text: Some("Hi all".to_string()),
        html: None,
        sender_name: None,
        sender_user_id: Some("alice".to_string()),
        attachments: Vec::new(),
        target_remote_id: None,
        reaction_key: None,
        rich: None,
        attempt: 1,
    };
    match adapter.deliver(&bridge, &request) {
        DeliveryOutcome::Failed { reason_code, error } => {
            assert_eq!(reason_code, "groupTextsNotSupported");
            assert!(error.contains("Sinch does not carry group texts"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    assert!(bed.fake.sends().is_empty());
    // an unknown region is refused at the start
    let mut config = bridge_config(&bed);
    config["region"] = json!("ap");
    let wrong = moqui_core::service::chat_bridge::BridgeInfo { config, ..bridge };
    assert!(adapter.start(&wrong).unwrap_err().contains("region ap"));
}

/// Behind an edge node: Sinch's webhooks are checked on the local node (the secret is never on the edge node), and one
/// the edge node queued an hour ago is verified there with the time it came: a forged one refused and kept (the item
/// done, nothing taken), a good one handed on to the adapter; a bridge with no webhookSecret here cannot check, and
/// its item waits.
#[test]
fn a_sinch_webhook_an_edge_node_queued_is_verified_on_the_local_node() {
    use moqui_core::service::chat_bridge::{ChatBridgeAdapter as _, WebhookVerification};
    let bed = bed_for(SINCH, "");
    let _entered = bed.ctx.enter();
    assert_eq!(
        crate::sms::SmsAdapter::new(Sinch::default()).webhook_verification(),
        WebhookVerification::Core {
            signature_header: "x-sinch-webhook-signature"
        }
    );
    row(
        &bed.facade,
        "moqui.security.UserAccount",
        &[
            ("userId", text("EDGE_DELEGATE")),
            ("username", text(moqui_core::edge::delegate::EDGE_USERNAME)),
        ],
    );
    let received = now() - 3_600_000;
    let queued = |item: &str, request: &WebhookRequest| {
        row(
            &bed.facade,
            "moqui.edge.EdgeReceipt",
            &[("pairedNodeId", text("edge-1")), ("queueItemId", text(item))],
        );
        let headers: Vec<[&str; 2]> = request
            .headers
            .iter()
            .map(|(name, value)| [name.as_str(), value.as_str()])
            .collect();
        let payload = json!({"chatBridgeId": BRIDGE, "method": "POST", "url": request.url, "headers": headers,
            "body": base64::engine::general_purpose::STANDARD.encode(&request.body), "clientIp": request.client_ip,
            "receivedMillis": received});
        login(moqui_core::edge::delegate::EDGE_USERNAME);
        call(
            "org.moqui.chat.ChatBridgeServices.apply#EdgeWebhook",
            &[
                ("queueItemId", text(item)),
                ("pairedNodeId", text("edge-1")),
                ("payload", text(&payload.to_string())),
            ],
        )
    };
    let forged_event = inbound("01EDGE2", THEIRS, json!({"text_message": {"text": "Forged"}}));
    let mut forged = bed.signed(&forged_event, received / 1000);
    forged.headers = sinch_headers("guess", &forged.body, received / 1000, "HmacSHA256");
    // refused: answered as done (no retryLater, apply#EdgeQueueItem's one answer), not taken, counted
    let answer = queued("item-2", &forged);
    assert!(!answer.contains_key("retryLater"), "{answer:?}");
    assert!(bed.room(THEIRS).is_none(), "the forged one was not taken");
    assert_eq!(
        moqui_core::service::chat_bridge::refused_webhooks(bed.ctx.factory(), BRIDGE),
        1
    );

    let event = inbound("01EDGE1", THEIRS, json!({"text_message": {"text": "Through the edge"}}));
    let good = bed.signed(&event, received / 1000);
    // verified with the time it came and handed to the adapter, not refused. This test stops there: inside the queue's
    // transaction the in-memory test facade cannot store ingest#Event's batch in a transaction of its own, so the
    // adapter answers 503 and the item retryLater. No test here or in moqui-host takes a Sinch text through the queue
    // into its room; the direct webhook tests above take the same request through the same parse and receive#Sms.
    queued("item-1", &good);
    errors();
    assert_eq!(
        moqui_core::service::chat_bridge::refused_webhooks(bed.ctx.factory(), BRIDGE),
        1,
        "only the forged one refused"
    );
    // the same request checked at the time it came is good, and now (an hour on) out of the window
    let adapter = crate::sms::SmsAdapter::new(Sinch::default());
    let mut as_queued = good.clone();
    as_queued.now_millis = received;
    assert_eq!(adapter.verify_webhook(&bridge_info(&bed), &as_queued), None);
    assert_eq!(
        adapter.verify_webhook(&bridge_info(&bed), &good),
        Some(WebhookResponse::not_signed())
    );

    // no webhookSecret on this node: it cannot check, so the item waits rather than being refused as forged
    bed.reconfigure(|config| {
        config.as_object_mut().unwrap().remove("webhookSecret");
    });
    let waiting = queued("item-3", &good);
    assert!(
        waiting
            .get("retryLater")
            .is_some_and(|why| to_plain_string(why).contains("webhookSecret")),
        "{waiting:?}"
    );
    errors();
    assert_eq!(
        moqui_core::service::chat_bridge::refused_webhooks(bed.ctx.factory(), BRIDGE),
        1,
        "not refused"
    );
}
