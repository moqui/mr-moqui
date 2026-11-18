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
//! Phase 13 (§18.3): rich messages in each provider's form (Twilio's Content made for the send and deleted once done,
//! or an app's own; Sinch's choice, card and carousel messages with our fallback), an SMS bridge sending the fallback,
//! the read receipts, and the typing indicator.

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
/// Numbers of ours on other bridges ([`rcs_bed`]): an SMS bridge of the same provider and account (`SMS2`), another
/// provider's (`SMS3`), one of the same provider and another account (`SMS4`), and a disabled one (`SMS5`).
const OTHER: &str = "+15550002000";
const THIRD: &str = "+15550003000";
const FOURTH: &str = "+15550004000";
const FIFTH: &str = "+15550005000";

const TWILIO_RCS: Provider = Provider {
    adapter_name: "rcs-twilio",
    bridge_name: "Twilio RCS",
    config: |base, _keys| {
        json!({"accountSid": ACCOUNT, "apiKeySid": API_KEY, "apiKeySecret": "key-secret", "authToken": TOKEN,
            "messagingServiceSid": SERVICE, "fallbackFrom": OURS, "apiBaseUrl": base, "conversationsBaseUrl": base,
            "mediaContentBaseUrl": base, "contentBaseUrl": base, "messagingBaseUrl": base, "rcsSenderId": AGENT,
            "allowInternalAddresses": true, "maxMediaCount": 3, "maxMediaBytes": 2000})
    },
    accepted: |n, path| {
        if path == CONTENT_PATH {
            Reply::Status(201, json!({"sid": sid("HX", n), "friendly_name": "moqui"}).to_string())
        } else if path.starts_with(CONTENT_PATH) {
            Reply::Status(204, String::new())
        } else if path == TYPING_PATH {
            Reply::Status(200, json!({"success": true}).to_string())
        } else {
            Reply::Status(201, json!({"sid": sid("SM", n), "status": "accepted"}).to_string())
        }
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

const TOKEN_PATH: &str = "/oauth2/token";
const SEND_PATH: &str = "/v1/projects/proj-1/messages:send";
const EVENTS_PATH: &str = "/v1/projects/proj-1/events:send";
/// Twilio's Content API (a content made for a send; its delete is this and the id) and its typing indicator.
const CONTENT_PATH: &str = "/v1/Content";
const TYPING_PATH: &str = "/v3/Indicators/Typing.json";

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

/// The bed for `provider` with its bridge an RCS one (`CbtRcs`, its one number the home number [`OURS`]), and SMS
/// bridges beside it, each with a number routed to alice: `SMS2` of `same_provider_sms` on the RCS bridge's account
/// (`account`) with [`OTHER`], `SMS3` on Telnyx with [`THIRD`], `SMS4` of `same_provider_sms` on another account
/// (`other_account`) with [`FOURTH`], and `SMS5` like `SMS2` but disabled, with [`FIFTH`].
fn rcs_bed(provider: Provider, same_provider_sms: &str, account: &Json, other_account: &Json) -> Bed {
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
        let none = json!({});
        for (id, adapter, number, config, status) in [
            ("SMS2", same_provider_sms, OTHER, account, "CbsNotConnected"),
            ("SMS3", "sms", THIRD, &none, "CbsNotConnected"),
            ("SMS4", same_provider_sms, FOURTH, other_account, "CbsNotConnected"),
            ("SMS5", same_provider_sms, FIFTH, account, "CbsDisabled"),
        ] {
            row(
                &bed.facade,
                "moqui.chat.ChatBridge",
                &[
                    ("chatBridgeId", text(id)),
                    ("typeEnumId", text("CbtSms")),
                    ("adapterName", text(adapter)),
                    ("name", text(id)),
                    ("statusId", text(status)),
                    ("configJson", text(&config.to_string())),
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

fn twilio_bed() -> Bed {
    let other = concat!("AC", "fedcba9876543210", "fedcba9876543210");
    rcs_bed(
        TWILIO_RCS,
        "sms-twilio",
        &json!({"accountSid": ACCOUNT}),
        &json!({"accountSid": other}),
    )
}

fn sinch_bed() -> Bed {
    rcs_bed(
        SINCH_RCS,
        "sms-sinch",
        &json!({"projectId": "proj-1", "appId": "app-1"}),
        &json!({"projectId": "proj-1", "appId": "app-2"}),
    )
}

/// Sign in a user with `permission` (VoiceAdmin, ChatBridgeAdmin).
fn admin(bed: &Bed, permission: &str) {
    let user = format!("admin-{permission}");
    let group = format!("G-{permission}");
    for (entity, fields) in [
        (
            "moqui.security.UserAccount",
            vec![("userId", text(&user)), ("username", text(&user))],
        ),
        ("moqui.security.UserGroup", vec![("userGroupId", text(&group))]),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", text(&group)),
                ("userId", text(&user)),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", text(&group)),
                ("userPermissionId", text(permission)),
                ("fromDate", Value::I64(0)),
            ],
        ),
    ] {
        row(&bed.facade, entity, &fields);
    }
    login(&user);
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
        self.hook(&self.rcs_request(ACCOUNT, from, to, extra))
    }

    /// The request of a Twilio message in, of `account`, signed now.
    fn rcs_request(&self, account: &str, from: &str, to: &str, extra: &[(&str, &str)]) -> WebhookRequest {
        let message = sid("SM", 5000 + self.next());
        let mut pairs = vec![
            ("MessageSid", message.as_str()),
            ("AccountSid", account),
            ("MessagingServiceSid", SERVICE),
            ("From", from),
            ("To", to),
            ("NumMedia", "0"),
        ];
        pairs.extend_from_slice(extra);
        self.signed_bytes(form(&pairs), now() / 1000)
    }

    /// The mappings of `bridge` whose external room id has `number`.
    fn rooms_with(&self, bridge: &str, number: &str) -> usize {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatRoomBridge")
            .unwrap()
            .eq("chatBridgeId", text(bridge))
            .list()
            .unwrap()
            .values()
            .iter()
            .filter(|row| {
                row.get("externalRoomId")
                    .map(to_plain_string)
                    .unwrap_or_default()
                    .contains(number)
            })
            .count()
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
    let bed = twilio_bed();
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
    assert_eq!(bed.delivery(&room, seq).4, "read");
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
    let bed = twilio_bed();
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
fn a_reply_to_a_fallback_text_goes_to_the_rcs_room_or_to_a_same_account_sms_bridges_room() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    // an SMS to the RCS bridge's own number: its RCS room
    assert_eq!(status(&bed.rcs_in(THEIRS, OURS, &[("Body", "By SMS")])), 200);
    assert_eq!(bed.messages(&room).last().unwrap().1, "By SMS");
    // to a number of an SMS bridge of the same provider and account: that number's own room, on its bridge
    let response = bed.rcs_in(THEIRS, OTHER, &[("Body", "To the other number")]);
    assert_eq!(status(&response), 200);
    let other = bed.room_on("SMS2", OTHER, THEIRS).expect("the SMS bridge's pair room");
    assert_ne!(other, room);
    assert_eq!(bed.messages(&other).last().unwrap().1, "To the other number");
    // a payload of another account to that number: left
    let stranger = concat!("AC", "1111111111111111", "1111111111111111");
    assert_eq!(
        status(&bed.hook(&bed.rcs_request(stranger, THEIRS, OTHER, &[("Body", "Not ours")]))),
        200
    );
    assert_eq!(bed.messages(&other).last().unwrap().1, "To the other number");
    // to a number of a same-provider bridge of another account, of another provider's bridge: left
    bed.rcs_in(THEIRS, FOURTH, &[("Body", "Another account")]);
    assert_eq!(bed.rooms_with("SMS4", FOURTH), 0);
    bed.rcs_in(THEIRS, THIRD, &[("Body", "Elsewhere")]);
    assert_eq!(bed.rooms_with("SMS3", THIRD) + bed.rooms_with(BRIDGE, THIRD), 0);
    // to a disabled bridge's number: left before anything is kept, a STOP's opt-out too
    bed.rcs_in(THEIRS, FIFTH, &[("Body", "STOP")]);
    assert_eq!(bed.rooms_with("SMS5", FIFTH), 0);
    assert_eq!(bed.count("moqui.call.SmsOptOut", &[("phoneNumber", text(FIFTH))]), 0);
    // outside the RCS bridge's own webhook (no mark): left
    let newcomer = "+15557770001";
    let info = moqui_core::service::chat_bridge::BridgeInfo {
        type_enum_id: "CbtRcs".to_string(),
        ..bridge_info(&bed)
    };
    let request = bed.rcs_request(ACCOUNT, newcomer, OTHER, &[("Body", "No mark")]);
    crate::sms::SmsAdapter::new(Twilio::rcs()).apply_webhook(&info, &request);
    assert!(bed.room_on("SMS2", OTHER, newcomer).is_none());
    // a sender that is not in E.164 (a short code): taken as sms-twilio takes it, in a room of its own
    assert_eq!(status(&bed.rcs_in("rcs:22395", AGENT, &[("Body", "Your code")])), 200);
    assert_eq!(bed.rooms_with(BRIDGE, "22395"), 1);
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
fn the_webhook_mark_is_put_back_after_a_hand_over_also_when_it_fails() {
    use moqui_core::service::chat_bridge::WEBHOOK_MARK;
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    let mark = || ExecutionContext::with_current(|ec| ec.shared_map().get(WEBHOOK_MARK).map(to_plain_string)).flatten();
    assert_eq!(crate::sms::with_mark("SMS2", mark).as_deref(), Some("SMS2"));
    assert_eq!(mark(), None, "no mark before, none after");
    ExecutionContext::with_current(|ec| {
        ec.shared_map().insert(WEBHOOK_MARK.to_string(), text(BRIDGE));
    });
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::sms::with_mark("SMS2", || -> () { panic!("the SMS room failed") })
    }));
    assert!(failed.is_err());
    assert_eq!(mark().as_deref(), Some(BRIDGE), "the RCS bridge's mark is back");
}

#[test]
fn an_rcs_bridge_has_one_number_its_home_number_never_the_systems() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    admin(&bed, "VoiceAdmin");
    let store = |number: &str, bridge: &str| {
        let out = call(
            "org.moqui.call.VoiceServices.store#PhoneNumber",
            &[("phoneNumber", text(number)), ("messagingBridgeId", text(bridge))],
        );
        (out_text(&out, "reasonCode"), errors())
    };
    // the bed's system number is OURS: its texts cannot go through the RCS bridge
    let (reason, error) = store(OURS, BRIDGE);
    assert_eq!(reason, "systemNumber");
    assert!(error.contains("sign-in codes"), "{error}");
    // a second number on the RCS bridge: refused, its replies and STOPs would come to the home number's pair
    let (reason, error) = store(OTHER, BRIDGE);
    assert_eq!(reason, "rcsOneNumber");
    assert!(error.contains("one number"), "{error}");
    // a number of an RCS bridge never becomes the system's
    let out = call(
        "org.moqui.call.VoiceServices.update#VoicePolicy",
        &[("smsSystemNumber", text(OURS))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "systemNumber");
    assert!(!errors().is_empty());
    ok(
        "org.moqui.call.VoiceServices.update#VoicePolicy",
        &[("smsSystemNumber", text(THIRD))],
    );
    // a new RCS bridge takes only its home number
    let home = "+15550006000";
    row(
        &bed.facade,
        "moqui.chat.ChatBridge",
        &[
            ("chatBridgeId", text("RCS2")),
            ("typeEnumId", text("CbtRcs")),
            ("adapterName", text("rcs-twilio")),
            ("name", text("RCS2")),
            ("statusId", text("CbsDisabled")),
            ("configJson", text(&json!({"fallbackFrom": home}).to_string())),
        ],
    );
    let (reason, error) = store("+15550007000", "RCS2");
    assert_eq!(reason, "notHomeNumber");
    assert!(error.contains("fallbackFrom"), "{error}");
    let (reason, error) = store(home, "RCS2");
    assert!(reason.is_empty() && error.is_empty(), "{reason} {error}");
}

#[test]
fn a_bridges_network_and_adapter_agree_and_an_rcs_bridge_keeps_one_number() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    admin(&bed, "ChatBridgeAdmin");
    // the networks, as ChatEntities.xml's seed data has them
    for (id, code) in [("CbtRcs", "rcs"), ("CbtSms", "sms")] {
        row(
            &bed.facade,
            "moqui.basic.Enumeration",
            &[
                ("enumId", text(id)),
                ("enumCode", text(code)),
                ("enumTypeId", text("ChatBridgeType")),
            ],
        );
    }
    let store = |fields: &[(&str, Value)]| {
        call("org.moqui.chat.ChatBridgeServices.store#Bridge", fields);
        errors()
    };
    let error = store(&[
        ("name", text("Wrong")),
        ("typeEnumId", text("CbtRcs")),
        ("adapterName", text("sms-twilio")),
    ]);
    assert!(error.contains("rcs-twilio or rcs-sinch"), "{error}");
    let error = store(&[
        ("name", text("Wrong")),
        ("typeEnumId", text("CbtSms")),
        ("adapterName", text("rcs-twilio")),
    ]);
    assert!(error.contains("sms, sms-twilio or sms-sinch"), "{error}");
    // SMS4 with the system's number: not made an RCS bridge
    {
        let _authz = bed.ctx.disable_authz();
        let mut policy = bed
            .facade
            .find("moqui.call.VoicePolicy")
            .unwrap()
            .eq("voicePolicyId", text("DEFAULT"))
            .one()
            .unwrap()
            .unwrap();
        policy.set("smsSystemNumber", text(FOURTH));
        policy.update(&bed.facade).unwrap();
    }
    let to_rcs = |bridge: &str, home: &str| {
        store(&[
            ("chatBridgeId", text(bridge)),
            ("typeEnumId", text("CbtRcs")),
            ("adapterName", text("rcs-twilio")),
            ("configJson", text(&json!({"fallbackFrom": home}).to_string())),
        ])
    };
    assert!(to_rcs("SMS4", FOURTH).contains("system"));
    // SMS3 with two numbers: not made an RCS bridge
    row(
        &bed.facade,
        "moqui.call.PhoneNumber",
        &[
            ("phoneNumber", text("+15550008000")),
            ("messagingBridgeId", text("SMS3")),
            ("routeTypeEnumId", text("PnrUser")),
            ("routeId", text("alice")),
        ],
    );
    assert!(to_rcs("SMS3", THIRD).contains("one number"));
    // SMS2 with its one number: only with it as the home number
    assert!(to_rcs("SMS2", "+15550009000").contains(OTHER));
    assert_eq!(to_rcs("SMS2", OTHER), "");
    assert_eq!(
        bed.one("moqui.chat.ChatBridge", &[("chatBridgeId", text("SMS2"))])["typeEnumId"],
        "CbtRcs"
    );
}

// ---- Sinch ----

#[test]
fn sinch_rcs_goes_out_rcs_first_with_its_fallback_and_comes_in_on_the_rcs_channel() {
    let bed = sinch_bed();
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

#[test]
fn a_sinch_reply_to_a_fallback_text_goes_to_a_same_account_sms_bridges_room() {
    let bed = sinch_bed();
    let _entered = bed.ctx.enter();
    // an SMS on the RCS app to a number of ours (its sender_id), of the app's project and app id
    let sms_in = |id: &str, to: &str, body: &str| {
        json!({"app_id": "app-1", "project_id": "proj-1", "accepted_time": "2026-11-01T12:00:00Z",
            "message": {"id": id, "direction": "TO_APP", "contact_message": {"text_message": {"text": body}},
                "channel_identity": {"channel": "SMS", "identity": digits(THEIRS), "app_id": "app-1"},
                "sender_id": digits(to), "accept_time": "2026-11-01T12:00:00Z"}})
    };
    assert_eq!(
        status(&bed.sinch_event(&sms_in("01S1", OURS, "To the home number"))),
        200
    );
    let room = bed.room(THEIRS).expect("the RCS room");
    assert_eq!(bed.messages(&room).last().unwrap().1, "To the home number");
    // SMS2 is of the same project and app: its room
    assert_eq!(
        status(&bed.sinch_event(&sms_in("01S2", OTHER, "To the other number"))),
        200
    );
    let other = bed.room_on("SMS2", OTHER, THEIRS).expect("the SMS bridge's pair room");
    assert_eq!(bed.messages(&other).last().unwrap().1, "To the other number");
    // SMS4 is another app's: left
    bed.sinch_event(&sms_in("01S3", FOURTH, "Another app"));
    assert_eq!(bed.rooms_with("SMS4", FOURTH), 0);
}

// ---- rich messages, read receipts and typing (§18.3 phase 13) ----

/// alice sends `rich` (with `files`, the cards' pictures) in `room`: its eventSeq.
fn send_rich(room: &str, rich: Json, files: &[String]) -> i64 {
    login("alice");
    let out = ok(
        "org.moqui.chat.ChatRichServices.send#Rich",
        &[
            ("chatRoomId", text(room)),
            ("rich", moqui_lib::json::from_json(&rich)),
            (
                "chatAttachmentIdList",
                Value::List(files.iter().map(|id| text(id)).collect()),
            ),
        ],
    );
    out_text(&out, "eventSeq").parse().unwrap()
}

/// A card with a reply, a link and a call, its picture the message's first file.
fn a_card() -> Json {
    json!({"kind": "card", "cards": [{"title": "Your order shipped", "description": "Arrives Tuesday", "attachmentSeq": 1,
        "suggestions": [{"type": "reply", "text": "Thanks", "postback": "order:7:thanks"},
            {"type": "url", "text": "Track", "url": "https://example.test/track/7"},
            {"type": "dial", "text": "Call us", "phone": "+15550109999"}]}]})
}

impl Bed {
    /// The sends to the fake on `path`.
    fn sent_to(&self, path: &str) -> Vec<Seen> {
        self.fake.sends().into_iter().filter(|seen| seen.path == path).collect()
    }

    /// The sends to the fake whose path starts with `prefix`.
    fn sent_under(&self, prefix: &str) -> Vec<Seen> {
        self.fake
            .sends()
            .into_iter()
            .filter(|seen| seen.path.starts_with(prefix))
            .collect()
    }

    /// The content kept on the delivery of `seq` in `room`.
    fn content_of(&self, room: &str, seq: i64) -> String {
        self.one(
            "moqui.chat.ChatBridgeDelivery",
            &[("chatRoomId", text(room)), ("eventSeq", Value::I64(seq))],
        )
        .get("providerContentId")
        .cloned()
        .unwrap_or_default()
    }

    /// `send#BridgeTyping` of `room` at `at`: the bridges asked.
    fn typing(&self, room: &str, at: i64) -> i64 {
        let _authz = self.ctx.disable_authz();
        let out = ok(
            "org.moqui.chat.ChatBridgeServices.send#BridgeTyping",
            &[("chatRoomId", text(room)), ("nowMillis", Value::I64(at))],
        );
        out_text(&out, "sent").parse().unwrap_or(0)
    }
}

#[test]
fn a_twilio_rcs_card_is_a_content_made_for_its_send_and_deleted_once_its_message_is_done() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    let picture = bed.upload(&room, "parcel.png", 100);
    let seq = send_rich(&room, a_card(), &[picture]);
    bed.round(0);

    // first the content: the card, and the fallback (its text and picture as MMS)
    let made = bed.sent_to(CONTENT_PATH);
    assert_eq!(made.len(), 1, "one content for the send");
    let content: Json = serde_json::from_str(&made[0].body).unwrap();
    assert!(
        content["friendly_name"].as_str().unwrap().starts_with("moqui-"),
        "{content}"
    );
    assert_eq!(content["language"], "en");
    let card = &content["types"]["twilio/card"];
    assert_eq!(card["title"], "Your order shipped");
    assert_eq!(card["body"], "Arrives Tuesday");
    let media = card["media"][0].as_str().unwrap();
    assert!(media.contains("/channels/") && media.contains("/media/"), "{media}");
    assert_eq!(
        card["actions"],
        json!([{"type": "QUICK_REPLY", "title": "Thanks", "id": "order:7:thanks", "chip_list": false},
            {"type": "URL", "title": "Track", "url": "https://example.test/track/7", "chip_list": false},
            {"type": "PHONE_NUMBER", "title": "Call us", "phone": "+15550109999", "chip_list": false}])
    );
    let fallback = &content["types"]["twilio/media"];
    assert_eq!(
        fallback["body"],
        "Your order shipped\nArrives Tuesday\nTrack: https://example.test/track/7\nCall us: +15550109999\nReply: Thanks"
    );
    assert_eq!(fallback["media"][0], media);
    assert!(content["types"].get("twilio/text").is_none());

    // then the send names it, with no Body or MediaUrl of its own
    let pairs = posted(&bed.fake.sends().pop().unwrap());
    let content_sid = value(&pairs, "ContentSid").unwrap().to_string();
    assert!(content_sid.starts_with("HX"), "{content_sid}");
    assert_eq!(value(&pairs, "Body"), None);
    assert_eq!(value(&pairs, "MediaUrl"), None);
    assert_eq!(value(&pairs, "FallbackFrom"), Some(OURS));
    assert_eq!(bed.content_of(&room, seq), content_sid, "kept for its delete");
    // the room's other clients see the fallback text
    assert!(bed
        .messages(&room)
        .last()
        .unwrap()
        .1
        .starts_with("Your order shipped\nArrives Tuesday"));

    // sent is not done; delivered is: the content is deleted and let go
    let remote = bed.delivery(&room, seq).3;
    bed.rcs_status(&remote, "sent", AGENT);
    assert!(bed.sent_under(&format!("{CONTENT_PATH}/")).is_empty());
    bed.rcs_status(&remote, "delivered", AGENT);
    let deleted = bed.sent_under(&format!("{CONTENT_PATH}/"));
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].path, format!("{CONTENT_PATH}/{content_sid}"));
    assert_eq!(bed.content_of(&room, seq), "");
    // read: past delivered, and never back
    bed.rcs_status(&remote, "read", AGENT);
    assert_eq!(bed.delivery(&room, seq).4, "read");
    bed.rcs_status(&remote, "delivered", AGENT);
    assert_eq!(bed.delivery(&room, seq).4, "read");
    assert_eq!(
        bed.sent_under(&format!("{CONTENT_PATH}/")).len(),
        1,
        "nothing left to delete"
    );
}

