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

//! The chat push rules (plans/Comms-Messaging.md §7.2, §7.6) through `send#Message`, `react#Message`, `mark#Read` and
//! `run#ChatPush` on a simulated clock, with a recording transport ([`set_transport`]) in place of the push services:
//! the room levels (mute as Mentions, an edit's new mention, a reply and a thread reply), a reaction that never pushes,
//! read on another device (no push, a clear), do not disturb held and sent as one summary, no push for a rolled-back
//! message, merges, retries with backoff and Retry-After, a gone device, TTL, collapse key and priority, the payload
//! encrypted to each device, the calls that ring through do not disturb, and the acknowledgements.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use moqui_lib::object::{to_plain_string, Value};
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

use super::*;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const CHAT: &str = "org.moqui.chat.ChatServices";
const PUSH: &str = "org.moqui.chat.ChatPushServices";
const DAY: i64 = 86_400_000;

// the user agent key pair and auth secret of RFC 8291 Appendix A, for every device here
const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

/// One push as the transport got it.
#[derive(Clone, Debug)]
struct Post {
    device: String,
    /// The FCM body, for an FCM device.
    fcm: Option<serde_json::Value>,
    /// The encrypted body, for a Web Push or UnifiedPush device.
    body: Vec<u8>,
    ttl: i64,
    urgency: String,
    topic: String,
}

/// A transport that keeps what it is given and answers each device from its script (delivered when none is left).
#[derive(Default)]
struct Recorder {
    posts: Mutex<Vec<Post>>,
    script: Mutex<HashMap<String, VecDeque<Sent>>>,
    /// A Web Push answer held this long, and the time past which a send counts as slow (none: the push services').
    slow: Mutex<Option<(std::time::Duration, std::time::Duration)>>,
    /// Each user's budget of a delivery round (none: the push services').
    budget: Mutex<Option<std::time::Duration>>,
}

impl Recorder {
    fn take(&self) -> Vec<Post> {
        std::mem::take(&mut *self.posts.lock().unwrap())
    }

    fn answer(&self, device: &str, answers: &[Sent]) {
        self.script
            .lock()
            .unwrap()
            .entry(device.to_string())
            .or_default()
            .extend(answers.iter().cloned());
    }

    fn next(&self, device: &str) -> Sent {
        self.script
            .lock()
            .unwrap()
            .get_mut(device)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Sent::Delivered)
    }
}

impl PushTransport for Recorder {
    fn available(&self, _provider: Provider) -> bool {
        true
    }

    fn fcm(&self, device: &str, message: &serde_json::Value) -> Sent {
        self.posts.lock().unwrap().push(Post {
            device: device.to_string(),
            fcm: Some(message.clone()),
            body: Vec::new(),
            ttl: 0,
            urgency: String::new(),
            topic: String::new(),
        });
        self.next(device)
    }

    fn web(&self, device: &str, _provider: Provider, post: &WebPost<'_>) -> Sent {
        self.posts.lock().unwrap().push(Post {
            device: device.to_string(),
            fcm: None,
            body: post.body.clone(),
            ttl: post.ttl_seconds,
            urgency: post.urgency.to_string(),
            topic: post.topic.to_string(),
        });
        let held = *self.slow.lock().unwrap();
        if let Some((hold, _)) = held {
            std::thread::sleep(hold);
        }
        self.next(device)
    }

    fn slow_after(&self) -> std::time::Duration {
        self.slow.lock().unwrap().map_or(SLOW_SEND, |(_, after)| after)
    }

    fn user_round_budget(&self) -> std::time::Duration {
        self.budget.lock().unwrap().unwrap_or(USER_ROUND_BUDGET)
    }
}

/// The user agent's side of RFC 8291: what the device reads.
fn decrypt(body: &[u8]) -> serde_json::Value {
    let ua_private = SecretKey::from_slice(&push_crypto::decode_b64(UA_PRIVATE).unwrap()).unwrap();
    let salt = &body[..16];
    let key_length = usize::from(body[20]);
    let as_public_bytes = &body[21..21 + key_length];
    let ciphertext = &body[21 + key_length..];
    let as_public = PublicKey::from_sec1_bytes(as_public_bytes).unwrap();
    let shared = p256::ecdh::diffie_hellman(ua_private.to_nonzero_scalar(), as_public.as_affine());
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&ua_private.public_key().to_sec1_bytes());
    key_info.extend_from_slice(as_public_bytes);
    let mut ikm = [0_u8; 32];
    Hkdf::<Sha256>::new(Some(&push_crypto::decode_b64(AUTH).unwrap()), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .unwrap();
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();
    let mut plain = Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .decrypt(&Nonce::try_from(&nonce[..]).unwrap(), ciphertext)
        .unwrap();
    assert_eq!(plain.pop(), Some(2));
    serde_json::from_slice(&plain).unwrap()
}

/// What a post says to its device: the FCM data's `enc`, or the Web Push body, decrypted.
fn payload(post: &Post) -> serde_json::Value {
    match &post.fcm {
        Some(body) => {
            let enc = body["message"]["data"]["enc"].as_str().expect("an encrypted payload");
            decrypt(&push_crypto::decode_b64(enc).unwrap())
        }
        None => decrypt(&post.body),
    }
}

fn devices(posts: &[Post]) -> BTreeSet<String> {
    posts.iter().map(|post| post.device.clone()).collect()
}

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

// ---- the bed ----

