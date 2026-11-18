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

//! Business RCS (plans/Comms-Messaging.md §16 RCS phase 1, docs/interfaces/RCS.md): the `rcs-twilio` and `rcs-sinch`
//! adapters on the SMS test bed (`crate::tests`), a bridge of type `CbtRcs`, against a fake Twilio and a fake Sinch:
//! each send's fields with and without the fallback and with a file; the path a message went, from the send and from
//! the reports, never back from the fallback to RCS; RCS messages, places and tapped suggestions in, to the home
//! number's room; a reply to a fallback text to the RCS room, to a same-provider SMS bridge's room, and left for
//! another provider's; the RCS opt-out; the capabilities; the bridges' settings; and the system's number kept on SMS.

use super::sinch::{digits, sinch_headers, SECRET};
use super::twilio::{form, posted, sid, value, ACCOUNT, API_KEY, TOKEN};
use super::*;
use crate::providers::sinch::Sinch;
use crate::providers::twilio::{self, Twilio};
use crate::sms::SmsProvider as _;
use moqui_core::service::chat_bridge::{ChatBridgeAdapter as _, Encryption};

const SERVICE: &str = concat!("MG", "0123456789abcdef", "0123456789abcdef");
/// The RCS sender's id, as Twilio names it in an address (`rcs:brand_..._agent`).
const AGENT: &str = "rcs:brand_spec_agent";
/// A number of ours on an SMS bridge of the same provider (`SMS2`), and one on another provider's (`SMS3`).
const OTHER: &str = "+15550002000";
const THIRD: &str = "+15550003000";

const TWILIO_RCS: Provider = Provider {
    adapter_name: "rcs-twilio",
    bridge_name: "Twilio RCS",
    config: |base, _keys| {
        json!({"accountSid": ACCOUNT, "apiKeySid": API_KEY, "apiKeySecret": "key-secret", "authToken": TOKEN,
            "messagingServiceSid": SERVICE, "fallbackFrom": OURS, "apiBaseUrl": base, "conversationsBaseUrl": base,
            "mediaContentBaseUrl": base, "allowInternalAddresses": true, "maxMediaCount": 3, "maxMediaBytes": 2000})
    },
    accepted: |n, _path| Reply::Status(201, json!({"sid": sid("SM", n), "status": "accepted"}).to_string()),
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

const TOKEN_PATH: &str = "/oauth2/token";
const SEND_PATH: &str = "/v1/projects/proj-1/messages:send";

const SINCH_RCS: Provider = Provider {
    adapter_name: "rcs-sinch",
    bridge_name: "Sinch RCS",
    config: |base, _keys| {
        json!({"projectId": "proj-1", "appId": "app-1", "keyId": "key-1", "keySecret": "key-secret", "region": "us",
            "apiBaseUrl": base, "authBaseUrl": base, "webhookSecret": SECRET, "smsSender": OURS, "mmsSender": OURS,
            "allowInternalAddresses": true})
    },
    accepted: |n, path| {
        if path == TOKEN_PATH {
            Reply::Status(
                200,
                json!({"access_token": format!("tok-{n}"), "expires_in": 3600}).to_string(),
            )
        } else {
            Reply::Status(200, json!({"message_id": format!("sinch-{n}")}).to_string())
        }
    },
    sign: |_keys, body, at_seconds, _url| sinch_headers(SECRET, body, at_seconds, "HmacSHA256"),
};

/// The bed for `provider` with its bridge an RCS one (`CbtRcs`), and two SMS bridges beside it: `SMS2` on the same
/// provider with [`OTHER`], `SMS3` on Telnyx with [`THIRD`], both routed to alice.
fn rcs_bed(provider: Provider, same_provider_sms: &str) -> Bed {
    let bed = bed_for(provider, "");
    {
        let _entered = bed.ctx.enter();
        let _authz = bed.ctx.disable_authz();
        let mut bridge = bed
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        bridge.set("typeEnumId", text("CbtRcs"));
        bridge.update(&bed.facade).unwrap();
        for (id, adapter, number) in [("SMS2", same_provider_sms, OTHER), ("SMS3", "sms", THIRD)] {
            row(
                &bed.facade,
                "moqui.chat.ChatBridge",
                &[
                    ("chatBridgeId", text(id)),
                    ("typeEnumId", text("CbtSms")),
                    ("adapterName", text(adapter)),
                    ("name", text(id)),
                    ("statusId", text("CbsNotConnected")),
                ],
            );
            row(
                &bed.facade,
                "moqui.call.PhoneNumber",
                &[
                    ("phoneNumber", text(number)),
                    ("messagingBridgeId", text(id)),
                    ("routeTypeEnumId", text("PnrUser")),
                    ("routeId", text("alice")),
                ],
            );
        }
    }
    bed
}

fn rcs_bridge(adapter: &str, config: Json) -> moqui_core::service::chat_bridge::BridgeInfo {
    moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtRcs".to_string(),
        adapter_name: adapter.to_string(),
        name: "RCS".to_string(),
        config,
    }
}