#[test]
fn a_twilio_text_with_chips_and_a_carousel_and_an_apps_own_template_go_in_twilios_forms() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();

    // a text with chips, RCS only: the chip list, and no fallback type
    bed.set_config(|config| config["smsFallback"] = json!(false));
    send_rich(
        &room,
        json!({"kind": "text", "text": "Table for two?", "suggestions": [{"type": "reply", "text": "Yes"},
            {"type": "reply", "text": "No", "postback": "no"}]}),
        &[],
    );
    bed.round(0);
    let content: Json = serde_json::from_str(&bed.sent_to(CONTENT_PATH).pop().unwrap().body).unwrap();
    assert_eq!(
        content["types"],
        json!({"twilio/card": {"body": "Table for two?", "actions": [
            {"type": "QUICK_REPLY", "title": "Yes", "id": "Yes", "chip_list": true},
            {"type": "QUICK_REPLY", "title": "No", "id": "no", "chip_list": true}]}})
    );
    let pairs = posted(&bed.fake.sends().pop().unwrap());
    assert_eq!(value(&pairs, "To"), Some(format!("rcs:{THEIRS}").as_str()));
    bed.set_config(|config| config["smsFallback"] = json!(true));

    // a carousel: each card's words, picture and buttons; the fallback text with the first picture
    let files = [bed.upload(&room, "a.png", 10), bed.upload(&room, "b.png", 10)];
    send_rich(
        &room,
        json!({"kind": "carousel", "cards": [
            {"title": "Jacket", "description": "Light", "attachmentSeq": 1, "suggestions": [{"type": "reply", "text": "Save"}]},
            {"title": "Pack", "description": "Roomy", "attachmentSeq": 2, "suggestions": [{"type": "reply", "text": "Save"}]}]}),
        &files,
    );
    bed.round(0);
    let content: Json = serde_json::from_str(&bed.sent_to(CONTENT_PATH).pop().unwrap().body).unwrap();
    let cards = &content["types"]["twilio/carousel"]["cards"];
    assert_eq!(cards.as_array().unwrap().len(), 2);
    assert_eq!(cards[1]["title"], "Pack");
    assert_eq!(cards[1]["body"], "Roomy");
    assert!(cards[1]["media"].as_str().unwrap().contains("/media/"));
    assert_eq!(
        content["types"]["twilio/media"]["body"],
        "1. Jacket\nLight\nReply: Save\n\n2. Pack\nRoomy\nReply: Save"
    );

    // an app's own template: sent as it is, never made, kept or deleted
    let sends_before = bed.sent_to(CONTENT_PATH).len();
    let own = concat!("HX", "0123456789abcdef", "0123456789abcdef");
    let seq = {
        let _authz = bed.ctx.disable_authz();
        let out = ok(
            "org.moqui.chat.ChatRichServices.send#Rich",
            &[
                ("chatRoomId", text(&room)),
                ("senderUserId", text("alice")),
                (
                    "rich",
                    moqui_lib::json::from_json(&json!({"kind": "text", "text": "Sale today",
                        "suggestions": [{"type": "url", "text": "Shop", "url": "https://example.test/sale"}],
                        "providerContent": {"twilio": {"contentSid": own, "contentVariables": {"1": "Ann"}}}})),
                ),
            ],
        );
        out_text(&out, "eventSeq").parse::<i64>().unwrap()
    };
    bed.round(0);
    assert_eq!(bed.sent_to(CONTENT_PATH).len(), sends_before);
    let pairs = posted(&bed.fake.sends().pop().unwrap());
    assert_eq!(value(&pairs, "ContentSid"), Some(own));
    assert_eq!(value(&pairs, "ContentVariables"), Some(r#"{"1":"Ann"}"#));
    assert_eq!(bed.content_of(&room, seq), "");
    let remote = bed.delivery(&room, seq).3;
    bed.rcs_status(&remote, "delivered", AGENT);
    assert!(
        bed.sent_under(&format!("{CONTENT_PATH}/")).is_empty(),
        "an app's template is never deleted"
    );
}

#[test]
fn a_refused_twilio_content_fails_the_send_and_one_not_made_sends_nothing_yet() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    let chips = || json!({"kind": "text", "text": "Ready?", "suggestions": [{"type": "reply", "text": "Yes"}]});
    let messages_before = bed.sent_under("/2010-04-01").len();
    bed.fake.script(&[Reply::Status(
        400,
        json!({"code": 21656, "message": "Invalid content"}).to_string(),
    )]);
    let refused = send_rich(&room, chips(), &[]);
    bed.round(0);
    let (state, reason, ..) = bed.delivery(&room, refused);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "richRefused"));
    bed.fake.script(&[Reply::Status(503, "{}".to_string())]);
    let later = send_rich(&room, chips(), &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, later).0, "CdsPending", "tried again later");
    assert_eq!(bed.sent_under("/2010-04-01").len(), messages_before, "no message went");
}