struct Bed {
    ctx: ExecutionContext,
    facade: EntityFacade,
    net: Arc<Recorder>,
    room: String,
    now: i64,
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn update(facade: &EntityFacade, entity: &str, key: &[(&str, &str)], fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade.find(entity).unwrap();
    for (field, value) in key {
        find = find.eq(field, text(value));
    }
    let fields: Vec<(String, Value)> = fields
        .iter()
        .map(|(field, value)| ((*field).to_string(), value.clone()))
        .collect();
    find.disable_authz().update_all(fields).unwrap();
}

fn rows(facade: &EntityFacade, entity: &str, key: &[(&str, &str)]) -> Vec<EntityValue> {
    let mut find = facade.find(entity).unwrap();
    for (field, value) in key {
        find = find.eq(field, text(value));
    }
    find.disable_authz().list().unwrap().values().to_vec()
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .unwrap_or_default()
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn number(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
        .unwrap_or(-1)
}

/// A device of `user`: FCM, Web Push or UnifiedPush, with the RFC's keys when `keyed`; a native one with a live device
/// key.
fn device(facade: &EntityFacade, id: &str, user: &str, provider: &str, keyed: bool) {
    let native = provider != "PushWeb";
    let login_key = format!("key-{id}");
    if native {
        row(
            facade,
            "moqui.security.UserLoginKey",
            &[
                ("loginKey", text(&login_key)),
                ("userId", text(user)),
                ("deviceName", text(id)),
                ("fromDate", Value::I64(0)),
            ],
        );
    }
    let mut fields = vec![
        ("subscriptionId", text(id)),
        ("userId", text(user)),
        ("pushProviderEnumId", text(provider)),
        (
            "endpoint",
            text(&if provider == "PushFcm" {
                format!("fcm-token-{id}")
            } else {
                format!("https://push.example.net/{id}")
            }),
        ),
        ("endpointHash", text(&format!("hash-{id}"))),
    ];
    if native {
        fields.push(("loginKey", text(&login_key)));
    }
    if keyed {
        fields.push(("p256dhKey", text(UA_PUBLIC)));
        fields.push(("authSecret", text(AUTH)));
    }
    row(facade, "moqui.security.user.WebPushSubscription", &fields);
}

/// Room Team of alice's with: bob (All; an FCM phone and a browser), carol (Mentions; an FCM phone that gave no keys),
/// dave (All; a UnifiedPush phone), erin (All but muted; an FCM phone), frank (Nothing; an FCM phone).
fn bed() -> Bed {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let net = Arc::new(Recorder::default());
    set_transport(ctx.factory(), net.clone());
    let room = {
        let _entered = ctx.enter();
        for id in ["alice", "bob", "carol", "dave", "erin", "frank"] {
            let mut name: Vec<char> = id.chars().collect();
            name[0] = name[0].to_ascii_uppercase();
            row(
                &facade,
                "moqui.security.UserAccount",
                &[
                    ("userId", text(id)),
                    ("username", text(id)),
                    ("userFullName", text(&name.into_iter().collect::<String>())),
                ],
            );
        }
        device(&facade, "BOB-PHONE", "bob", "PushFcm", true);
        device(&facade, "BOB-WEB", "bob", "PushWeb", true);
        device(&facade, "CAROL-PHONE", "carol", "PushFcm", false);
        device(&facade, "DAVE-UP", "dave", "PushUnified", true);
        device(&facade, "ERIN-PHONE", "erin", "PushFcm", true);
        device(&facade, "FRANK-PHONE", "frank", "PushFcm", true);
        login("alice");
        let room = ok(
            &format!("{CHAT}.create#Room"),
            &[
                ("roomTypeEnumId", text("CrtPrivate")),
                ("roomName", text("Team")),
                (
                    "memberUserIdList",
                    Value::List(
                        ["bob", "carol", "dave", "erin", "frank"]
                            .iter()
                            .map(|id| text(id))
                            .collect(),
                    ),
                ),
            ],
        );
        let room = field_of(&room, "chatRoomId");
        let member = |user: &str, fields: &[(&str, Value)]| {
            update(
                &facade,
                "moqui.chat.ChatRoomMember",
                &[("chatRoomId", &room), ("userId", user)],
                fields,
            );
        };
        member("carol", &[("notificationLevelEnumId", text("CnlMentions"))]);
        member("erin", &[("isMuted", text("Y"))]);
        member("frank", &[("notificationLevelEnumId", text("CnlNone"))]);
        room
    };
    Bed {
        ctx,
        facade,
        net,
        room,
        now: crate::user::now_millis(),
    }
}

fn field_of(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

/// A message from the user signed in; its eventSeq.
fn send(room: &str, body: &str, more: &[(&str, Value)]) -> i64 {
    let mut parameters = vec![("chatRoomId", text(room)), ("body", text(body))];
    parameters.extend(more.iter().cloned());
    number(&ok(&format!("{CHAT}.send#Message"), &parameters), "eventSeq")
}

fn mention(user: &str) -> String {
    format!(r#"<span data-mention-id="{user}">@{user}</span>"#)
}

fn run(at: i64) -> BTreeMap<String, Value> {
    ok(&format!("{PUSH}.run#ChatPush"), &[("nowMillis", Value::I64(at))])
}

fn outbox(facade: &EntityFacade, device: &str) -> Vec<EntityValue> {
    let mut found = rows(facade, OUTBOX, &[("subscriptionId", device)]);
    found.sort_by_key(|row| count_of(row, "eventSeq"));
    found
}

// ---- the rules ----

/// Rule 1: All gets every message, Mentions and a muted room only their mentions, replies and thread replies, Nothing
/// nothing, the sender nothing; an edit pushes only the mention it adds; a direct room starts at All.
#[test]
fn the_room_level_decides_who_is_pushed() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let first = send(&bed.room, "<p>Hello all</p>", &[]);
    assert_eq!(number(&run(bed.now), "jobCount"), 1);
    assert_eq!(devices(&bed.net.take()), set(["BOB-PHONE", "BOB-WEB", "DAVE-UP"]));

    // a mention reaches the Mentions level and a muted room, never Nothing
    let body = format!(
        "<p>{} {} {} look</p>",
        mention("carol"),
        mention("erin"),
        mention("frank")
    );
    send(&bed.room, &body, &[]);
    run(bed.now + 1_000);
    assert_eq!(
        devices(&bed.net.take()),
        set(["BOB-PHONE", "BOB-WEB", "DAVE-UP", "CAROL-PHONE", "ERIN-PHONE"])
    );

    // an edit pushes only the mention it adds
    ok(
        &format!("{CHAT}.edit#Message"),
        &[
            ("chatRoomId", text(&bed.room)),
            ("eventSeq", Value::I64(first)),
            ("body", text(&format!("<p>Hello all {}</p>", mention("carol")))),
        ],
    );
    run(bed.now + 2_000);
    assert_eq!(devices(&bed.net.take()), set(["CAROL-PHONE"]));

    // a reply to carol's message, and a reply in a thread erin started
    login("carol");
    let carols = send(&bed.room, "<p>mine</p>", &[]);
    login("erin");
    let erins = send(&bed.room, "<p>a thread</p>", &[]);
    run(bed.now + 3_000);
    bed.net.take();
    login("alice");
    send(&bed.room, "<p>to carol</p>", &[("replyToEventSeq", Value::I64(carols))]);
    send(
        &bed.room,
        "<p>in the thread</p>",
        &[("threadRootEventSeq", Value::I64(erins))],
    );
    run(bed.now + 4_000);
    let reached = devices(&bed.net.take());
    assert!(
        reached.contains("CAROL-PHONE") && reached.contains("ERIN-PHONE"),
        "{reached:?}"
    );
    assert!(!reached.contains("FRANK-PHONE"), "{reached:?}");
    let kinds: BTreeSet<String> = rows(&bed.facade, ACTIVITY, &[("chatRoomId", &bed.room)])
        .iter()
        .map(|row| format!("{} {}", field(row, "userId"), field(row, "activityKindEnumId")))
        .collect();
    assert!(
        kinds.contains("carol CuaReply") && kinds.contains("erin CuaThreadReply"),
        "{kinds:?}"
    );

    // a direct room starts at All
    let direct = field_of(
        &ok(
            &format!("{CHAT}.create#Room"),
            &[
                ("roomTypeEnumId", text("CrtDirect")),
                ("memberUserIdList", Value::List(vec![text("frank")])),
            ],
        ),
        "chatRoomId",
    );
    send(&direct, "<p>just you</p>", &[]);
    run(bed.now + 5_000);
    assert_eq!(devices(&bed.net.take()), set(["FRANK-PHONE"]));
}

/// Phase 11 (block and spam): a member whose room is in Spam & blocked gets no push, a mention included, and nor does a
/// user who blocked the sender; the others still do.
#[test]
fn a_filtered_room_and_a_blocked_sender_push_nothing() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    update(
        &bed.facade,
        "moqui.chat.ChatRoomMember",
        &[("chatRoomId", &bed.room), ("userId", "bob")],
        &[("filterEnumId", text("CmfSpam"))],
    );
    row(
        &bed.facade,
        "moqui.chat.ChatUserBlock",
        &[("userId", text("dave")), ("blockedUserId", text("alice"))],
    );
    login("alice");
    send(
        &bed.room,
        &format!("<p>{} {} hello</p>", mention("bob"), mention("dave")),
        &[],
    );
    run(bed.now);
    assert!(
        devices(&bed.net.take()).is_empty(),
        "bob filtered it, dave blocked alice"
    );
    login("carol");
    send(&bed.room, "<p>from carol</p>", &[]);
    run(bed.now + 1_000);
    assert_eq!(
        devices(&bed.net.take()),
        set(["DAVE-UP"]),
        "dave blocked alice, not carol"
    );
}

/// Rule 4: a reaction is the sender's activity and never a push; taking it back takes its activity.
#[test]
fn a_reaction_never_pushes() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = send(&bed.room, "<p>Hello</p>", &[]);
    run(bed.now);
    bed.net.take();
    login("bob");
    let react = || {
        ok(
            &format!("{CHAT}.react#Message"),
            &[
                ("chatRoomId", text(&bed.room)),
                ("eventSeq", Value::I64(seq)),
                ("key", text("👍")),
            ],
        )
    };
    react();
    assert!(rows(&bed.facade, JOB, &[]).is_empty(), "a reaction writes no push job");
    run(bed.now + 1_000);
    assert!(bed.net.take().is_empty());
    let activity = rows(&bed.facade, ACTIVITY, &[("activityKindEnumId", "CuaReaction")]);
    assert_eq!(activity.len(), 1);
    assert_eq!(field(&activity[0], "userId"), "alice");
    assert_eq!(field(&activity[0], "actorUserId"), "bob");
    react();
    assert!(rows(&bed.facade, ACTIVITY, &[("activityKindEnumId", "CuaReaction")]).is_empty());
}

/// Rule 3: a read on another device stops the pushes not yet sent and clears what was shown; the read marker is also
/// checked at the send.
#[test]
fn a_read_on_another_device_sends_no_push_and_a_clear() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = send(&bed.room, "<p>Hello</p>", &[]);
    run(bed.now);
    assert_eq!(devices(&bed.net.take()), set(["BOB-PHONE", "BOB-WEB", "DAVE-UP"]));
    login("bob");
    ok(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&bed.room))]);
    run(bed.now + 1_000);
    let clears = bed.net.take();
    assert_eq!(devices(&clears), set(["BOB-PHONE", "BOB-WEB"]));
    for post in &clears {
        let said = payload(post);
        assert_eq!(said["kind"], "clear");
        assert_eq!(said["chatRoomId"], bed.room.as_str());
        assert!(said["lastReadEventSeq"].as_i64().unwrap() >= seq);
        match &post.fcm {
            Some(body) => {
                assert_eq!(body["message"]["data"]["kind"], "clear");
                assert_eq!(body["message"]["android"]["priority"], "NORMAL");
            }
            None => assert_eq!(post.urgency, "normal"),
        }
    }

    // read before the push went: none to the reader
    login("alice");
    send(&bed.room, "<p>Again</p>", &[]);
    login("bob");
    ok(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&bed.room))]);
    run(bed.now + 2_000);
    assert_eq!(devices(&bed.net.take()), set(["DAVE-UP"]));

    // read after the push was queued, before it went (its first send was not taken): dropped at its next send
    bed.net.answer(
        "DAVE-UP",
        &[Sent::Retry {
            after_millis: Some(1_000),
            error: "503".to_string(),
        }],
    );
    login("alice");
    let third = send(&bed.room, "<p>Third</p>", &[]);
    run(bed.now + 3_000);
    bed.net.take();
    update(
        &bed.facade,
        "moqui.chat.ChatRoomMember",
        &[("chatRoomId", &bed.room), ("userId", "dave")],
        &[("lastReadEventSeq", Value::I64(third))],
    );
    let out = run(bed.now + 10_000);
    assert!(bed.net.take().is_empty());
    assert_eq!(number(&out, "droppedCount"), 1);
}