impl Bed {
    /// The room of `bridge`'s mapping `ours/theirs`.
    fn room_on(&self, bridge: &str, ours: &str, theirs: &str) -> Option<String> {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatRoomBridge")
            .unwrap()
            .eq("chatBridgeId", text(bridge))
            .eq("externalRoomId", text(&format!("{ours}/{theirs}")))
            .one()
            .unwrap()
            .and_then(|row| row.get("chatRoomId").map(to_plain_string))
    }

    /// Change the RCS bridge's configuration.
    fn set_config(&self, change: impl FnOnce(&mut Json)) {
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

    /// A Twilio message in (`from`, `to`, its other parameters), signed now.
    fn rcs_in(&self, from: &str, to: &str, extra: &[(&str, &str)]) -> WebhookResponse {
        let message = sid("SM", 5000 + self.next());
        let mut pairs = vec![
            ("MessageSid", message.as_str()),
            ("AccountSid", ACCOUNT),
            ("MessagingServiceSid", SERVICE),
            ("From", from),
            ("To", to),
            ("NumMedia", "0"),
        ];
        pairs.extend_from_slice(extra);
        self.hook(&self.signed_bytes(form(&pairs), now() / 1000))
    }

    /// A Twilio status callback of `message` from `from` (the agent's address, or the fallback's number).
    fn rcs_status(&self, message: &str, status: &str, from: &str) -> WebhookResponse {
        let to = format!("rcs:{THEIRS}");
        let pairs = [
            ("MessageSid", message),
            ("AccountSid", ACCOUNT),
            ("From", from),
            ("To", to.as_str()),
            ("MessageStatus", status),
        ];
        self.hook(&self.signed_bytes(form(&pairs), now() / 1000))
    }

    /// The path kept on the delivery of `seq` in `room`.
    fn path(&self, room: &str, seq: i64) -> String {
        self.one(
            "moqui.chat.ChatBridgeDelivery",
            &[("chatRoomId", text(room)), ("eventSeq", Value::I64(seq))],
        )
        .get("deliveryPath")
        .cloned()
        .unwrap_or_default()
    }

    /// A Sinch event of `body`, signed now.
    fn sinch_event(&self, body: &Json) -> WebhookResponse {
        self.hook(&self.signed(body, now() / 1000))
    }
}

/// A Sinch `MESSAGE_INBOUND` on the `RCS` channel (its `sender_id` the agent's, no number).
fn sinch_rcs_in(id: &str, content: Json) -> Json {
    json!({"app_id": "app-1", "project_id": "proj-1", "accepted_time": "2026-11-01T12:00:00Z",
        "message": {"id": id, "direction": "TO_APP", "contact_message": content,
            "channel_identity": {"channel": "RCS", "identity": digits(THEIRS), "app_id": "app-1"},
            "sender_id": "brand_spec_agent", "accept_time": "2026-11-01T12:00:00Z"}})
}

/// A Sinch `MESSAGE_DELIVERY` of `id` in `status` on `channel`.
fn sinch_report(id: &str, status: &str, channel: &str) -> Json {
    json!({"app_id": "app-1", "project_id": "proj-1", "event_time": "2026-11-01T12:00:04Z",
        "message_delivery_report": {"message_id": id, "status": status,
            "channel_identity": {"channel": channel, "identity": digits(THEIRS), "app_id": "app-1"}}})
}

// ---- Twilio ----

#[test]
fn an_rcs_bridge_is_encrypted_in_transit_has_no_groups_and_needs_its_sender_and_home_number() {
    for (adapter, name, config, missing) in [
        (
            crate::sms::SmsAdapter::new(Twilio::rcs()).name().to_string(),
            "rcs-twilio",
            json!({"accountSid": ACCOUNT, "authToken": TOKEN, "messagingServiceSid": SERVICE, "fallbackFrom": OURS}),
            ["messagingServiceSid", "fallbackFrom"],
        ),
        (
            crate::sms::SmsAdapter::new(Sinch::rcs()).name().to_string(),
            "rcs-sinch",
            json!({"projectId": "p", "appId": "a", "keyId": "k", "keySecret": "s", "webhookSecret": "w",
                "smsSender": OURS}),
            ["smsSender", "smsSender"],
        ),
    ] {
        assert_eq!(adapter, name);
        let caps = match name {
            "rcs-twilio" => crate::sms::SmsAdapter::new(Twilio::rcs()).capabilities(&rcs_bridge(name, config.clone())),
            _ => crate::sms::SmsAdapter::new(Sinch::rcs()).capabilities(&rcs_bridge(name, config.clone())),
        };
        assert_eq!(
            caps.encryption,
            Encryption::Transport,
            "{name}: RCS is encrypted in transit only"
        );
        assert_eq!(caps.group_max, 0, "{name}: RBM has no group chats");
        assert!(
            !(caps.edit || caps.delete || caps.reaction || caps.idempotent),
            "{name}"
        );
        let start = |config: &Json| match name {
            "rcs-twilio" => crate::sms::SmsAdapter::new(Twilio::rcs()).start(&rcs_bridge(name, config.clone())),
            _ => crate::sms::SmsAdapter::new(Sinch::rcs()).start(&rcs_bridge(name, config.clone())),
        };
        assert!(start(&config).is_ok(), "{name}: {:?}", start(&config));
        for key in missing {
            let mut without = config.clone();
            without.as_object_mut().unwrap().remove(key);
            let error = start(&without).unwrap_err();
            assert!(error.contains(key), "{name} without {key}: {error}");
        }
    }
    // the SMS adapters are unchanged
    let sms = crate::sms::SmsAdapter::new(Twilio::default());
    assert_eq!(
        sms.capabilities(&rcs_bridge("sms-twilio", json!({}))).encryption,
        Encryption::None
    );
}

#[test]
fn twilio_rcs_comes_in_to_the_home_numbers_room_and_goes_out_with_its_fallback_and_path() {
    let bed = rcs_bed(TWILIO_RCS, "sms-twilio");
    let _entered = bed.ctx.enter();
    // an RCS message names the agent: its room is the home number's (fallbackFrom), keyed by their number
    let response = bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi from RCS")]);
    assert_eq!(status(&response), 200, "{:?}", String::from_utf8_lossy(&response.body));
    let room = bed.room(THEIRS).expect("the RCS room of the home number and theirs");
    let topic = bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))])["topic"].clone();
    assert!(topic.starts_with("RCS and texts with"), "{topic}");

    // a place with no words: said with a map link built from its coordinates; a tapped suggestion kept for R2
    let placed = bed.rcs_in(
        &format!("rcs:{THEIRS}"),
        AGENT,
        &[
            ("Latitude", "45.5"),
            ("Longitude", "-122.6"),
            ("Label", "The shop"),
            ("Address", "1 Main St"),
        ],
    );
    assert_eq!(status(&placed), 200, "{:?}", String::from_utf8_lossy(&placed.body));
    bed.rcs_in(
        &format!("rcs:{THEIRS}"),
        AGENT,
        &[
            ("Body", "Yes please"),
            ("ButtonPayload", "order:yes"),
            ("ButtonText", "Yes please"),
        ],
    );
    let texts: Vec<String> = bed.messages(&room).into_iter().map(|(_, body)| body).collect();
    assert_eq!(texts[0], "Hi from RCS");
    assert!(texts[1].starts_with("Location: The shop, 1 Main St"), "{}", texts[1]);
    assert!(
        texts[1].contains("https://www.openstreetmap.org/?mlat=45.5&mlon=-122.6"),
        "{}",
        texts[1]
    );
    assert_eq!(texts[2], "Yes please");
    let _authz = bed.ctx.disable_authz();
    let metadata: Vec<String> = bed
        .facade
        .find("moqui.chat.ChatEventExternal")
        .unwrap()
        .eq("chatRoomId", text(&room))
        .list()
        .unwrap()
        .values()
        .iter()
        .filter_map(|row| row.get("metadataJson").map(to_plain_string))
        .collect();
    assert!(
        metadata.iter().any(|json| json.contains("\"postback\":\"order:yes\"")),
        "{metadata:?}"
    );
    assert!(
        metadata.iter().any(|json| json.contains("\"latitude\":\"45.5\"")),
        "{metadata:?}"
    );

    // the reply: RCS first through the service, the fallback from the room's number, the receipts to the webhook
    let seq = bed.send(&room, "Thanks", &[]);
    bed.round(0);
    let sent = bed.fake.sends().pop().unwrap();
    let pairs = posted(&sent);
    assert_eq!(value(&pairs, "To"), Some(THEIRS));
    assert_eq!(value(&pairs, "MessagingServiceSid"), Some(SERVICE));
    assert_eq!(value(&pairs, "FallbackFrom"), Some(OURS));
    assert_eq!(value(&pairs, "From"), None);
    assert!(value(&pairs, "StatusCallback").is_some());
    let (state, .., remote, _) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsAccepted");
    assert_eq!(bed.path(&room, seq), "", "not known until a report says");

    // the report's From: the agent's address is RCS; then the fallback's number is SMS; RCS again changes nothing
    assert_eq!(status(&bed.rcs_status(&remote, "sent", AGENT)), 200);
    assert_eq!(bed.path(&room, seq), "rcs");
    bed.rcs_status(&remote, "delivered", OURS);
    assert_eq!(bed.path(&room, seq), "sms");
    bed.rcs_status(&remote, "read", AGENT);
    assert_eq!(bed.path(&room, seq), "sms", "a fallback never goes back to RCS");
    assert_eq!(bed.delivery(&room, seq).4, "delivered");
    let deliveries = ok(
        "org.moqui.chat.ChatBridgeServices.get#EventDeliveries",
        &[
            ("chatRoomId", text(&room)),
            ("eventSeqList", Value::List(vec![Value::I64(seq)])),
        ],
    );
    let map = moqui_lib::json::to_json(deliveries.get("deliveryMap").unwrap());
    assert_eq!(map[seq.to_string()]["destinations"][0]["path"], "sms", "{map}");

    // a file that fell back went as MMS
    let file = bed.upload(&room, "photo.png", 100);
    let with_file = bed.send(&room, "A photo", &[file]);
    bed.round(0);
    let remote = bed.delivery(&room, with_file).3;
    bed.rcs_status(&remote, "delivered", OURS);
    assert_eq!(bed.path(&room, with_file), "mms");
}