#[test]
fn the_daily_sweep_deletes_the_contents_left_after_two_days_and_keeps_one_it_could_not() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    let chips = || json!({"kind": "text", "text": "Ready?", "suggestions": [{"type": "reply", "text": "Yes"}]});
    let first = send_rich(&room, chips(), &[]);
    let second = send_rich(&room, chips(), &[]);
    bed.round(0);
    let (one, two) = (bed.content_of(&room, first), bed.content_of(&room, second));
    assert!(!one.is_empty() && !two.is_empty());
    let sweep = |at: i64| {
        let _authz = bed.ctx.disable_authz();
        ok(
            "org.moqui.chat.ChatBridgeServices.clean#ProviderContent",
            &[("nowMillis", Value::I64(at))],
        )
    };
    // not yet two days old: kept
    assert_eq!(out_text(&sweep(now() + 86_400_000), "deleted"), "0");
    // past two days: one delete fails (kept for the next sweep), the other is done (a 404, Twilio has it no more)
    bed.fake.script(&[
        Reply::Status(500, "{}".to_string()),
        Reply::Status(404, "{}".to_string()),
    ]);
    let out = sweep(now() + 3 * 86_400_000);
    assert_eq!(
        (out_text(&out, "deleted"), out_text(&out, "kept")),
        ("1".to_string(), "1".to_string())
    );
    let left: Vec<String> = [first, second].iter().map(|seq| bed.content_of(&room, *seq)).collect();
    assert_eq!(left.iter().filter(|id| id.is_empty()).count(), 1, "{left:?}");
    let out = sweep(now() + 3 * 86_400_000);
    assert_eq!(out_text(&out, "deleted"), "1");
    assert_eq!(bed.content_of(&room, first), "");
    assert_eq!(bed.content_of(&room, second), "");
}