/// Rule 4: on do not disturb the pushes are held, and go as one summary per room and device when it ends.
#[test]
fn do_not_disturb_holds_the_pushes_and_sends_one_summary() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", text("bob")), ("statusEnumId", text("UpsDnd"))],
    );
    login("alice");
    for body in ["<p>one</p>", "<p>two</p>", "<p>three</p>"] {
        send(&bed.room, body, &[]);
    }
    run(bed.now);
    let posts = bed.net.take();
    assert_eq!(
        devices(&posts),
        set(["DAVE-UP"]),
        "the others' pushes merge per room too"
    );
    assert_eq!(payload(&posts[0])["count"], 3);
    for device in ["BOB-PHONE", "BOB-WEB"] {
        let held = outbox(&bed.facade, device);
        assert_eq!(held.len(), 1, "{device}");
        assert_eq!(field(&held[0], "statusEnumId"), "PosHeld");
        assert_eq!(count_of(&held[0], "messageCount"), 3);
    }
    run(bed.now + 60_000);
    assert!(bed.net.take().is_empty(), "still on do not disturb");
    update(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", "bob")],
        &[("statusEnumId", text("UpsOnline"))],
    );
    run(bed.now + 120_000);
    let posts = bed.net.take();
    assert_eq!(devices(&posts), set(["BOB-PHONE", "BOB-WEB"]));
    for post in &posts {
        let said = payload(post);
        assert_eq!(said["count"], 3);
        assert_eq!(said["title"], "3 new messages in Team");
        assert_eq!(said["snippet"], "three");
    }
}

/// Rule 2: the push is after the commit: a message that rolls back pushes nothing.
#[test]
fn a_rolled_back_message_pushes_nothing() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let tx = bed.facade.default_transactions().unwrap();
    let outcome: Result<(), moqui_lib::error::Error> =
        tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
            send(&bed.room, "<p>never</p>", &[]);
            Err(moqui_lib::error::Error::new("rolled back"))
        });
    assert!(outcome.is_err());
    assert!(rows(&bed.facade, JOB, &[]).is_empty());
    assert_eq!(number(&run(bed.now), "jobCount"), 0);
    assert!(bed.net.take().is_empty());
}

/// A pending push of a device and room takes the next message in, with a count; a push sent is not merged into.
#[test]
fn pending_pushes_merge_per_device_and_room() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    send(&bed.room, "<p>one</p>", &[]);
    send(&bed.room, "<p>two</p>", &[]);
    run(bed.now);
    let posts: Vec<Post> = bed
        .net
        .take()
        .into_iter()
        .filter(|post| post.device == "BOB-PHONE")
        .collect();
    assert_eq!(posts.len(), 1);
    let said = payload(&posts[0]);
    assert_eq!(said["count"], 2);
    assert_eq!(said["snippet"], "two");
    send(&bed.room, "<p>three</p>", &[]);
    run(bed.now + 1_000);
    let posts: Vec<Post> = bed
        .net
        .take()
        .into_iter()
        .filter(|post| post.device == "BOB-PHONE")
        .collect();
    assert_eq!(payload(&posts[0])["count"], 1);
    assert_eq!(outbox(&bed.facade, "BOB-PHONE").len(), 2);
}

/// FCM 429 and 5xx wait with backoff and jitter, or the Retry-After FCM gave, and the push fails after its tries; the
/// device keeps its counts.
#[test]
fn a_push_not_taken_is_sent_again_with_backoff() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.net.answer(
        "BOB-PHONE",
        &[
            Sent::Retry {
                after_millis: None,
                error: "503".to_string(),
            },
            Sent::Retry {
                after_millis: Some(120_000),
                error: "429".to_string(),
            },
        ],
    );
    login("alice");
    send(&bed.room, "<p>Hello</p>", &[]);
    assert_eq!(number(&run(bed.now), "retriedCount"), 1);
    bed.net.take();
    let push = outbox(&bed.facade, "BOB-PHONE").remove(0);
    let next = millis_of(&push, "nextAttemptDate").unwrap() - bed.now;
    assert!((8_000..=12_000).contains(&next), "{next}");
    assert_eq!(field(&push, "statusEnumId"), "PosPending");
    run(bed.now + 5_000);
    assert!(bed.net.take().is_empty(), "not due yet");
    let second = bed.now + 13_000;
    run(second);
    assert_eq!(devices(&bed.net.take()), set(["BOB-PHONE"]));
    let push = outbox(&bed.facade, "BOB-PHONE").remove(0);
    assert_eq!(
        millis_of(&push, "nextAttemptDate").unwrap(),
        second + 120_000,
        "FCM's Retry-After"
    );
    run(second + 120_000);
    assert_eq!(devices(&bed.net.take()), set(["BOB-PHONE"]));
    let push = outbox(&bed.facade, "BOB-PHONE").remove(0);
    assert_eq!(field(&push, "statusEnumId"), "PosSent");
    assert_eq!(count_of(&push, "attemptCount"), 3);
    let phone = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-PHONE")]).remove(0);
    assert_eq!(count_of(&phone, "sentCount"), 1);
    assert!(millis_of(&phone, "lastPushDate").is_some());

    // out of tries: failed, and counted on the device
    let tries: Vec<Sent> = (0..MAX_ATTEMPTS)
        .map(|_| Sent::Retry {
            after_millis: None,
            error: "503".to_string(),
        })
        .collect();
    bed.net.answer("BOB-PHONE", &tries);
    send(&bed.room, "<p>Again</p>", &[]);
    let mut at = second + 200_000;
    for _ in 0..MAX_ATTEMPTS {
        run(at);
        at += MAX_WAIT_MILLIS + 1_000;
    }
    let pushes = outbox(&bed.facade, "BOB-PHONE");
    let last = pushes.last().unwrap();
    assert_eq!(field(last, "statusEnumId"), "PosFailed");
    assert_eq!(field(last, "reasonCode"), "attempts");
    let phone = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-PHONE")]).remove(0);
    assert_eq!(count_of(&phone, "failedCount"), 1);
}