#[test]
fn without_its_fallback_a_twilio_rcs_send_goes_rcs_only_and_its_path_is_known_at_once() {
    let bed = rcs_bed(TWILIO_RCS, "sms-twilio");
    let _entered = bed.ctx.enter();
    bed.set_config(|config| config["smsFallback"] = json!(false));
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    let seq = bed.send(&room, "RCS only", &[]);
    bed.round(0);
    let pairs = posted(&bed.fake.sends().pop().unwrap());
    assert_eq!(value(&pairs, "To"), Some(format!("rcs:{THEIRS}").as_str()));
    assert_eq!(value(&pairs, "FallbackFrom"), None);
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(bed.path(&room, seq), "rcs");
}

#[test]
fn a_reply_to_a_fallback_text_goes_to_the_rcs_room_or_to_its_numbers_own_sms_room() {
    let bed = rcs_bed(TWILIO_RCS, "sms-twilio");
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    // an SMS to the RCS bridge's own number: its RCS room
    assert_eq!(status(&bed.rcs_in(THEIRS, OURS, &[("Body", "By SMS")])), 200);
    assert_eq!(bed.messages(&room).last().unwrap().1, "By SMS");
    // to a number of an SMS bridge of the same provider: that number's own room, on its bridge
    let response = bed.rcs_in(THEIRS, OTHER, &[("Body", "To the other number")]);
    assert_eq!(status(&response), 200);
    let other = bed.room_on("SMS2", OTHER, THEIRS).expect("the SMS bridge's pair room");
    assert_ne!(other, room);
    assert_eq!(bed.messages(&other).last().unwrap().1, "To the other number");
    // to a number of another provider's bridge: left
    bed.rcs_in(THEIRS, THIRD, &[("Body", "Elsewhere")]);
    assert!(bed.room_on("SMS3", THIRD, THEIRS).is_none());
    assert!(bed.room_on(BRIDGE, THIRD, THEIRS).is_none());
    // an SMS bridge takes no RCS message
    let sms = Twilio::default();
    let request = bed.signed_bytes(
        form(&[
            ("MessageSid", &sid("SM", 1)),
            ("From", &format!("rcs:{THEIRS}")),
            ("To", AGENT),
            ("Body", "x"),
        ]),
        now() / 1000,
    );
    assert!(matches!(sms.parse(&request), Ok(crate::sms::SmsEvent::Ignored(_))));
}