#[test]
fn sinch_rcs_rich_messages_go_as_choice_card_and_carousel_messages_with_our_fallback() {
    let bed = sinch_bed();
    let _entered = bed.ctx.enter();
    bed.sinch_event(&sinch_rcs_in("01RICH0", json!({"text_message": {"text": "Hi"}})));
    let room = bed.room(THEIRS).unwrap();
    let last_send = || -> Json { serde_json::from_str(&bed.sent_to(SEND_PATH).pop().unwrap().body).unwrap() };

    send_rich(
        &room,
        json!({"kind": "text", "text": "Table for two?", "suggestions": [{"type": "reply", "text": "Yes", "postback": "t:yes"},
            {"type": "dial", "text": "Call", "phone": "+15550109999"}]}),
        &[],
    );
    bed.round(0);
    let body = last_send();
    assert_eq!(
        body["message"],
        json!({"choice_message": {"text_message": {"text": "Table for two?"}, "choices": [
            {"text_message": {"text": "Yes"}, "postback_data": "t:yes"},
            {"call_message": {"title": "Call", "phone_number": "+15550109999"}}]}})
    );
    assert_eq!(
        body["explicit_channel_omni_message"],
        json!({"SMS": {"text_message": {"text": "Table for two?\n\nCall: +15550109999\nReply: Yes"}}})
    );
    assert_eq!(body["channel_priority_order"], json!(["RCS", "SMS"]));

    // a card with its picture: MMS is its fallback, a card of our text
    let picture = bed.upload(&room, "parcel.png", 100);
    let seq = send_rich(&room, a_card(), &[picture]);
    bed.round(0);
    let body = last_send();
    let card = &body["message"]["card_message"];
    assert_eq!(card["title"], "Your order shipped");
    assert_eq!(card["description"], "Arrives Tuesday");
    assert!(card["media_message"]["url"].as_str().unwrap().contains("/media/"));
    assert_eq!(
        card["choices"][1],
        json!({"url_message": {"title": "Track", "url": "https://example.test/track/7"}})
    );
    assert_eq!(body["channel_priority_order"], json!(["RCS", "MMS"]));
    let mms = &body["explicit_channel_omni_message"]["MMS"]["card_message"];
    assert_eq!(mms["title"], "Your order shipped");
    assert!(mms["description"].as_str().unwrap().ends_with("Reply: Thanks"));

    // a carousel's two pictures go in its cards (more than one file only for a rich message)
    let files = [bed.upload(&room, "a.png", 10), bed.upload(&room, "b.png", 10)];
    bed.set_config(|config| config["smsFallback"] = json!(false));
    send_rich(
        &room,
        json!({"kind": "carousel", "cards": [
            {"title": "Jacket", "attachmentSeq": 1, "suggestions": [{"type": "url", "text": "See", "url": "https://example.test/j"}]},
            {"title": "Pack", "attachmentSeq": 2, "suggestions": [{"type": "url", "text": "See", "url": "https://example.test/p"}]}]}),
        &files,
    );
    bed.round(0);
    let body = last_send();
    assert_eq!(
        body["message"]["carousel_message"]["cards"].as_array().unwrap().len(),
        2
    );
    assert!(
        body.get("explicit_channel_omni_message").is_none(),
        "no fallback: no override"
    );

    // READ is read, past delivered
    let remote = bed.delivery(&room, seq).3;
    bed.sinch_event(&sinch_report(&remote, "DELIVERED", "RCS"));
    bed.sinch_event(&sinch_report(&remote, "READ", "RCS"));
    assert_eq!(bed.delivery(&room, seq).4, "read");
}