/// The backoff doubles from ten seconds with a jitter of a fifth either way, at most an hour.
#[test]
fn the_backoff_doubles_with_jitter_to_an_hour() {
    for attempt in 1..=6 {
        let base = FIRST_WAIT_MILLIS << (attempt - 1);
        let wait = backoff("P1", attempt, None);
        assert!(wait >= base * 8 / 10 && wait <= base * 12 / 10, "{attempt}: {wait}");
    }
    assert!(backoff("P1", 30, None) <= MAX_WAIT_MILLIS);
    assert_eq!(backoff("P1", 1, Some(7_200_000)), MAX_WAIT_MILLIS);
    assert_ne!(
        backoff("P1", 3, None),
        backoff("P2", 3, None),
        "the jitter spreads pushes"
    );
}

/// FCM gone deletes the registration; Web Push and UnifiedPush gone disable theirs. Nothing more goes to them.
#[test]
fn a_gone_device_is_deleted_or_disabled() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.net.answer("BOB-PHONE", &[Sent::Gone]);
    bed.net.answer("BOB-WEB", &[Sent::Gone]);
    bed.net.answer("DAVE-UP", &[Sent::Gone]);
    login("alice");
    send(&bed.room, "<p>Hello</p>", &[]);
    run(bed.now);
    bed.net.take();
    assert!(rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-PHONE")]).is_empty());
    for device in ["BOB-WEB", "DAVE-UP"] {
        let row = rows(&bed.facade, DEVICE, &[("subscriptionId", device)]).remove(0);
        assert!(millis_of(&row, "disabledDate").is_some(), "{device}");
    }
    send(&bed.room, "<p>Again</p>", &[]);
    run(bed.now + 1_000);
    assert!(bed.net.take().is_empty());
}

/// SEC-COM-81: a Web Push endpoint that takes most of the request's timeout to answer counts as failing though it took
/// the push, and is disabled after [`super::super::web_push::MAX_FAILURES`] such answers, as one that never answers: a
/// host a browser names does not hold the one push worker for every user. An FCM phone is not held to it.
#[test]
fn a_web_endpoint_that_answers_late_is_disabled_as_failing() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    *bed.net.slow.lock().unwrap() = Some((
        std::time::Duration::from_millis(30),
        std::time::Duration::from_millis(10),
    ));
    login("alice");
    for round in 0..super::super::web_push::MAX_FAILURES {
        send(&bed.room, &format!("<p>Message {round}</p>"), &[]);
        run(bed.now + round * 1_000);
    }
    assert!(devices(&bed.net.take()).contains("BOB-WEB"));
    let web = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-WEB")]).remove(0);
    assert!(millis_of(&web, "disabledDate").is_some());
    assert_eq!(count_of(&web, "sentCount"), 0);
    let phone = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-PHONE")]).remove(0);
    assert!(millis_of(&phone, "disabledDate").is_none());
}

/// SEC-COM-81: a user keeps at most `push/web@max-subscriptions` (10) browser subscriptions active, each an endpoint
/// of any https host; a new one past it disables the longest unregistered. And a test push is one a user in ten
/// seconds, whichever of the user's endpoints asks.
#[test]
fn a_user_keeps_ten_browser_subscriptions_and_one_test_push_at_a_time() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("carol");
    let endpoint = |n: usize| format!("https://push.attacker.example/{n}");
    for n in 0..12 {
        ok(
            "org.moqui.ui.NotificationServices.store#WebPushSubscription",
            &[
                ("endpoint", text(&endpoint(n))),
                ("p256dhKey", text(UA_PUBLIC)),
                ("authSecret", text(AUTH)),
            ],
        );
    }
    let web: Vec<EntityValue> = rows(&bed.facade, DEVICE, &[("userId", "carol")])
        .into_iter()
        .filter(|row| field(row, "pushProviderEnumId") == "PushWeb")
        .collect();
    let active: Vec<String> = web
        .iter()
        .filter(|row| millis_of(row, "disabledDate").is_none())
        .map(|row| field(row, "endpoint"))
        .collect();
    assert_eq!((web.len(), active.len()), (12, 10), "{active:?}");
    assert!(active.contains(&endpoint(11)), "{active:?}");
    let test = |n: usize| {
        let out = ok(
            "org.moqui.ui.NotificationServices.send#TestPush",
            &[("endpoint", text(&endpoint(n)))],
        );
        field_of(&out, "reasonCode")
    };
    assert_eq!(test(11), "");
    assert_eq!(test(10), "tooSoon");
    // an endpoint stored again keeps its failures: one disabled for them stays disabled, one short of them is still
    // counting
    let stored = |n: usize| {
        let row = rows(&bed.facade, DEVICE, &[("userId", "carol")])
            .into_iter()
            .find(|row| field(row, "endpoint") == endpoint(n))
            .unwrap();
        (
            count_of(&row, "failureCount"),
            millis_of(&row, "disabledDate").is_some(),
        )
    };
    let store = |n: usize| {
        ok(
            "org.moqui.ui.NotificationServices.store#WebPushSubscription",
            &[
                ("endpoint", text(&endpoint(n))),
                ("p256dhKey", text(UA_PUBLIC)),
                ("authSecret", text(AUTH)),
            ],
        );
    };
    let failing = |n: usize, failures: i64, disabled: bool| {
        let hash = super::super::web_push::endpoint_hash(&endpoint(n));
        update(
            &bed.facade,
            DEVICE,
            &[("endpointHash", hash.as_str())],
            &[
                ("failureCount", Value::I64(failures)),
                ("disabledDate", if disabled { stamp(bed.now) } else { Value::Null }),
            ],
        );
    };
    failing(11, super::super::web_push::MAX_FAILURES, true);
    failing(10, 3, false);
    store(11);
    store(10);
    assert_eq!(stored(11), (super::super::web_push::MAX_FAILURES, true));
    assert_eq!(stored(10), (3, false));
}

/// SEC-COM-81: a delivery round gives each user's pushes at most a budget of its time; that user's other pushes wait
/// for the next round, so the endpoints one user names do not hold the others'.
#[test]
fn each_user_has_a_budget_of_a_delivery_round() {
    let mut spent = UserBudget::new(std::time::Duration::from_millis(100));
    assert!(!spent.exhausted("mallory"));
    spent.add("mallory", std::time::Duration::from_millis(60));
    assert!(!spent.exhausted("mallory"));
    spent.add("mallory", std::time::Duration::from_millis(60));
    assert!(spent.exhausted("mallory") && !spent.exhausted("ann"));
    assert!(USER_ROUND_BUDGET > SLOW_SEND);
}

/// SEC-COM-81: the rest of the pushes of a user whose budget of the round is spent are put off to the next round, and
/// the round reads the pushes due again, so a user with many old pushes does not hold everyone else's.
#[test]
fn a_user_whose_budget_is_spent_waits_and_the_others_are_pushed() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    send(&bed.room, "<p>Hello all</p>", &[]);
    let mut report = RoundReport::default();
    fan_out(&bed.facade, bed.net.as_ref(), "node-1", bed.now, &mut report).unwrap();
    bed.net.take();
    // bob's two pushes (his phone and browser) are the oldest, and each user's budget ends at a push
    update(
        &bed.facade,
        OUTBOX,
        &[("userId", "bob")],
        &[("createdDate", stamp(bed.now - 60_000))],
    );
    *bed.net.budget.lock().unwrap() = Some(std::time::Duration::ZERO);
    let pushed = |at: i64| {
        let mut report = RoundReport::default();
        deliver_due(&bed.facade, bed.net.as_ref(), "node-1", at, 2, &mut report).unwrap();
        bed.net.take().into_iter().map(|post| post.device).collect::<Vec<_>>()
    };
    let first = pushed(bed.now);
    assert_eq!(
        first.iter().filter(|device| device.starts_with("BOB-")).count(),
        1,
        "{first:?}"
    );
    assert!(
        first.iter().any(|device| device == "DAVE-UP"),
        "dave is pushed in the same round: {first:?}"
    );
    let put_off = rows(&bed.facade, OUTBOX, &[("userId", "bob"), ("statusEnumId", PENDING)]);
    assert_eq!(put_off.len(), 1);
    assert!(put_off
        .iter()
        .all(|row| millis_of(row, "nextAttemptDate").is_some_and(|at| at > bed.now)));
    // the next round sends it
    assert_eq!(pushed(bed.now).len(), 0, "not again in this round");
    let next = pushed(bed.now + 1);
    assert!(next.len() == 1 && next[0].starts_with("BOB-"), "{next:?}");
}