#[test]
fn the_systems_number_stays_on_an_sms_bridge() {
    let bed = rcs_bed(TWILIO_RCS, "sms-twilio");
    let _entered = bed.ctx.enter();
    for (entity, fields) in [
        (
            "moqui.security.UserAccount",
            vec![("userId", text("voiceadmin")), ("username", text("voiceadmin"))],
        ),
        ("moqui.security.UserGroup", vec![("userGroupId", text("VADMIN"))]),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", text("VADMIN")),
                ("userId", text("voiceadmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", text("VADMIN")),
                ("userPermissionId", text("VoiceAdmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
    ] {
        row(&bed.facade, entity, &fields);
    }
    login("voiceadmin");
    // the bed's system number is OURS: its texts cannot go through the RCS bridge
    let out = call(
        "org.moqui.call.VoiceServices.store#PhoneNumber",
        &[("phoneNumber", text(OURS)), ("messagingBridgeId", text(BRIDGE))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "systemNumber");
    assert!(errors().contains("sign-in codes"));
    // another number may, and then cannot become the system's number
    ok(
        "org.moqui.call.VoiceServices.store#PhoneNumber",
        &[("phoneNumber", text(OTHER)), ("messagingBridgeId", text(BRIDGE))],
    );
    let out = call(
        "org.moqui.call.VoiceServices.update#VoicePolicy",
        &[("smsSystemNumber", text(OTHER))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "systemNumber");
    assert!(!errors().is_empty());
    ok(
        "org.moqui.call.VoiceServices.update#VoicePolicy",
        &[("smsSystemNumber", text(THIRD))],
    );
}

// ---- Sinch ----

#[test]
fn sinch_rcs_goes_out_rcs_first_with_its_fallback_and_comes_in_on_the_rcs_channel() {
    let bed = rcs_bed(SINCH_RCS, "sms-sinch");
    let _entered = bed.ctx.enter();
    // an RCS text (its sender_id the agent's): the home number's room
    let response = bed.sinch_event(&sinch_rcs_in("01RCS1", json!({"text_message": {"text": "Hi"}})));
    assert_eq!(status(&response), 200, "{:?}", String::from_utf8_lossy(&response.body));
    let room = bed.room(THEIRS).expect("the RCS room of smsSender and theirs");
    bed.sinch_event(&sinch_rcs_in(
        "01RCS2",
        json!({"location_message": {"title": "The shop", "label": "1 Main St",
            "coordinates": {"latitude": 45.5, "longitude": -122.6}}}),
    ));
    bed.sinch_event(&sinch_rcs_in(
        "01RCS3",
        json!({"choice_response_message": {"message_id": "m1", "postback_data": "order:yes"}}),
    ));
    let texts: Vec<String> = bed.messages(&room).into_iter().map(|(_, body)| body).collect();
    assert_eq!(texts[0], "Hi");
    assert!(texts[1].starts_with("Location: The shop, 1 Main St"), "{}", texts[1]);
    assert_eq!(texts[2], "order:yes");
    // the SMS adapter still leaves an RCS message
    let request = bed.signed(
        &sinch_rcs_in("01RCS4", json!({"text_message": {"text": "x"}})),
        now() / 1000,
    );
    assert!(matches!(
        Sinch::default().parse(&request),
        Ok(crate::sms::SmsEvent::Ignored(_))
    ));

    // the reply: RCS, then SMS from the room's number
    let seq = bed.send(&room, "Thanks", &[]);
    bed.round(0);
    let sends: Vec<Seen> = bed
        .fake
        .sends()
        .into_iter()
        .filter(|seen| seen.path == SEND_PATH)
        .collect();
    let body: Json = serde_json::from_str(&sends.last().unwrap().body).unwrap();
    assert_eq!(body["channel_priority_order"], json!(["RCS", "SMS"]));
    assert_eq!(
        body["recipient"]["identified_by"]["channel_identities"],
        json!([{"channel": "RCS", "identity": THEIRS}, {"channel": "SMS", "identity": THEIRS}])
    );
    assert_eq!(body["channel_properties"], json!({"SMS_SENDER": OURS}));
    // a file: its fallback is MMS
    let file = bed.upload(&room, "photo.png", 100);
    let with_file = bed.send(&room, "A photo", &[file]);
    bed.round(0);
    let body: Json = serde_json::from_str(&bed.fake.sends().pop().unwrap().body).unwrap();
    assert_eq!(body["channel_priority_order"], json!(["RCS", "MMS"]));
    assert_eq!(body["channel_properties"], json!({"MMS_SENDER": OURS}));
    // without the fallback: RCS alone
    bed.set_config(|config| config["smsFallback"] = json!(false));
    let only = bed.send(&room, "RCS only", &[]);
    bed.round(0);
    let body: Json = serde_json::from_str(&bed.fake.sends().pop().unwrap().body).unwrap();
    assert_eq!(body["channel_priority_order"], json!(["RCS"]));
    assert_eq!(
        body["recipient"]["identified_by"]["channel_identities"],
        json!([{"channel": "RCS", "identity": THEIRS}])
    );
    assert_eq!(bed.path(&room, only), "rcs");

    // the reports: RCS delivered; a switch to the fallback, then SMS delivered; MMS for the file
    let remote = bed.delivery(&room, seq).3;
    bed.sinch_event(&sinch_report(&remote, "SWITCHING_CHANNEL", "RCS"));
    assert_eq!(bed.path(&room, seq), "sms");
    bed.sinch_event(&sinch_report(&remote, "DELIVERED", "SMS"));
    assert_eq!(bed.path(&room, seq), "sms");
    assert_eq!(bed.delivery(&room, seq).4, "delivered");
    let remote = bed.delivery(&room, with_file).3;
    bed.sinch_event(&sinch_report(&remote, "DELIVERED", "RCS"));
    assert_eq!(bed.path(&room, with_file), "rcs");

    // an RCS opt-out is the person's STOP of the pair
    bed.sinch_event(&json!({"app_id": "app-1", "project_id": "proj-1",
        "opt_out_notification": {"request_id": "opt-1", "channel": "RCS", "identity": digits(THEIRS),
            "status": "OPT_OUT_SUCCEEDED"}}));
    let opt = bed.one(
        "moqui.call.SmsOptOut",
        &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
    );
    assert_eq!(opt["optedOut"], "Y");
}