#[test]
fn an_sms_bridge_sends_a_rich_messages_fallback_and_its_pictures() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "Hello", json!([]));
    let room = bed.room(THEIRS).unwrap();
    let picture = bed.upload(&room, "parcel.png", 100);
    let seq = send_rich(&room, a_card(), &[picture]);
    bed.round(0);
    let body: Json = serde_json::from_str(&bed.fake.sends().pop().unwrap().body).unwrap();
    assert_eq!(
        body["text"],
        "Your order shipped\nArrives Tuesday\nTrack: https://example.test/track/7\nCall us: +15550109999\nReply: Thanks"
    );
    assert_eq!(body["media_urls"].as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
}

#[test]
fn a_users_typing_shows_on_the_persons_phone_once_in_15_seconds_and_never_after_a_stop() {
    let bed = twilio_bed();
    let _entered = bed.ctx.enter();
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "Hi")]);
    let room = bed.room(THEIRS).unwrap();
    let start = now();
    assert_eq!(bed.typing(&room, start), 1);
    let typed = bed.sent_to(TYPING_PATH);
    assert_eq!(typed.len(), 1);
    let body: Json = serde_json::from_str(&typed[0].body).unwrap();
    assert_eq!(
        body,
        json!({"channel": "RCS", "from": "rcs:brand_spec_agent", "to": format!("rcs:{THEIRS}"), "event": "START"})
    );
    // again within the window: nothing
    assert_eq!(bed.typing(&room, start + 5_000), 0);
    assert_eq!(bed.sent_to(TYPING_PATH).len(), 1);
    // turned off on the bridge, then without the agent's id: asked, nothing sent
    bed.set_config(|config| config["typingIndicators"] = json!(false));
    bed.typing(&room, start + 20_000);
    bed.set_config(|config| {
        config["typingIndicators"] = json!(true);
        config.as_object_mut().unwrap().remove("rcsSenderId");
    });
    bed.typing(&room, start + 40_000);
    assert_eq!(bed.sent_to(TYPING_PATH).len(), 1);
    // the person sent STOP: none
    bed.set_config(|config| config["rcsSenderId"] = json!(AGENT));
    bed.rcs_in(&format!("rcs:{THEIRS}"), AGENT, &[("Body", "STOP")]);
    bed.typing(&room, start + 60_000);
    assert_eq!(bed.sent_to(TYPING_PATH).len(), 1);
}

#[test]
fn a_sinch_rcs_typing_indicator_is_a_composing_event() {
    let bed = sinch_bed();
    let _entered = bed.ctx.enter();
    bed.sinch_event(&sinch_rcs_in("01TYPE0", json!({"text_message": {"text": "Hi"}})));
    let room = bed.room(THEIRS).unwrap();
    assert_eq!(bed.typing(&room, now()), 1);
    let event = bed.sent_to(EVENTS_PATH).pop().expect("an event sent");
    assert!(event
        .headers
        .iter()
        .any(|(name, value)| name == "authorization" && value.starts_with("Bearer ")));
    let body: Json = serde_json::from_str(&event.body).unwrap();
    assert_eq!(body["event"], json!({"composing_event": {}}));
    assert_eq!(
        body["recipient"]["identified_by"]["channel_identities"],
        json!([{"channel": "RCS", "identity": THEIRS}])
    );
}