/// A message lives 4 weeks with the room's collapse key at high priority; one not taken by then fails.
#[test]
fn a_message_has_its_ttl_collapse_key_and_priority() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.net.answer(
        "ERIN-PHONE",
        &[Sent::Retry {
            after_millis: None,
            error: "503".to_string(),
        }],
    );
    login("alice");
    send(&bed.room, &format!("<p>{}</p>", mention("erin")), &[]);
    run(bed.now);
    let posts = bed.net.take();
    // a keyed hash of the device and the room: no room id, and another per device and for a clear
    let key = |device: &str| collapse_key(device, &bed.room, false);
    assert!(!key("BOB-PHONE").contains(&bed.room) && key("BOB-PHONE").len() <= 32);
    assert_ne!(
        key("BOB-PHONE"),
        key("BOB-WEB"),
        "two devices' pushes of a room are not linked"
    );
    assert_ne!(
        key("BOB-PHONE"),
        collapse_key("BOB-PHONE", &bed.room, true),
        "a clear never replaces a message"
    );
    for post in &posts {
        match &post.fcm {
            Some(body) => {
                let android = &body["message"]["android"];
                assert_eq!(android["priority"], "HIGH");
                assert_eq!(android["ttl"], format!("{MESSAGE_TTL_SECONDS}s"));
                assert_eq!(android["collapse_key"], key(&post.device).as_str());
            }
            None => {
                assert_eq!(post.ttl, MESSAGE_TTL_SECONDS);
                assert_eq!(post.urgency, "high");
                assert_eq!(post.topic, key(&post.device));
            }
        }
    }
    // erin's push, not taken, is out of time 4 weeks on (and, done, goes with the week's cleaning)
    let out = run(bed.now + 29 * DAY);
    assert!(bed.net.take().is_empty());
    assert_eq!(number(&out, "failedCount"), 1);
    let erin = rows(&bed.facade, DEVICE, &[("subscriptionId", "ERIN-PHONE")]).remove(0);
    assert_eq!(count_of(&erin, "failedCount"), 1);
    assert!(outbox(&bed.facade, "ERIN-PHONE").is_empty());
}

/// Rule 5: the payload is encrypted to each device: the push service sees no text; a plain room's has the snippet
/// unless the user turned previews off, an encrypted room's the room and sender only; an FCM device that gave no keys
/// gets the push's kind and id alone.
#[test]
fn the_payload_is_encrypted_to_each_device() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    send(
        &bed.room,
        &format!("<p>Hello secret plan {}</p>", mention("carol")),
        &[],
    );
    run(bed.now);
    let posts = bed.net.take();
    for post in &posts {
        let seen = match &post.fcm {
            Some(body) => body.to_string(),
            None => String::from_utf8_lossy(&post.body).to_string(),
        };
        assert!(
            !seen.contains("secret") && !seen.contains("Alice") && !seen.contains("Team"),
            "{seen}"
        );
    }
    let phone = posts.iter().find(|post| post.device == "BOB-PHONE").unwrap();
    let data = phone.fcm.as_ref().unwrap()["message"]["data"]
        .as_object()
        .unwrap()
        .clone();
    let keys: BTreeSet<&str> = data.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["enc", "kind", "pushId"]));
    let said = payload(phone);
    assert_eq!(said["kind"], "message");
    assert_eq!(said["senderName"], "Alice");
    assert_eq!(said["roomName"], "Team");
    assert_eq!(said["snippet"], "Hello secret plan @carol");
    assert_eq!(said["pushId"], "BOB-PHONE", "the registration, as a ring names it");
    assert_eq!(data["pushId"], "BOB-PHONE");
    let ack_id = said["ackId"].as_str().unwrap();
    assert_eq!(
        field(&outbox(&bed.facade, "BOB-PHONE").remove(0), "pushOutboxId"),
        ack_id
    );
    assert_eq!(said["title"], "Alice in Team");
    assert_eq!(said["mention"], false);
    let carol = posts.iter().find(|post| post.device == "CAROL-PHONE").unwrap();
    let data = carol.fcm.as_ref().unwrap()["message"]["data"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(
        data.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["kind", "pushId"])
    );
    let web = posts.iter().find(|post| post.device == "BOB-WEB").unwrap();
    assert_eq!(payload(web)["snippet"], "Hello secret plan @carol");

    // previews off: no snippet
    row(
        &bed.facade,
        "moqui.security.UserPreference",
        &[
            ("userId", text("bob")),
            ("preferenceKey", text("CHAT_NOTIFY_PREVIEW")),
            ("preferenceValue", text("N")),
        ],
    );
    send(&bed.room, "<p>Hidden</p>", &[]);
    run(bed.now + 1_000);
    let posts = bed.net.take();
    let phone = posts.iter().find(|post| post.device == "BOB-PHONE").unwrap();
    assert!(payload(phone)["snippet"].is_null());
    assert_eq!(payload(phone)["senderName"], "Alice");

    // an encrypted room: the room and the sender only
    let sealed = field_of(
        &ok(
            &format!("{CHAT}.create#Room"),
            &[
                ("roomTypeEnumId", text("CrtPrivate")),
                ("roomName", text("Sealed")),
                ("encrypted", Value::Bool(true)),
                ("memberUserIdList", Value::List(vec![text("dave")])),
            ],
        ),
        "chatRoomId",
    );
    let mut megolm = moqui_lib::object::ValueMap::default();
    for (key, value) in [
        ("algorithm", "m.megolm.v1.aes-sha2"),
        ("ciphertext", "AwgAEnAC"),
        ("session_id", "S1"),
    ] {
        megolm.insert(key.to_string(), text(value));
    }
    ok(
        &format!("{CHAT}.send#Message"),
        &[("chatRoomId", text(&sealed)), ("encrypted", Value::Map(megolm))],
    );
    run(bed.now + 2_000);
    let posts = bed.net.take();
    let said = payload(posts.iter().find(|post| post.device == "DAVE-UP").unwrap());
    assert_eq!(said["encrypted"], true);
    assert!(said["snippet"].is_null());
    assert_eq!(said["roomName"], "Sealed");
    assert_eq!(said["senderName"], "Alice");
}

/// A disappearing message (plans/Comms-Messaging.md §18.3 phase 16): its push has the room and sender only, as an
/// encrypted room's, and is not sent after the message's own time.
#[test]
fn a_disappearing_message_pushes_no_text_and_ends_with_it() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    ok(
        "org.moqui.chat.ChatExpiryServices.set#RoomTimer",
        &[("chatRoomId", text(&bed.room)), ("seconds", Value::I64(300))],
    );
    let seq = send(&bed.room, "<p>Gone soon</p>", &[]);
    let expires = rows(&bed.facade, "moqui.chat.ChatEvent", &[("chatRoomId", &bed.room)])
        .into_iter()
        .find(|row| count_of(row, "eventSeq") == seq)
        .and_then(|row| millis_of(&row, "expireDate"))
        .expect("the message's time");
    run(bed.now);
    let posts = bed.net.take();
    let said = payload(posts.iter().find(|post| post.device == "BOB-PHONE").unwrap());
    assert!(said["snippet"].is_null(), "{said}");
    assert_eq!(said["senderName"], "Alice");
    // a push that waits is not sent past the message's time
    let ends = rows(&bed.facade, OUTBOX, &[("subscriptionId", "BOB-PHONE")])
        .iter()
        .filter_map(|row| millis_of(row, "expireDate"))
        .max()
        .unwrap();
    assert!(ends <= expires, "{ends} past {expires}");
}

/// A text's one-time code (plans/Comms-Messaging.md §18.3 phase 21, §7.2 rule 10) is in its push for the
/// notification's Copy, only where the push carries the message's words: none with previews off.
#[test]
fn a_one_time_code_is_in_the_push_only_with_previews() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = send(&bed.room, "<p>Your code is 482913</p>", &[]);
    {
        let _authz = bed.ctx.disable_authz();
        let event = rows(&bed.facade, "moqui.chat.ChatEvent", &[("chatRoomId", &bed.room)])
            .into_iter()
            .find(|row| count_of(row, "eventSeq") == seq)
            .unwrap();
        let mut content: serde_json::Value = serde_json::from_str(&field(&event, "contentJson")).unwrap();
        content["otp"] = serde_json::json!({"code": "482913"});
        update(
            &bed.facade,
            "moqui.chat.ChatEvent",
            &[("chatRoomId", &bed.room), ("eventSeq", &seq.to_string())],
            &[("contentJson", text(&content.to_string()))],
        );
    }
    run(bed.now);
    let posts = bed.net.take();
    let said = payload(posts.iter().find(|post| post.device == "BOB-PHONE").unwrap());
    assert_eq!(said["otp"]["code"], "482913", "{said}");

    // with previews off, no code
    row(
        &bed.facade,
        "moqui.security.UserPreference",
        &[
            ("userId", text("bob")),
            ("preferenceKey", text("CHAT_NOTIFY_PREVIEW")),
            ("preferenceValue", text("N")),
        ],
    );
    login("alice");
    let again = send(&bed.room, "<p>Your code is 551177</p>", &[]);
    {
        let _authz = bed.ctx.disable_authz();
        update(
            &bed.facade,
            "moqui.chat.ChatEvent",
            &[("chatRoomId", &bed.room), ("eventSeq", &again.to_string())],
            &[(
                "contentJson",
                text(r#"{"body":"<p>Your code is 551177</p>","otp":{"code":"551177"}}"#),
            )],
        );
    }
    run(bed.now + 1000);
    let posts = bed.net.take();
    let said = payload(posts.iter().find(|post| post.device == "BOB-PHONE").unwrap());
    assert!(said.get("otp").is_none(), "{said}");

    // previews on again, and a code's message that disappears (its snippet is none): no code either
    update(
        &bed.facade,
        "moqui.security.UserPreference",
        &[("userId", "bob"), ("preferenceKey", "CHAT_NOTIFY_PREVIEW")],
        &[("preferenceValue", text("Y"))],
    );
    login("alice");
    let gone = send(&bed.room, "<p>Your code is 778899</p>", &[]);
    {
        let _authz = bed.ctx.disable_authz();
        update(
            &bed.facade,
            "moqui.chat.ChatEvent",
            &[("chatRoomId", &bed.room), ("eventSeq", &gone.to_string())],
            &[
                (
                    "contentJson",
                    text(r#"{"body":"<p>Your code is 778899</p>","otp":{"code":"778899"}}"#),
                ),
                ("expireDate", Value::I64(bed.now + 86_400_000)),
            ],
        );
    }
    run(bed.now + 2000);
    let posts = bed.net.take();
    let said = payload(posts.iter().find(|post| post.device == "BOB-PHONE").unwrap());
    assert!(said["snippet"].is_null(), "{said}");
    assert!(said.get("otp").is_none(), "no code where no snippet: {said}");
}

/// Rule 4: on do not disturb a call rings from a person on the user's allowed list, or as the same person's second
/// call within 3 minutes.
#[test]
fn a_call_rings_through_do_not_disturb_from_the_allowed_and_a_repeat() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", text("bob")), ("statusEnumId", text("UpsDnd"))],
    );
    let ring = |call_id: &str| -> BTreeSet<String> {
        let out = ok(
            &format!("{PUSH}.filter#RingUsers"),
            &[
                ("userIdList", Value::List(vec![text("bob"), text("carol")])),
                ("fromUserId", text("alice")),
                ("callSessionId", text(call_id)),
            ],
        );
        match out.get("ringUserIdList") {
            Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(ring("C1"), set(["carol"]));
    // a second call from the same person within 3 minutes
    let now = crate::user::now_millis();
    row(
        &bed.facade,
        "moqui.call.CallSession",
        &[
            ("callSessionId", text("C0")),
            ("startedByUserId", text("alice")),
            ("startedDate", Value::I64(now - 60_000)),
        ],
    );
    row(
        &bed.facade,
        "moqui.call.CallUserLog",
        &[
            ("callSessionId", text("C0")),
            ("userId", text("bob")),
            ("roleEnumId", text("CurCallee")),
            ("logDate", Value::I64(now - 60_000)),
        ],
    );
    assert_eq!(ring("C1"), set(["bob", "carol"]));
    // not once the 3 minutes are over
    update(
        &bed.facade,
        "moqui.call.CallUserLog",
        &[("callSessionId", "C0")],
        &[("logDate", Value::I64(now - 240_000))],
    );
    assert_eq!(ring("C1"), set(["carol"]));
    // a person on the allowed list
    row(
        &bed.facade,
        "moqui.security.UserPreference",
        &[
            ("userId", text("bob")),
            ("preferenceKey", text("CHAT_DND_ALLOWED")),
            ("preferenceValue", text("dave,alice")),
        ],
    );
    assert_eq!(ring("C1"), set(["bob", "carol"]));
}

/// A device's acknowledgement is counted on it, once a push, for the push's own user only.
#[test]
fn an_acknowledgement_is_counted_on_its_device() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    send(&bed.room, "<p>Hello</p>", &[]);
    run(bed.now);
    let posts = bed.net.take();
    let ack_id = payload(posts.iter().find(|post| post.device == "BOB-PHONE").unwrap())["ackId"]
        .as_str()
        .unwrap()
        .to_string();
    let ack = |id: &str| {
        field_of(
            &ok("org.moqui.ui.NotificationServices.ack#Push", &[("ackId", text(id))]),
            "acked",
        )
    };
    login("carol");
    assert_eq!(ack(&ack_id), "false", "another user's push");
    login("bob");
    assert_eq!(ack(&ack_id), "true");
    assert_eq!(ack(&ack_id), "true");
    let phone = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-PHONE")]).remove(0);
    assert_eq!(count_of(&phone, "ackedCount"), 1, "once a push");
    assert!(millis_of(&phone, "lastAckDate").is_some());
    assert!(millis_of(&outbox(&bed.facade, "BOB-PHONE").remove(0), "ackedDate").is_some());
    // a notification of send#WebPush names its registration
    assert_eq!(ack("BOB-WEB"), "true");
    let web = rows(&bed.facade, DEVICE, &[("subscriptionId", "BOB-WEB")]).remove(0);
    assert_eq!(count_of(&web, "ackedCount"), 1);
}

/// A redaction takes the message's activity with it.
#[test]
fn a_redaction_removes_the_activity() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = send(&bed.room, &format!("<p>{}</p>", mention("carol")), &[]);
    assert_eq!(rows(&bed.facade, ACTIVITY, &[("chatRoomId", &bed.room)]).len(), 1);
    ok(
        &format!("{CHAT}.redact#Message"),
        &[("chatRoomId", text(&bed.room)), ("eventSeq", Value::I64(seq))],
    );
    assert!(rows(&bed.facade, ACTIVITY, &[("chatRoomId", &bed.room)]).is_empty());
    // its push job finds the message gone: nothing is pushed
    run(bed.now);
    assert!(bed.net.take().is_empty());
}

fn retry_once(bed: &Bed, device: &str) {
    bed.net.answer(
        device,
        &[Sent::Retry {
            after_millis: Some(1_000),
            error: "503".to_string(),
        }],
    );
}

/// A push sends the row as its claim locked it: a message another node merged in after the row was listed goes with it.
#[test]
fn a_push_is_sent_as_its_claim_found_it() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    retry_once(&bed, "BOB-PHONE");
    login("alice");
    send(&bed.room, "<p>one</p>", &[]);
    run(bed.now);
    bed.net.take();
    let listed = outbox(&bed.facade, "BOB-PHONE").remove(0);
    // another node merges a message in after this one listed the row
    let mut merged: serde_json::Value = serde_json::from_str(&field(&listed, "payloadJson")).unwrap();
    merged["count"] = 2.into();
    merged["snippet"] = "two".into();
    update(
        &bed.facade,
        OUTBOX,
        &[("pushOutboxId", &field(&listed, "pushOutboxId"))],
        &[
            ("messageCount", Value::I64(2)),
            ("payloadJson", text(&merged.to_string())),
        ],
    );
    let net: Arc<dyn PushTransport> = bed.net.clone();
    deliver_one(&bed.facade, net.as_ref(), &listed, "node-test", bed.now + 5_000).unwrap();
    let posts = bed.net.take();
    assert_eq!(devices(&posts), set(["BOB-PHONE"]));
    let said = payload(&posts[0]);
    assert_eq!(said["count"], 2);
    assert_eq!(said["snippet"], "two");
}

/// A device that now belongs to another user gets none of the earlier user's pushes: dropped when it moves, and checked
/// at the send.
#[test]
fn a_moved_device_gets_nothing_of_the_earlier_user() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    retry_once(&bed, "BOB-PHONE");
    retry_once(&bed, "BOB-WEB");
    login("alice");
    send(&bed.room, "<p>for bob</p>", &[]);
    run(bed.now);
    bed.net.take();
    // the phone signs in as carol: checked at its send
    update(
        &bed.facade,
        DEVICE,
        &[("subscriptionId", "BOB-PHONE")],
        &[("userId", text("carol"))],
    );
    // the browser moves through its registration: its waiting pushes are dropped then
    update(
        &bed.facade,
        DEVICE,
        &[("subscriptionId", "BOB-WEB")],
        &[("userId", text("carol"))],
    );
    forget_device_pushes(&bed.facade, "BOB-WEB");
    assert_eq!(
        field(&outbox(&bed.facade, "BOB-WEB").remove(0), "reasonCode"),
        "deviceMoved"
    );
    let out = run(bed.now + 10_000);
    assert!(bed.net.take().is_empty());
    assert_eq!(number(&out, "droppedCount"), 1);
    assert_eq!(
        field(&outbox(&bed.facade, "BOB-PHONE").remove(0), "reasonCode"),
        "deviceMoved"
    );
}

/// A clear has a collapse key of its own, closes the room's notifications by its tag, and is not sent while a newer
/// push of the room is waiting or shown unread on the device.
#[test]
fn a_clear_never_hides_a_newer_message() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let first = send(&bed.room, "<p>one</p>", &[]);
    run(bed.now);
    bed.net.take();
    // the second waits at the phone's push service; the browser shows it
    retry_once(&bed, "BOB-PHONE");
    send(&bed.room, "<p>two</p>", &[]);
    run(bed.now + 1_000);
    bed.net.take();
    login("bob");
    ok(
        &format!("{CHAT}.mark#Read"),
        &[("chatRoomId", text(&bed.room)), ("eventSeq", Value::I64(first))],
    );
    run(bed.now + 10_000);
    let posts = bed.net.take();
    // the phone gets its second message and no clear, the browser no clear
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert_eq!(posts[0].device, "BOB-PHONE");
    assert_eq!(payload(&posts[0])["kind"], "message");
    // read all: each device's clear, under the clear's own key, with the room's tag
    ok(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&bed.room))]);
    run(bed.now + 20_000);
    let clears = bed.net.take();
    assert_eq!(devices(&clears), set(["BOB-PHONE", "BOB-WEB"]));
    for post in &clears {
        assert_eq!(payload(post)["tag"], format!("chat-{}", bed.room));
        let key = collapse_key(&post.device, &bed.room, true);
        match &post.fcm {
            Some(body) => assert_eq!(body["message"]["android"]["collapse_key"], key.as_str()),
            None => assert_eq!(post.topic, key),
        }
    }
}

/// An edit that mentions someone with a push waiting is not another message; a read of part of what waits lowers its
/// count.
#[test]
fn an_edit_and_a_partial_read_keep_the_count_right() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", text("bob")), ("statusEnumId", text("UpsDnd"))],
    );
    login("alice");
    let first = send(&bed.room, "<p>one</p>", &[]);
    run(bed.now);
    ok(
        &format!("{CHAT}.edit#Message"),
        &[
            ("chatRoomId", text(&bed.room)),
            ("eventSeq", Value::I64(first)),
            ("body", text(&format!("<p>one {}</p>", mention("bob")))),
        ],
    );
    run(bed.now + 1_000);
    let held = outbox(&bed.facade, "BOB-PHONE").remove(0);
    assert_eq!(count_of(&held, "messageCount"), 1, "an edit is not another message");
    assert_eq!(field(&held, "mention"), "Y");
    let second = send(&bed.room, "<p>two</p>", &[]);
    send(&bed.room, "<p>three</p>", &[]);
    run(bed.now + 2_000);
    assert_eq!(count_of(&outbox(&bed.facade, "BOB-PHONE").remove(0), "messageCount"), 3);
    login("bob");
    ok(
        &format!("{CHAT}.mark#Read"),
        &[("chatRoomId", text(&bed.room)), ("eventSeq", Value::I64(second))],
    );
    let held = outbox(&bed.facade, "BOB-PHONE").remove(0);
    assert_eq!(count_of(&held, "messageCount"), 1, "two of the three were read");
    let said: serde_json::Value = serde_json::from_str(&field(&held, "payloadJson")).unwrap();
    assert_eq!(said["count"], 1);
    assert_eq!(said["title"], "Alice in Team");
}

/// Rule 6: a missed call pushes to those who missed it, a voicemail to the room, as messages; a call's other events
/// push nothing.
#[test]
fn a_missed_call_and_a_voicemail_push_as_messages() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let event = |content: serde_json::Value| {
        let content = moqui_lib::json::from_json(&content);
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        ok(
            &format!("{CHAT}.append#Event"),
            &[
                ("chatRoomId", text(&bed.room)),
                ("eventTypeEnumId", text("CetCall")),
                ("senderUserId", text("alice")),
                ("content", content),
            ],
        );
    };
    event(serde_json::json!({"callSessionId": "C1", "action": "started"}));
    event(serde_json::json!({"callSessionId": "C1", "action": "ended", "missedUserIdList": ["bob", "frank"]}));
    run(bed.now);
    let posts = bed.net.take();
    assert_eq!(
        devices(&posts),
        set(["BOB-PHONE", "BOB-WEB"]),
        "frank's level is Nothing"
    );
    let said = payload(&posts[0]);
    assert_eq!(said["call"], "missed");
    assert!(said["snippet"].is_null());
    assert_eq!(said["title"], "Missed call in Team");
    event(serde_json::json!({"callSessionId": "C2", "action": "voicemail", "voicemailId": "V1"}));
    run(bed.now + 1_000);
    assert_eq!(
        devices(&bed.net.take()),
        set(["BOB-PHONE", "BOB-WEB", "CAROL-PHONE", "DAVE-UP", "ERIN-PHONE"])
    );
}

/// A job worked again (its lease ran out during a long fan-out) writes no push twice.
#[test]
fn a_job_worked_twice_queues_each_push_once() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    send(&bed.room, "<p>once</p>", &[]);
    let job = rows(&bed.facade, JOB, &[]).remove(0);
    let net: Arc<dyn PushTransport> = bed.net.clone();
    let first = fan_out_job(&bed.facade, net.as_ref(), &job, bed.now).unwrap();
    // the same job again, as a second node holding its copy would work it
    let again = fan_out_job(&bed.facade, net.as_ref(), &job, bed.now).unwrap();
    assert_eq!(first, 3);
    assert_eq!(again, 0);
    assert_eq!(count_of(&outbox(&bed.facade, "BOB-PHONE").remove(0), "messageCount"), 1);
}

/// Rule 4: an outside number's second call within 3 minutes rings through do not disturb too.
#[test]
fn an_outside_number_calling_again_rings_through() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", text("bob")), ("statusEnumId", text("UpsDnd"))],
    );
    let now = crate::user::now_millis();
    row(
        &bed.facade,
        "moqui.call.CallSession",
        &[("callSessionId", text("P0")), ("startedDate", Value::I64(now - 60_000))],
    );
    row(
        &bed.facade,
        "moqui.call.CallLeg",
        &[
            ("callLegId", text("L0")),
            ("callSessionId", text("P0")),
            ("legTypeEnumId", text("CltPstn")),
            ("phoneNumber", text("+15550100")),
        ],
    );
    row(
        &bed.facade,
        "moqui.call.CallUserLog",
        &[
            ("callSessionId", text("P0")),
            ("userId", text("bob")),
            ("roleEnumId", text("CurCallee")),
            ("logDate", Value::I64(now - 60_000)),
        ],
    );
    let ring = |number: &str| -> BTreeSet<String> {
        let out = ok(
            &format!("{PUSH}.filter#RingUsers"),
            &[
                ("userIdList", Value::List(vec![text("bob")])),
                ("fromNumber", text(number)),
                ("callSessionId", text("P1")),
            ],
        );
        match out.get("ringUserIdList") {
            Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(ring("+15550100"), set(["bob"]));
    assert!(ring("+15550199").is_empty());
}

/// §7.6: a test notification goes through the outbox, encrypted to the device, and is acknowledged by its id.
#[test]
fn a_test_push_goes_the_full_path_to_the_device() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("bob");
    let ids = queue_test(&bed.facade, "bob", &["BOB-PHONE".to_string()], bed.now).unwrap();
    run(bed.now);
    let posts = bed.net.take();
    assert_eq!(devices(&posts), set(["BOB-PHONE"]));
    let data = posts[0].fcm.as_ref().unwrap()["message"]["data"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(
        data.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["enc", "kind", "pushId"])
    );
    let said = payload(&posts[0]);
    assert_eq!(said["kind"], "test");
    assert_eq!(said["ackId"], ids[0].as_str());
    assert_eq!(said["sentDate"], bed.now);
    assert_eq!(
        field_of(
            &ok(
                "org.moqui.ui.NotificationServices.ack#Push",
                &[("ackId", text(&ids[0]))]
            ),
            "acked"
        ),
        "true"
    );
    // a request that is no device of the user's queues nothing
    let out = ok("org.moqui.ui.NotificationServices.send#TestPush", &[]);
    assert_eq!(field_of(&out, "reasonCode"), "noDevice");
    // review finding: one test a device in ten seconds
    let phone = ["BOB-PHONE".to_string()];
    assert!(test_too_soon(&bed.facade, &phone, bed.now + 5_000).unwrap());
    assert!(!test_too_soon(&bed.facade, &phone, bed.now + 11_000).unwrap());
    assert!(!test_too_soon(&bed.facade, &["OTHER".to_string()], bed.now + 5_000).unwrap());
}

/// A ring to a device with keys has its caller and call sealed: only its kind and registration in clear text.
#[test]
fn a_ring_to_a_keyed_device_is_sealed() {
    let notice = super::super::fcm::Notice {
        title: "Call from Ann".to_string(),
        link: "/qapps/comms/Calls?callSessionId=CS1".to_string(),
        topic: "IncomingCall".to_string(),
        tag: "call-CS1".to_string(),
        kind: super::super::fcm::Kind::Ring(super::super::fcm::Ring {
            call_session_id: "CS1".to_string(),
            chat_room_id: "ROOM1".to_string(),
            caller: "Ann".to_string(),
            video: true,
            expires_at: 1_800_000_045_000,
        }),
        high: true,
        ttl: 45,
    };
    let body = super::super::fcm::device_message("t", "D1", Some((UA_PUBLIC, AUTH)), &notice, 1_800_000_000);
    let data = body["message"]["data"].as_object().unwrap();
    assert_eq!(
        data.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["enc", "kind", "pushId"])
    );
    assert_eq!(data["kind"], "ring");
    // the caller is in no clear-text field: every field but the ciphertext (a random base64 enc may spell "Ann")
    let mut clear = body.clone();
    clear["message"]["data"]["enc"] = serde_json::Value::Null;
    assert!(!clear.to_string().contains("Ann"), "{clear}");
    let opened = decrypt(&push_crypto::decode_b64(data["enc"].as_str().unwrap()).unwrap());
    assert_eq!(opened["caller"], "Ann");
    assert_eq!(opened["callSessionId"], "CS1");
    assert_eq!(opened["ringSeconds"], "45");
    assert_eq!(body["message"]["android"]["priority"], "HIGH");
}

/// A disappearing message merged into a push that waits for an ordinary one: the push lasts as long as the ordinary
/// message's, not as the shorter one's.
#[test]
fn a_merged_push_keeps_the_longer_time_to_live() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.net.answer(
        "BOB-PHONE",
        &[Sent::Retry {
            after_millis: None,
            error: "503".to_string(),
        }],
    );
    login("alice");
    send(&bed.room, "<p>Kept a while</p>", &[]);
    run(bed.now);
    bed.net.take();
    ok(
        "org.moqui.chat.ChatExpiryServices.set#RoomTimer",
        &[("chatRoomId", text(&bed.room)), ("seconds", Value::I64(300))],
    );
    send(&bed.room, "<p>Gone soon</p>", &[]);
    run(bed.now + 1_000);
    let waiting = outbox(&bed.facade, "BOB-PHONE");
    let ends = waiting
        .iter()
        .filter_map(|row| millis_of(row, "expireDate"))
        .max()
        .unwrap();
    assert!(ends > bed.now + 27 * DAY, "the ordinary message's 4 weeks: {ends}");
}

/// Rule 9 (plans/Comms-Messaging.md phase 15): a reminder pushes to each device of its user though they read past the
/// message, as a notification with its own tag and collapse key, and a read's clear leaves it; during do not disturb it
/// is held and goes when it ends; with previews off it says only that a message is due.
#[test]
fn a_reminder_pushes_though_read_and_is_held_by_do_not_disturb() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = send(&bed.room, "<p>Lunch at one?</p>", &[]);
    run(bed.now);
    bed.net.take();
    login("bob");
    ok(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&bed.room))]);
    run(bed.now + 1_000);
    bed.net.take();
    let remind = |id: &str, at: i64| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let reminder = Reminder {
            user: "bob",
            reminder_id: id,
            room_id: &bed.room,
            event_seq: seq,
            title: "Reminder: Alice in Team",
            hidden_body: "A message you asked to be reminded of",
        };
        queue_reminder(&bed.facade, &reminder, at).unwrap()
    };
    assert_eq!(remind("R1", bed.now + 2_000), 2);
    run(bed.now + 3_000);
    let posts = bed.net.take();
    assert_eq!(devices(&posts), set(["BOB-PHONE", "BOB-WEB"]), "sent though read");
    let collapse = reminder_collapse_room(&bed.room, seq);
    for post in &posts {
        let said = payload(post);
        assert_eq!(said["kind"], "notification");
        assert_eq!(said["topic"], "ChatReminder");
        assert_eq!(said["title"], "Reminder: Alice in Team");
        assert_eq!(said["body"], "Lunch at one?");
        assert_eq!(said["tag"], "chat-reminder-R1");
        assert!(said["link"].as_str().unwrap().ends_with(&format!("&eventSeq={seq}")));
        match &post.fcm {
            Some(body) => {
                assert_eq!(body["message"]["android"]["priority"], "HIGH");
                let key = body["message"]["android"]["collapse_key"].as_str().unwrap();
                assert_eq!(key, collapse_key("BOB-PHONE", &collapse, false));
                assert_ne!(key, collapse_key("BOB-PHONE", &bed.room, false), "not the room's");
            }
            None => assert_eq!(post.topic, collapse_key("BOB-WEB", &collapse, false)),
        }
    }
    // a read of the room since: its clear leaves the reminder as sent
    login("alice");
    send(&bed.room, "<p>Or two</p>", &[]);
    login("bob");
    ok(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&bed.room))]);
    run(bed.now + 4_000);
    bed.net.take();
    let reminders = rows(&bed.facade, OUTBOX, &[("userId", "bob"), ("kindEnumId", KIND_REMINDER)]);
    assert_eq!(reminders.len(), 2);
    assert!(reminders.iter().all(|row| field(row, "statusEnumId") == SENT));

    // do not disturb and previews off: held, then sent, with no text
    row(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", text("bob")), ("statusEnumId", text("UpsDnd"))],
    );
    row(
        &bed.facade,
        "moqui.security.UserPreference",
        &[
            ("userId", text("bob")),
            ("preferenceKey", text("CHAT_NOTIFY_PREVIEW")),
            ("preferenceValue", text("N")),
        ],
    );
    assert_eq!(remind("R2", bed.now + 5_000), 2);
    run(bed.now + 6_000);
    assert!(bed.net.take().is_empty(), "held during do not disturb");
    update(
        &bed.facade,
        "moqui.security.UserStatus",
        &[("userId", "bob")],
        &[("statusEnumId", text("UpsOnline"))],
    );
    run(bed.now + 7_000);
    let posts = bed.net.take();
    assert_eq!(devices(&posts), set(["BOB-PHONE", "BOB-WEB"]));
    for post in &posts {
        let said = payload(post);
        assert_eq!(said["tag"], "chat-reminder-R2");
        assert_eq!(said["body"], "A message you asked to be reminded of");
    }
}
