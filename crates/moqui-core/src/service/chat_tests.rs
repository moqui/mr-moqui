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

//! `org.moqui.impl.ChatServices`: who reaches a room (members, a public room's readers, a private room's strangers, a
//! record room through a stub of the record visibility hook), direct rooms one per set of users, the timeline (pages,
//! edits, redactions, reactions, threads, replies, pins), unread and mention counts by notification level, the pushes
//! (to members only, after the commit), presence (status, heartbeat, auto-away, clear-at, do not disturb), the search
//! document and the search by membership, files, the agent entry point, and the later parts: a send tried again, one
//! event by its eventSeq, the files and threads of a room, forwards, polls, link previews (through a stub of the page),
//! read receipts by preference and @room, retention per room, the mentions of a record room's non-members, and the
//! unread counts of one query each against the counts per room. The SQL on the real database and the
//! search index are `runtime/base-component/example/test/Chat.spec.js`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const SERVICES: &str = "org.moqui.impl.ChatServices";
const MINUTE: i64 = 60_000;

/// The stubs the tests point the hooks at: the record visibility service (a record whose id starts with OPEN is
/// visible, and one whose id lists the user among ids joined by + is, to that user with authorization on), a service that pushes and then fails (so its transaction rolls back), the pages of link previews (a URL
/// with broken in it fails), and an agent that answers.
const STUBS: &str = r#"<services>
    <service verb="check" noun="Visible">
        <in-parameters><parameter name="recordEntityName"/><parameter name="recordId"/><parameter name="checkOnly" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="recordVisible" type="Boolean"/></out-parameters>
        <actions><script><![CDATA[ recordVisible = checkOnly === true && (String(recordId).startsWith('OPEN')
                || (String(recordId).split('+').includes(ec.user.userId) && !ec.artifactExecution.authzDisabled())) ]]></script></actions>
    </service>
    <service verb="push" noun="ThenFail">
        <in-parameters><parameter name="userId"/></in-parameters>
        <actions><script><![CDATA[
            ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({topic: 'chatUser:' + userId,
                userIdList: [userId], message: {kind: 'room', action: 'rolledBack'}}).call()
            ec.message.addError('failed after the push')
        ]]></script></actions>
    </service>
    <service verb="get" noun="LinkPage">
        <in-parameters><parameter name="url"/></in-parameters>
        <out-parameters><parameter name="status"/><parameter name="contentType"/><parameter name="body" allow-html="any"/></out-parameters>
        <actions><script><![CDATA[
            if (url.includes('broken')) { status = '500'; contentType = 'text/html'; body = '' }
            else { status = '200'; contentType = 'text/html; charset=utf-8'
                body = '<html><head><title>Page ' + url + '</title><meta property="og:description" content="About it"></head><body></body></html>' }
        ]]></script></actions>
    </service>
    <service verb="receive" noun="AgentMessage">
        <in-parameters><parameter name="chatRoomId"/><parameter name="eventSeq" type="Integer"/><parameter name="agentUserId"/>
            <parameter name="senderUserId"/><parameter name="body" allow-html="any"/><parameter name="bodyText" allow-html="any"/>
            <parameter name="threadRootEventSeq" type="Integer"/></in-parameters>
        <actions><script><![CDATA[
            ec.artifactExecution.disableAuthz()
            ec.service.sync().name('org.moqui.impl.ChatServices.send#Message').parameters({chatRoomId: chatRoomId,
                senderUserId: agentUserId, body: '<p>echo: ' + bodyText + '</p>', threadRootEventSeq: threadRootEventSeq}).call()
        ]]></script></actions>
    </service>
</services>"#;

fn setup(name: &str, pools: crate::service::ServicePools) -> (ExecutionContext, EntityFacade, PathBuf) {
    let ctx = ExecutionContextFactory::test_context_in("", pools, crate::context::framework_dir());
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let dir = std::env::temp_dir().join(format!("moqui-chat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    std::fs::write(dir.join("service/test/ChatStub.xml"), STUBS).unwrap();
    (ctx, facade, dir)
}

fn install(dir: &std::path::Path) {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths = vec![
        framework.join("impl/ChatServices.xml"),
        framework.join("impl/ShellServices.xml"),
        dir.join("service/test/ChatStub.xml"),
    ];
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|item| Value::string(*item)).collect())
}

fn int(value: i64) -> Value {
    Value::I64(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

/// Users with these ids, each named by the id with a capital letter.
fn users(facade: &EntityFacade, ids: &[&str]) {
    for id in ids {
        let mut name: Vec<char> = id.chars().collect();
        name[0] = name[0].to_ascii_uppercase();
        row(
            facade,
            "moqui.security.UserAccount",
            &[
                ("userId", text(id)),
                ("username", text(id)),
                ("userFullName", text(&name.into_iter().collect::<String>())),
                ("emailAddress", text(&format!("{id}@example.com"))),
            ],
        );
    }
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
    let full = if name.contains('.') {
        name.to_string()
    } else {
        format!("{SERVICES}.{name}")
    };
    crate::service::sync(&full, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The errors so far, cleared.
fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

/// `call`, which must leave no error.
fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

/// `call`, which must leave an error containing `expected`.
fn refused(name: &str, parameters: &[(&str, Value)], expected: &str) {
    let _ = call(name, parameters);
    let errors = errors();
    assert!(
        errors.contains(expected),
        "{name}: expected an error with {expected:?}, got {errors:?}"
    );
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn out_number(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
}

fn maps(value: Option<&Value>) -> Vec<IndexMap<String, Value>> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect(),
        other => panic!("not a list: {other:?}"),
    }
}

fn field(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn number(map: &IndexMap<String, Value>, key: &str) -> i64 {
    field(map, key).parse().unwrap_or(-1)
}

fn sub(map: &IndexMap<String, Value>, key: &str) -> IndexMap<String, Value> {
    match map.get(key) {
        Some(Value::Map(inner)) => inner.clone(),
        other => panic!("{key} is not a map: {other:?}"),
    }
}

fn out_map(out: &BTreeMap<String, Value>, key: &str) -> IndexMap<String, Value> {
    match out.get(key) {
        Some(Value::Map(inner)) => inner.clone(),
        other => panic!("{key} is not a map: {other:?}"),
    }
}

/// A room of the current user: type, name, members.
fn room(kind: &str, name: &str, members: &[&str]) -> String {
    let mut parameters = vec![("roomTypeEnumId", text(kind)), ("memberUserIdList", list(members))];
    if !name.is_empty() {
        parameters.push(("roomName", text(name)));
    }
    out_text(&ok("create#Room", &parameters), "chatRoomId")
}

fn send(room: &str, body: &str) -> i64 {
    out_number(
        &ok("send#Message", &[("chatRoomId", text(room)), ("body", text(body))]),
        "eventSeq",
    )
}

fn timeline(room: &str, more: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let mut parameters = vec![("chatRoomId", text(room))];
    parameters.extend(more.iter().cloned());
    ok("get#Timeline", &parameters)
}

fn events(out: &BTreeMap<String, Value>) -> Vec<IndexMap<String, Value>> {
    maps(out.get("eventList"))
}

fn seqs(out: &BTreeMap<String, Value>) -> Vec<i64> {
    events(out).iter().map(|event| number(event, "eventSeq")).collect()
}

fn texts(out: &BTreeMap<String, Value>) -> Vec<String> {
    events(out)
        .iter()
        .filter(|event| field(event, "type") == "message")
        .map(|event| field(event, "bodyText"))
        .collect()
}

fn counts(user_id: &str) -> BTreeMap<String, Value> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    ok("get#UnreadCounts", &[("userId", text(user_id))])
}

fn room_ids(out: &BTreeMap<String, Value>) -> Vec<String> {
    maps(out.get("roomList"))
        .iter()
        .map(|room| field(room, "chatRoomId"))
        .collect()
}

fn set_time(ctx: &ExecutionContext, millis: i64) {
    ctx.set_effective_time(Some(millis));
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A push: its topic, the users it was routed to, and its message.
type Pushed = (String, Vec<String>, serde_json::Value);

/// The pushes of the notification bus: (topic, routed user ids, message) of each NotificationMessage, kept while the
/// subscription lives. Tests run at once in one process, so each test keeps only the pushes to its own users.
struct Tape {
    _subscription: crate::notify::Subscription,
    seen: Arc<Mutex<Vec<Pushed>>>,
}

impl Tape {
    fn start(users: &[&str]) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let users: Vec<String> = users.iter().map(|id| (*id).to_string()).collect();
        let subscription = crate::notify::subscribe(
            crate::notify::NOTIFICATION_MESSAGE,
            Arc::new(move |message: &crate::notify::TopicMessage| {
                let topic = message.body["topic"].as_str().unwrap_or_default().to_string();
                let routed: Vec<String> = message.body[crate::notify::ROUTE_USER_IDS]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if routed.iter().any(|id| users.contains(id)) {
                    kept.lock()
                        .unwrap()
                        .push((topic, routed, message.body["message"].clone()));
                }
            }),
        )
        .unwrap();
        Self {
            _subscription: subscription,
            seen,
        }
    }

    /// The pushes on topics starting with `prefix`, taken off the tape.
    fn take(&self, prefix: &str) -> Vec<Pushed> {
        let mut seen = self.seen.lock().unwrap();
        let (taken, kept): (Vec<_>, Vec<_>) = seen.drain(..).partition(|(topic, _, _)| topic.starts_with(prefix));
        *seen = kept;
        taken
    }

    fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }
}

/// Tests that store chat files set the property chat_attachment_root, which the whole process shares, to a folder of
/// their own: one at a time. The property is cleared before the lock goes.
fn attachment_root(dir: &std::path::Path) -> (std::sync::MutexGuard<'static, ()>, String) {
    static ROOT: Mutex<()> = Mutex::new(());
    let guard = ROOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::create_dir_all(dir.join("chat")).unwrap();
    let root = format!("file:{}", dir.join("chat").display());
    moqui_lib::env::set_property("chat_attachment_root", &root);
    (guard, root)
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// Who reaches a room: members, readers of a public room who may join it, strangers to a private room, and a record
/// room through the record visibility hook (a stub that sees the records whose id starts with OPEN).
#[test]
fn room_access_for_members_public_readers_strangers_and_record_rooms() {
    let (ctx, facade, dir) = setup("access", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["acowner", "acmember", "acstranger"]);
    login("acowner");
    let private = room("private", "Ops", &["acmember"]);
    let public = room("public", "General", &[]);
    refused("create#Room", &[("roomTypeEnumId", text("private"))], "needs a name");
    refused(
        "create#Room",
        &[
            ("roomTypeEnumId", text("private")),
            ("roomName", text("X")),
            ("memberUserIdList", list(&["nobody"])),
        ],
        "User nobody not found",
    );
    let first = send(&private, "<p>hello team</p>");
    let access = ok("get#RoomAccess", &[("chatRoomId", text(&private))]);
    assert_eq!(out_number(&access, "level"), 4);
    assert_eq!(access.get("canModerate"), Some(&Value::Bool(true)));

    // a member posts; a member who is not a moderator gives no roles and deletes no one else's message
    login("acmember");
    let access = ok("get#RoomAccess", &[("chatRoomId", text(&private))]);
    assert_eq!(out_number(&access, "level"), 2);
    send(&private, "<p>hi</p>");
    refused(
        "set#MemberRole",
        &[
            ("chatRoomId", text(&private)),
            ("userId", text("acmember")),
            ("roleEnumId", text("moderator")),
        ],
        "Only a moderator or owner",
    );
    refused(
        "redact#Message",
        &[("chatRoomId", text(&private)), ("eventSeq", int(first))],
        "only your own",
    );

    // a stranger: the private room is not there; the public one is read only until joined
    login("acstranger");
    let none = ok("get#RoomAccess", &[("chatRoomId", text(&private))]);
    assert_eq!(out_number(&none, "level"), 0);
    assert_eq!(none.get("room"), Some(&Value::Null));
    refused("get#Timeline", &[("chatRoomId", text(&private))], "not found");
    refused(
        "send#Message",
        &[("chatRoomId", text(&private)), ("body", text("<p>x</p>"))],
        "not found",
    );
    refused("signal#Typing", &[("chatRoomId", text(&private))], "not found");
    refused("add#Member", &[("chatRoomId", text(&private))], "not found");
    refused(
        "get#RoomAccess",
        &[("chatRoomId", text(&private)), ("userId", text("acowner"))],
        "not yours to see",
    );
    assert!(room_ids(&ok("find#Rooms", &[])).is_empty());
    let joinable = ok("find#Rooms", &[("includeJoinable", Value::Bool(true))]);
    assert_eq!(room_ids(&joinable), std::slice::from_ref(&public));
    assert_eq!(field(&maps(joinable.get("roomList"))[0], "isMember"), "false");
    let read = timeline(&public, &[]);
    assert_eq!(out_map(&read, "access").get("canJoin"), Some(&Value::Bool(true)));
    refused(
        "send#Message",
        &[("chatRoomId", text(&public)), ("body", text("<p>x</p>"))],
        "Join the room",
    );
    ok("add#Member", &[("chatRoomId", text(&public))]);
    send(&public, "<p>joined</p>");
    assert_eq!(room_ids(&ok("find#Rooms", &[])), std::slice::from_ref(&public));

    // the owner makes the member a moderator, who then deletes another's message
    login("acowner");
    ok(
        "set#MemberRole",
        &[
            ("chatRoomId", text(&private)),
            ("userId", text("acmember")),
            ("roleEnumId", text("moderator")),
        ],
    );
    refused(
        "set#MemberRole",
        &[
            ("chatRoomId", text(&private)),
            ("userId", text("acowner")),
            ("roleEnumId", text("member")),
        ],
        "at least one owner",
    );
    login("acmember");
    ok(
        "redact#Message",
        &[("chatRoomId", text(&private)), ("eventSeq", int(first))],
    );

    // record rooms: none is visible before the theme names the hook
    login("acowner");
    let hook = ok(
        "check#RecordVisible",
        &[("recordEntityName", text("test.Thing")), ("recordId", text("OPEN1"))],
    );
    assert_eq!(hook.get("recordVisible"), Some(&Value::Bool(false)));
    assert_eq!(hook.get("hookService"), Some(&Value::Null));
    refused(
        "create#Room",
        &[
            ("roomTypeEnumId", text("record")),
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("OPEN1")),
        ],
        "not found",
    );
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", int(990)),
            ("resourceTypeEnumId", text("STRT_RECORD_VISIBLE_SERVICE")),
            ("resourceValue", text("test.ChatStub.check#Visible")),
        ],
    );
    let hook = ok(
        "check#RecordVisible",
        &[("recordEntityName", text("test.Thing")), ("recordId", text("OPEN1"))],
    );
    assert_eq!(hook.get("recordVisible"), Some(&Value::Bool(true)));
    assert_eq!(out_text(&hook, "hookService"), "test.ChatStub.check#Visible");
    let record = out_text(
        &ok(
            "create#Room",
            &[
                ("roomTypeEnumId", text("record")),
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("OPEN1")),
            ],
        ),
        "chatRoomId",
    );
    refused(
        "create#Room",
        &[
            ("roomTypeEnumId", text("record")),
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("SECRET1")),
        ],
        "not found",
    );
    // a record room only for its members
    let members_only = out_text(
        &ok(
            "create#Room",
            &[
                ("roomTypeEnumId", text("record")),
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("OPEN2")),
                ("visibilityEnumId", text("members")),
            ],
        ),
        "chatRoomId",
    );
    send(&record, "<p>about the thing</p>");

    // a user who may see the record reads the room, and joins it by asking for the record's room
    login("acstranger");
    let access = ok("get#RoomAccess", &[("chatRoomId", text(&record))]);
    assert_eq!(out_number(&access, "level"), 1);
    assert_eq!(access.get("canJoin"), Some(&Value::Bool(true)));
    let bound = timeline(
        "",
        &[("recordEntityName", text("test.Thing")), ("recordId", text("OPEN1"))],
    );
    assert_eq!(texts(&bound), ["about the thing"]);
    let none = ok(
        "get#Timeline",
        &[("recordEntityName", text("test.Thing")), ("recordId", text("NONE"))],
    );
    assert_eq!(none.get("room"), Some(&Value::Null));
    let again = ok(
        "create#Room",
        &[
            ("roomTypeEnumId", text("record")),
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("OPEN1")),
        ],
    );
    assert_eq!(out_text(&again, "chatRoomId"), record);
    assert_eq!(again.get("created"), Some(&Value::Bool(false)));
    assert_eq!(
        out_number(&ok("get#RoomAccess", &[("chatRoomId", text(&record))]), "level"),
        2
    );
    let closed = ok("get#RoomAccess", &[("chatRoomId", text(&members_only))]);
    assert_eq!(out_number(&closed, "level"), 0);
    // a record's Chatter asks by the record: no room, and no error
    let unread_room = ok(
        "get#Timeline",
        &[("recordEntityName", text("test.Thing")), ("recordId", text("OPEN2"))],
    );
    assert_eq!(unread_room.get("room"), Some(&Value::Null));
    refused("get#Timeline", &[("chatRoomId", text(&members_only))], "not found");
    refused(
        "create#Room",
        &[
            ("roomTypeEnumId", text("record")),
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("OPEN2")),
        ],
        "for its members",
    );

    // archived: read only
    login("acowner");
    ok(
        "update#Room",
        &[("chatRoomId", text(&public)), ("archived", Value::Bool(true))],
    );
    refused(
        "send#Message",
        &[("chatRoomId", text(&public)), ("body", text("<p>x</p>"))],
        "archived",
    );
    login("acmember");
    refused(
        "update#Room",
        &[("chatRoomId", text(&private)), ("archived", Value::Bool(true))],
        "Only an owner",
    );
    ok(
        "update#Room",
        &[("chatRoomId", text(&private)), ("topic", text("Weekly ops"))],
    );
    let changed = timeline(&private, &[]);
    let last = events(&changed).pop().unwrap();
    assert_eq!(field(&last, "type"), "system");
    assert_eq!(field(&sub(&last, "content"), "action"), "topic");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A direct room is one per set of users, whoever starts it; leaving hides it until its next message.
#[test]
fn direct_rooms_are_reused_and_come_back_with_a_new_message() {
    let (ctx, facade, dir) = setup("direct", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["dra", "drb", "drc"]);
    login("dra");
    let created = ok(
        "create#Room",
        &[("roomTypeEnumId", text("direct")), ("memberUserIdList", list(&["drb"]))],
    );
    let pair = out_text(&created, "chatRoomId");
    assert_eq!(created.get("created"), Some(&Value::Bool(true)));
    let again = ok(
        "create#Room",
        &[
            ("roomTypeEnumId", text("direct")),
            ("memberUserIdList", list(&["drb", "dra"])),
        ],
    );
    assert_eq!(out_text(&again, "chatRoomId"), pair);
    assert_eq!(again.get("created"), Some(&Value::Bool(false)));
    let group = room("direct", "", &["drb", "drc"]);
    assert_ne!(group, pair);
    refused(
        "add#Member",
        &[("chatRoomId", text(&pair)), ("userId", text("drc"))],
        "fixed members",
    );
    refused(
        "update#Room",
        &[("chatRoomId", text(&pair)), ("topic", text("x"))],
        "no settings",
    );
    login("drb");
    let theirs = ok(
        "create#Room",
        &[("roomTypeEnumId", text("direct")), ("memberUserIdList", list(&["dra"]))],
    );
    assert_eq!(out_text(&theirs, "chatRoomId"), pair);

    // the list names a direct room by the other members, with the other's presence
    login("dra");
    let mine = ok("find#Rooms", &[("filter", text("direct"))]);
    let rooms = maps(mine.get("roomList"));
    assert_eq!(rooms.len(), 2);
    let listed = rooms.iter().find(|room| field(room, "chatRoomId") == pair).unwrap();
    assert_eq!(field(listed, "displayName"), "Drb");
    assert_eq!(field(listed, "otherUserId"), "drb");
    assert_eq!(field(listed, "presence"), "offline");
    let grouped = rooms.iter().find(|room| field(room, "chatRoomId") == group).unwrap();
    assert_eq!(field(grouped, "displayName"), "Drb, Drc");
    assert_eq!(field(grouped, "otherUserId"), "");

    // leaving hides it; a new message brings it back, unread
    ok("remove#Member", &[("chatRoomId", text(&pair))]);
    assert!(!room_ids(&ok("find#Rooms", &[])).contains(&pair));
    login("drb");
    send(&pair, "<p>still there?</p>");
    login("dra");
    let back = ok("find#Rooms", &[("filter", text("unread"))]);
    assert_eq!(room_ids(&back), std::slice::from_ref(&pair));
    let listed = &maps(back.get("roomList"))[0];
    assert_eq!(number(listed, "unreadCount"), 1);
    assert_eq!(field(&sub(listed, "lastMessage"), "snippet"), "still there?");
    assert_eq!(field(&sub(listed, "lastMessage"), "senderName"), "Drb");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The timeline: pages by eventSeq both ways and around one, edits applied, a redaction as "message removed",
/// reactions aggregated and toggled, threads apart from the main timeline with their counts, replies with a preview,
/// pins.
#[test]
fn the_timeline_pages_and_applies_edits_redactions_reactions_threads_and_pins() {
    let (ctx, facade, dir) = setup("timeline", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["tla", "tlb"]);
    login("tla");
    let chat = room("private", "Timeline", &["tlb"]);
    let seq: Vec<i64> = (1..=7).map(|n| send(&chat, &format!("<p>m{n}</p>"))).collect();
    // event 1 is the room's creation
    assert_eq!(seq, [2, 3, 4, 5, 6, 7, 8]);

    let latest = timeline(&chat, &[("limit", int(3))]);
    assert_eq!(texts(&latest), ["m5", "m6", "m7"]);
    assert_eq!(latest.get("hasMoreBefore"), Some(&Value::Bool(true)));
    assert_eq!(latest.get("hasMoreAfter"), Some(&Value::Bool(false)));
    let before = timeline(&chat, &[("limit", int(3)), ("beforeSeq", int(6))]);
    assert_eq!(texts(&before), ["m2", "m3", "m4"]);
    assert_eq!(before.get("hasMoreAfter"), Some(&Value::Bool(true)));
    let oldest = timeline(&chat, &[("limit", int(3)), ("beforeSeq", int(3))]);
    assert_eq!(seqs(&oldest), [1, 2]);
    assert_eq!(field(&events(&oldest)[0], "type"), "system");
    assert_eq!(oldest.get("hasMoreBefore"), Some(&Value::Bool(false)));
    let after = timeline(&chat, &[("limit", int(2)), ("afterSeq", int(5))]);
    assert_eq!(texts(&after), ["m5", "m6"]);
    assert_eq!(after.get("hasMoreAfter"), Some(&Value::Bool(true)));
    assert_eq!(after.get("hasMoreBefore"), Some(&Value::Bool(true)));
    let around = timeline(&chat, &[("limit", int(4)), ("aroundSeq", int(5))]);
    assert_eq!(texts(&around), ["m2", "m3", "m4", "m5"]);

    // an edit of one's own message, in its place
    ok(
        "edit#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(8)),
            ("body", text("<p>m7, edited</p>")),
        ],
    );
    let edited = timeline(&chat, &[("limit", int(1))]);
    let last = &events(&edited)[0];
    assert_eq!(number(last, "eventSeq"), 8);
    assert_eq!(field(last, "body"), "<p>m7, edited</p>");
    assert_eq!(field(last, "edited"), "true");
    login("tlb");
    refused(
        "edit#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(8)),
            ("body", text("<p>mine now</p>")),
        ],
        "only your own",
    );

    // reactions: two people, two keys, and one taken back
    for (user, key) in [("tla", "👍"), ("tlb", "👍"), ("tlb", "🎉")] {
        login(user);
        let out = ok(
            "react#Message",
            &[("chatRoomId", text(&chat)), ("eventSeq", int(7)), ("key", text(key))],
        );
        assert_eq!(out.get("added"), Some(&Value::Bool(true)));
    }
    let back = ok(
        "react#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(7)), ("key", text("👍"))],
    );
    assert_eq!(back.get("added"), Some(&Value::Bool(false)));
    login("tla");
    let reacted = timeline(&chat, &[("limit", int(2))]);
    let message = &events(&reacted)[0];
    assert_eq!(field(message, "bodyText"), "m6");
    let reactions = maps(message.get("reactions"));
    assert_eq!(reactions.len(), 2);
    assert_eq!(field(&reactions[0], "key"), "👍");
    assert_eq!(number(&reactions[0], "count"), 1);
    assert_eq!(field(&reactions[0], "mine"), "true");
    assert_eq!(field(&reactions[1], "key"), "🎉");
    assert_eq!(field(&reactions[1], "mine"), "false");

    // a thread: replies stay out of the main timeline, the root counts them
    login("tlb");
    let replies: Vec<i64> = ["<p>r1</p>", "<p>r2</p>"]
        .iter()
        .map(|body| {
            out_number(
                &ok(
                    "send#Message",
                    &[
                        ("chatRoomId", text(&chat)),
                        ("body", text(body)),
                        ("threadRootEventSeq", int(6)),
                    ],
                ),
                "eventSeq",
            )
        })
        .collect();
    let main = timeline(&chat, &[("limit", int(50))]);
    assert!(!texts(&main).contains(&"r1".to_string()));
    let root = events(&main)
        .into_iter()
        .find(|event| number(event, "eventSeq") == 6)
        .unwrap();
    assert_eq!(number(&root, "threadReplyCount"), 2);
    let thread = timeline(&chat, &[("threadRootEventSeq", int(6))]);
    assert_eq!(texts(&thread), ["m5", "r1", "r2"]);
    // a reply to a reply goes to the root's thread
    let deeper = ok(
        "send#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("body", text("<p>r3</p>")),
            ("threadRootEventSeq", int(replies[1])),
        ],
    );
    assert_eq!(field(&out_map(&deeper, "event"), "threadRootEventSeq"), "6");

    // a reply that quotes a message
    let quoted = ok(
        "send#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("body", text("<p>about m1</p>")),
            ("replyToEventSeq", int(2)),
        ],
    );
    let page = timeline(&chat, &[("limit", int(1))]);
    let reply = &events(&page)[0];
    assert_eq!(number(reply, "eventSeq"), out_number(&quoted, "eventSeq"));
    let preview = sub(reply, "replyTo");
    assert_eq!(field(&preview, "snippet"), "m1");
    assert_eq!(field(&preview, "senderName"), "Tla");

    // a redaction: "message removed", no body, no reactions; only the sender (or a moderator)
    refused(
        "redact#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(7))],
        "only your own",
    );
    login("tla");
    ok("redact#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(7))]);
    let removed = events(&timeline(&chat, &[("limit", int(50))]))
        .into_iter()
        .find(|event| number(event, "eventSeq") == 7)
        .unwrap();
    assert_eq!(field(&removed, "redacted"), "true");
    assert_eq!(removed.get("body"), Some(&Value::Null));
    assert_eq!(removed.get("bodyText"), Some(&Value::Null));
    assert_eq!(field(&removed, "removedLabel"), "Message removed");
    assert!(maps(removed.get("reactions")).is_empty());
    refused(
        "react#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(7)), ("key", text("👍"))],
        "not found",
    );

    // pins
    ok("pin#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(3))]);
    ok("pin#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(4))]);
    ok(
        "pin#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(4)),
            ("pinned", Value::Bool(false)),
        ],
    );
    assert_eq!(texts(&timeline(&chat, &[("pinnedOnly", Value::Bool(true))])), ["m2"]);
    let pin_events = events(&timeline(&chat, &[("limit", int(3))]));
    assert_eq!(field(pin_events.last().unwrap(), "type"), "pin");

    // the edit window: a day by default
    set_time(&ctx, now() + 2 * 24 * 60 * MINUTE);
    refused(
        "edit#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(2)),
            ("body", text("<p>late</p>")),
        ],
        "too old",
    );
    ctx.set_effective_time(None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A membership starts at the member's own default level (CHAT_NOTIFY_DEFAULT) in create#Room and add#Member, but in a direct room;
/// a mention's notification leaves out the message text for a user whose CHAT_NOTIFY_PREVIEW is N.
#[test]
fn a_member_starts_at_their_default_level_and_a_mention_may_leave_out_the_text() {
    let (ctx, facade, dir) = setup("defaults", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["dla", "dlb", "dlc", "dld"]);
    for (user, key, value) in [
        ("dlb", "CHAT_NOTIFY_DEFAULT", "none"),
        ("dlc", "CHAT_NOTIFY_DEFAULT", "mentions"),
        ("dld", "CHAT_NOTIFY_DEFAULT", "bogus"),
        ("dlc", "CHAT_NOTIFY_PREVIEW", "N"),
    ] {
        row(
            &facade,
            "moqui.security.UserPreference",
            &[
                ("userId", text(user)),
                ("preferenceKey", text(key)),
                ("preferenceValue", text(value)),
            ],
        );
    }
    row(
        &facade,
        "moqui.security.user.NotificationTopic",
        &[
            ("topic", text("ChatMention")),
            ("persistOnSend", text("Y")),
            (
                "titleTemplate",
                text("${authorName} mentioned you in ${roomName}: ${summary}"),
            ),
        ],
    );
    let level = |room: &str, user: &str| {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.chat.ChatRoomMember")
            .unwrap()
            .eq("chatRoomId", text(room))
            .eq("userId", text(user))
            .one()
            .unwrap()
            .and_then(|row| row.get("notificationLevelEnumId").map(to_plain_string))
            .unwrap_or_default()
    };
    login("dla");
    let team = room("private", "Defaults", &["dlb", "dlc"]);
    assert_eq!(level(&team, "dla"), "CnlAll", "no preference: the room's");
    assert_eq!(level(&team, "dlb"), "CnlNone");
    assert_eq!(level(&team, "dlc"), "CnlMentions");
    ok("add#Member", &[("chatRoomId", text(&team)), ("userId", text("dld"))]);
    assert_eq!(level(&team, "dld"), "CnlAll", "a value that is no level: the room's");
    let open = room("public", "Lobby", &[]);
    login("dlb");
    ok("add#Member", &[("chatRoomId", text(&open))]);
    assert_eq!(level(&open, "dlb"), "CnlNone", "joining a public room");
    login("dla");
    let direct = room("direct", "", &["dlb"]);
    assert_eq!(level(&direct, "dlb"), "CnlAll", "a direct room's messages always count");

    // the mention of dlc (no preview) and of dld (preview): the text is in dld's notification only
    send(
        &team,
        r#"<p>secret plan <span data-mention-id="dlc">@Dlc</span> <span data-mention-id="dld">@Dld</span></p>"#,
    );
    let _authz = ctx.disable_authz();
    let title_of = |user: &str| {
        let users = facade
            .find("moqui.security.user.NotificationMessageUser")
            .unwrap()
            .eq("userId", text(user))
            .list()
            .unwrap();
        assert_eq!(users.len(), 1, "{user}");
        let id = to_plain_string(users.get(0).unwrap().get("notificationMessageId").unwrap());
        let message = facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("notificationMessageId", text(&id))
            .one()
            .unwrap()
            .unwrap();
        (
            to_plain_string(message.get("titleText").unwrap()),
            message.get("messageJson").map(to_plain_string).unwrap_or_default(),
        )
    };
    let (shown_title, shown_json) = title_of("dld");
    assert_eq!(shown_title, "Dla mentioned you in Defaults: secret plan @Dlc @Dld");
    assert!(shown_json.contains("secret plan"), "{shown_json}");
    let (hidden_title, hidden_json) = title_of("dlc");
    assert_eq!(hidden_title, "Dla mentioned you in Defaults");
    assert!(!hidden_json.contains("secret"), "{hidden_json}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unread and mention counts: messages from others after the read marker, mentions among them, by notification level;
/// a mention is notified on ChatMention unless the level is none; a redaction takes its mention back.
#[test]
fn unread_and_mention_counts_follow_the_read_marker_and_the_notification_level() {
    let (ctx, facade, dir) = setup("unread", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["una", "unb", "unc", "und"]);
    row(
        &facade,
        "moqui.security.user.NotificationTopic",
        &[
            ("topic", text("ChatMention")),
            ("persistOnSend", text("Y")),
            ("showAlert", text("Y")),
            (
                "titleTemplate",
                text("${authorName} mentioned you in ${roomName}: ${summary}"),
            ),
        ],
    );
    login("una");
    let chat = room("private", "Counts", &["unb", "unc"]);
    send(&chat, "<p>one</p>");
    let mention = send(
        &chat,
        r#"<p>two <span data-mention-id="unb">@Unb</span> and <span data-mention-id="und">@Und</span></p>"#,
    );
    send(&chat, r#"<p>three <span data-mention-id="unc">@Unc</span></p>"#);
    assert_eq!(out_number(&counts("una"), "total"), 0, "one's own messages are read");
    let b = counts("unb");
    assert_eq!(out_number(&b, "total"), 3);
    assert_eq!(out_number(&b, "unread"), 3);
    assert_eq!(out_number(&b, "mentions"), 1);
    assert_eq!(out_number(&b, "unreadRoomCount"), 1);
    assert_eq!(number(&sub(&out_map(&b, "roomCounts"), &chat), "mentions"), 1);
    // und is not a member of the private room: not mentioned there
    assert_eq!(out_number(&counts("und"), "mentions"), 0);
    let notified = |user: &str| {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.security.user.NotificationMessageUser")
            .unwrap()
            .eq("userId", text(user))
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(notified("unb"), 1);
    assert_eq!(notified("und"), 0);
    {
        let _authz = ctx.disable_authz();
        let messages = facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", text("ChatMention"))
            .list()
            .unwrap();
        let titles: Vec<String> = messages
            .iter()
            .map(|row| to_plain_string(row.get("titleText").unwrap()))
            .collect();
        assert!(
            titles.contains(&"Una mentioned you in Counts: two @Unb and @Und".to_string()),
            "{titles:?}"
        );
        assert!(messages
            .iter()
            .all(|row| to_plain_string(row.get("reasonEnumId").unwrap()) == "NrMention"));
    }

    // the read marker
    login("unb");
    let read = ok("mark#Read", &[("chatRoomId", text(&chat)), ("eventSeq", int(mention))]);
    assert_eq!(out_number(&read, "lastReadEventSeq"), mention);
    assert_eq!(out_number(&counts("unb"), "total"), 1);
    assert_eq!(out_number(&counts("unb"), "mentions"), 0);
    // a marker does not go back, except to mark unread from a message
    ok("mark#Read", &[("chatRoomId", text(&chat)), ("eventSeq", int(2))]);
    assert_eq!(out_number(&counts("unb"), "total"), 1);
    ok(
        "mark#Read",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(mention)),
            ("markUnread", Value::Bool(true)),
        ],
    );
    assert_eq!(out_number(&counts("unb"), "total"), 2);
    assert_eq!(
        room_ids(&ok("find#Rooms", &[("filter", text("mentions"))])),
        std::slice::from_ref(&chat)
    );
    ok("mark#AllRead", &[]);
    assert_eq!(out_number(&counts("unb"), "total"), 0);
    assert!(room_ids(&ok("find#Rooms", &[("filter", text("unread"))])).is_empty());

    // notification levels: mentions only, nothing, muted
    login("unc");
    assert_eq!(out_number(&counts("unc"), "total"), 3);
    ok(
        "set#Notification",
        &[
            ("chatRoomId", text(&chat)),
            ("notificationLevelEnumId", text("mentions")),
        ],
    );
    assert_eq!(out_number(&counts("unc"), "total"), 1);
    assert_eq!(out_number(&counts("unc"), "unread"), 0);
    ok(
        "set#Notification",
        &[("chatRoomId", text(&chat)), ("notificationLevelEnumId", text("none"))],
    );
    let none = counts("unc");
    assert_eq!(out_number(&none, "total"), 0);
    assert_eq!(out_number(&none, "mentions"), 0);
    assert_eq!(number(&sub(&out_map(&none, "roomCounts"), &chat), "unread"), 3);
    ok(
        "set#Notification",
        &[
            ("chatRoomId", text(&chat)),
            ("notificationLevelEnumId", text("all")),
            ("isMuted", Value::Bool(true)),
        ],
    );
    assert_eq!(out_number(&counts("unc"), "total"), 1);
    ok(
        "set#Notification",
        &[
            ("chatRoomId", text(&chat)),
            ("notificationLevelEnumId", text("none")),
            ("isMuted", Value::Bool(false)),
        ],
    );
    // at level none a mention is counted in the room but not notified
    login("una");
    send(&chat, r#"<p>four <span data-mention-id="unc">@Unc</span></p>"#);
    assert_eq!(notified("unc"), 1, "the first mention only");

    // a redaction takes the message and its mention out of the counts
    login("una");
    let before = counts("unb");
    ok(
        "redact#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(mention))],
    );
    login("unb");
    ok(
        "mark#Read",
        &[
            ("chatRoomId", text(&chat)),
            ("eventSeq", int(mention)),
            ("markUnread", Value::Bool(true)),
        ],
    );
    let after = counts("unb");
    assert_eq!(out_number(&after, "mentions"), 0);
    assert!(out_number(&after, "unread") >= out_number(&before, "unread"));
    assert_eq!(out_number(&after, "unread"), 2, "three and four, not the removed one");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pushes: each event goes on chatRoom:<id> to the room's members only, not persisted; typing and read markers go to
/// the other members; room list changes on chatUser:<id> to that user; presence to the user's room members; a push in
/// a transaction that rolls back is never sent.
#[test]
fn pushes_go_only_to_the_room_members_and_only_after_the_commit() {
    let (ctx, facade, dir) = setup("pushes", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["psa", "psb", "psc"]);
    let tape = Tape::start(&["psa", "psb", "psc"]);
    login("psa");
    let chat = room("private", "Pushes", &["psb"]);
    let joined = tape.take("chatUser:");
    assert_eq!(
        sorted(joined.iter().map(|(topic, _, _)| topic.clone()).collect()),
        ["chatUser:psa", "chatUser:psb"]
    );
    assert!(joined.iter().all(|(topic, routed, message)| routed.len() == 1
        && topic.ends_with(&routed[0])
        && message["kind"] == "room"
        && message["action"] == "joined"));
    tape.clear();

    let seq = send(&chat, "<p>for members</p>");
    let pushed = tape.take("chatRoom:");
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let (topic, routed, message) = &pushed[0];
    assert_eq!(topic, &format!("chatRoom:{chat}"));
    assert_eq!(sorted(routed.clone()), ["psa", "psb"]);
    assert_eq!(message["kind"], "event");
    assert_eq!(message["eventSeq"], seq);
    assert_eq!(message["event"]["body"], "<p>for members</p>");
    assert_eq!(message["event"]["senderName"], "Psa");
    {
        let _authz = ctx.disable_authz();
        let stored = facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .condition(crate::entity::Condition::like("topic", text("chatRoom:%")))
            .list()
            .unwrap();
        assert!(stored.is_empty(), "chat pushes are not persisted");
    }

    // typing: the others only; read: my tabs, and the others' read receipts
    tape.clear();
    ok("signal#Typing", &[("chatRoomId", text(&chat))]);
    let typing = tape.take("chatRoom:");
    assert_eq!(typing.len(), 1);
    assert_eq!(typing[0].1, ["psb"]);
    assert_eq!(typing[0].2["kind"], "typing");
    assert_eq!(typing[0].2["userId"], "psa");
    login("psb");
    tape.clear();
    ok("mark#Read", &[("chatRoomId", text(&chat))]);
    let mine = tape.take("chatUser:");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].1, ["psb"]);
    assert_eq!(mine[0].2["kind"], "read");
    assert_eq!(mine[0].2["lastReadEventSeq"], seq);
    let receipts = tape.take("chatRoom:");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].1, ["psa"]);
    assert_eq!(receipts[0].2["userId"], "psb");

    // a stranger gets none of it
    assert!(tape
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|(_, routed, _)| !routed.contains(&"psc".to_string())));

    // presence: to the members of the user's rooms and the user's own tabs
    tape.clear();
    login("psa");
    ok("heartbeat#UserStatus", &[("idleSeconds", int(0))]);
    let status = tape.take("userStatus");
    assert_eq!(status.len(), 2, "{status:?}");
    let to_others = status.iter().find(|(_, routed, _)| routed == &["psb"]).unwrap();
    assert_eq!(to_others.2["presence"], "online");
    assert_eq!(to_others.2.get("self"), None);
    let to_self = status.iter().find(|(_, routed, _)| routed == &["psa"]).unwrap();
    assert_eq!(to_self.2["self"], true);
    // the same presence is not published again
    ok("heartbeat#UserStatus", &[("idleSeconds", int(5))]);
    assert!(tape.take("userStatus").is_empty());

    // a rolled back transaction pushes nothing
    tape.clear();
    let _ = call("test.ChatStub.push#ThenFail", &[("userId", text("psc"))]);
    assert!(errors().contains("failed after the push"));
    assert!(tape.take("chatUser:").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Presence: none without a heartbeat, online with one, away after the idle minutes (the user's preference), offline
/// three minutes after the last beat; a set status with its text and clear-at time; invisible is offline to others;
/// do not disturb still records the notifications.
#[test]
fn presence_follows_the_heartbeat_the_status_and_its_clear_at_time() {
    let (ctx, facade, dir) = setup("presence", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["pra", "prb"]);
    row(
        &facade,
        "moqui.security.user.NotificationTopic",
        &[
            ("topic", text("ChatMention")),
            ("persistOnSend", text("Y")),
            ("showAlert", text("Y")),
        ],
    );
    let tape = Tape::start(&["pra", "prb"]);
    let presence_of = |user: &str| {
        let out = ok("get#UserStatuses", &[("userIdList", list(&[user]))]);
        sub(&out_map(&out, "statusMap"), user)
    };
    let start = now();
    set_time(&ctx, start);
    login("prb");
    room("direct", "", &["pra"]);
    assert_eq!(field(&presence_of("pra"), "presence"), "offline");

    login("pra");
    let mine = ok("heartbeat#UserStatus", &[("idleSeconds", int(0))]);
    assert_eq!(field(&out_map(&mine, "myStatus"), "presence"), "online");
    assert_eq!(field(&out_map(&mine, "myStatus"), "status"), "online");
    assert_eq!(number(&out_map(&mine, "myStatus"), "autoAwayMinutes"), 10);
    login("prb");
    assert_eq!(field(&presence_of("pra"), "presence"), "online");

    // idle past the auto-away minutes: away; a longer preference keeps the user online
    set_time(&ctx, start + 11 * MINUTE);
    login("pra");
    ok("heartbeat#UserStatus", &[("idleSeconds", int(11 * 60))]);
    assert_eq!(field(&presence_of("pra"), "presence"), "away");
    let pushed = tape.take("userStatus");
    assert!(
        pushed
            .iter()
            .any(|(_, routed, message)| routed == &["prb"] && message["presence"] == "away"),
        "{pushed:?}"
    );
    row(
        &facade,
        "moqui.security.UserPreference",
        &[
            ("userId", text("pra")),
            ("preferenceKey", text("CHAT_AUTO_AWAY_MINUTES")),
            ("preferenceValue", text("30")),
        ],
    );
    assert_eq!(field(&presence_of("pra"), "presence"), "online");
    // back from idle at once
    ok("heartbeat#UserStatus", &[("active", Value::Bool(true))]);
    assert_eq!(field(&presence_of("pra"), "presence"), "online");

    // no heartbeat: offline after three minutes, published by the frequent job
    tape.clear();
    set_time(&ctx, start + 15 * MINUTE);
    assert_eq!(field(&presence_of("pra"), "presence"), "offline");
    {
        let _authz = ctx.disable_authz();
        let job = ok("expire#UserStatuses", &[]);
        assert!(out_number(&job, "publishedCount") >= 1);
    }
    let pushed = tape.take("userStatus");
    assert!(
        pushed
            .iter()
            .any(|(_, routed, message)| routed == &["prb"] && message["presence"] == "offline"),
        "{pushed:?}"
    );

    // do not disturb with a text that clears in 30 minutes
    let set = ok(
        "set#UserStatus",
        &[
            ("status", text("dnd")),
            ("statusText", text("Focus time")),
            ("emoji", text("🎧")),
            ("clearAfterMinutes", int(30)),
        ],
    );
    let status = out_map(&set, "myStatus");
    assert_eq!(field(&status, "presence"), "dnd");
    assert_eq!(field(&status, "status"), "dnd");
    assert_eq!(field(&status, "statusText"), "Focus time");
    assert_eq!(number(&status, "clearAtDate"), start + 45 * MINUTE);
    let shown = presence_of("pra");
    assert_eq!(field(&shown, "presence"), "dnd");
    assert_eq!(field(&shown, "emoji"), "🎧");
    {
        let _authz = ctx.disable_authz();
        assert_eq!(
            out_number(&ok("expire#UserStatuses", &[]), "clearedCount"),
            0,
            "not yet"
        );
    }
    // do not disturb: the mention is still recorded
    login("prb");
    let pair = room("direct", "", &["pra"]);
    send(&pair, r#"<p>ping <span data-mention-id="pra">@Pra</span></p>"#);
    {
        let _authz = ctx.disable_authz();
        let rows = facade
            .find("moqui.security.user.NotificationMessageUser")
            .unwrap()
            .eq("userId", text("pra"))
            .list()
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
    assert_eq!(out_number(&counts("pra"), "mentions"), 1);

    // the clear-at time: the status reads as online at once, and the job writes it
    set_time(&ctx, start + 46 * MINUTE);
    login("pra");
    ok("heartbeat#UserStatus", &[("idleSeconds", int(0))]);
    let cleared = presence_of("pra");
    assert_eq!(field(&cleared, "presence"), "online");
    assert_eq!(cleared.get("statusText"), Some(&Value::Null));
    {
        let _authz = ctx.disable_authz();
        let job = ok("expire#UserStatuses", &[]);
        assert_eq!(out_number(&job, "clearedCount"), 1);
        let stored = facade
            .find("moqui.security.UserStatus")
            .unwrap()
            .eq("userId", text("pra"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(stored.get("statusText"), Some(&Value::Null));
        assert_eq!(to_plain_string(stored.get("statusEnumId").unwrap()), "UpsOnline");
    }

    // invisible: offline to others, invisible to oneself
    let set = ok("set#UserStatus", &[("status", text("invisible"))]);
    assert_eq!(field(&out_map(&set, "myStatus"), "status"), "invisible");
    assert_eq!(field(&out_map(&set, "myStatus"), "presence"), "offline");
    login("prb");
    assert_eq!(field(&presence_of("pra"), "presence"), "offline");
    refused(
        "set#UserStatus",
        &[("userId", text("pra")), ("status", text("online"))],
        "only your own",
    );
    refused("set#UserStatus", &[("status", text("busy"))], "Unknown status");
    ctx.set_effective_time(None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The MoquiChatEvent document: a message with its room and sender, no other events; and the search by membership
/// (without a search backend, the text of the messages of the rooms the user may read).
#[test]
fn the_search_document_and_the_search_by_membership() {
    let (ctx, facade, dir) = setup("search", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    {
        let _authz = ctx.disable_authz();
        let data = std::fs::read_to_string(crate::context::framework_dir().join("data/ChatSetupData.xml")).unwrap();
        crate::entity::load_feed::load_entity_xml_text(&facade, "ChatSetupData.xml", &data, &Default::default())
            .unwrap();
    }
    users(&facade, &["sea", "seb", "sec", "sed"]);
    login("sea");
    let team = room("private", "Team", &["seb"]);
    let quarterly = send(&team, "<p>the quarterly <strong>forecast</strong> is due</p>");
    login("sec");
    let lobby = room("public", "Lobby", &[]);
    send(&lobby, "<p>forecast party on Friday</p>");
    let own = room("private", "Notes", &[]);
    send(&own, "<p>my forecast notes</p>");

    // the document of a message; the room's creation is not one
    let documents = {
        let _authz = ctx.disable_authz();
        crate::entity::document::data_documents(
            &facade,
            "MoquiChatEvent",
            Some(crate::entity::Condition::equals("chatRoomId", text(&team))),
            None,
            None,
        )
        .unwrap()
    };
    assert_eq!(documents.len(), 1, "{documents:?}");
    let document = &documents[0];
    assert_eq!(field(document, "_id"), format!("{team}::{quarterly}"));
    assert_eq!(field(document, "_type"), "MoquiChatEvent");
    assert_eq!(field(document, "bodyText"), "the quarterly forecast is due");
    assert_eq!(field(document, "senderName"), "Sea");
    assert_eq!(field(document, "roomName"), "Team");
    assert_eq!(field(document, "roomVisibilityEnumId"), "CrvMembers");

    let found = |user: &str, more: &[(&str, Value)]| {
        login(user);
        let mut parameters = vec![("searchText", text("forecast"))];
        parameters.extend(more.iter().cloned());
        let out = ok("search#ChatEvents", &parameters);
        let mut hits: Vec<String> = maps(out.get("eventList"))
            .iter()
            .map(|hit| format!("{}:{}", field(hit, "roomName"), field(hit, "snippet")))
            .collect();
        hits.sort();
        hits
    };
    assert_eq!(found("sed", &[]), ["Lobby:forecast party on Friday"]);
    assert_eq!(
        found("sea", &[]),
        ["Lobby:forecast party on Friday", "Team:the quarterly forecast is due"]
    );
    assert_eq!(
        found("sec", &[]),
        ["Lobby:forecast party on Friday", "Notes:my forecast notes"]
    );
    assert_eq!(
        found("seb", &[("chatRoomId", text(&team))]),
        ["Team:the quarterly forecast is due"]
    );
    login("sed");
    refused(
        "search#ChatEvents",
        &[("searchText", text("forecast")), ("chatRoomId", text(&team))],
        "not found",
    );
    // the current text: an edit is found, a redaction is not
    login("sea");
    ok(
        "edit#Message",
        &[
            ("chatRoomId", text(&team)),
            ("eventSeq", int(quarterly)),
            ("body", text("<p>the quarterly budget</p>")),
        ],
    );
    assert_eq!(found("seb", &[("chatRoomId", text(&team))]), Vec::<String>::new());
    login("sea");
    let budget = ok("search#ChatEvents", &[("searchText", text("budget"))]);
    assert_eq!(out_number(&budget, "total"), 1);
    login("sec");
    ok("redact#Message", &[("chatRoomId", text(&own)), ("eventSeq", int(2))]);
    assert_eq!(found("sec", &[]), ["Lobby:forecast party on Friday"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files: uploaded to a room, sent with a message, read by those who may read the room, removed with the message; a
/// file never sent goes after a day. The in-memory database has no DbResource store, so the files go under a file:
/// root here (`chat_attachment_root`); `Chat.spec.js` stores them as DbResource files.
#[test]
fn files_go_with_their_message_and_go_away_with_it() {
    let (ctx, facade, dir) = setup("files", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["fla", "flb", "flc"]);
    let (_root_lock, root) = attachment_root(&dir);
    let upload = |name: &str, bytes: &[u8]| {
        crate::web::multipart::UploadedFile::in_memory("file", name, "text/plain", bytes.to_vec()).into_value()
    };
    login("fla");
    let chat = room("private", "Files", &["flb"]);
    let stored = ok(
        "upload#Attachment",
        &[
            ("chatRoomId", text(&chat)),
            ("file", upload("../notes/plan.txt", b"the plan")),
        ],
    );
    let attachment = out_text(&stored, "chatAttachmentId");
    let location = out_text(&stored, "location");
    assert_eq!(location, format!("{root}/{chat}/{attachment}/plan.txt"));
    assert_eq!(out_number(&stored, "fileSize"), 8);
    let bytes = ctx.factory().resource().get(&location).unwrap().read_bytes().unwrap();
    assert_eq!(bytes, b"the plan");

    // another member may not send it; the uploader sends it alone, with no text
    login("flb");
    refused(
        "send#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("chatAttachmentIdList", list(&[&attachment])),
        ],
        "not found",
    );
    refused(
        "get#Attachment",
        &[("chatAttachmentId", text(&attachment))],
        "not found",
    );
    login("fla");
    refused("send#Message", &[("chatRoomId", text(&chat))], "needs text or a file");
    let sent = ok(
        "send#Message",
        &[
            ("chatRoomId", text(&chat)),
            ("chatAttachmentIdList", list(&[&attachment])),
        ],
    );
    let seq = out_number(&sent, "eventSeq");
    let listed = events(&timeline(&chat, &[("limit", int(1))]));
    let files = maps(listed[0].get("attachments"));
    assert_eq!(files.len(), 1);
    assert_eq!(field(&files[0], "fileName"), "plan.txt");
    let rooms = ok("find#Rooms", &[]);
    assert_eq!(
        field(&sub(&maps(rooms.get("roomList"))[0], "lastMessage"), "snippet"),
        "📎 plan.txt"
    );
    login("flb");
    let info = ok("get#Attachment", &[("chatAttachmentId", text(&attachment))]);
    assert_eq!(out_text(&info, "location"), location);
    login("flc");
    refused(
        "get#Attachment",
        &[("chatAttachmentId", text(&attachment))],
        "not found",
    );
    refused(
        "upload#Attachment",
        &[("chatRoomId", text(&chat)), ("file", upload("x.txt", b"x"))],
        "not found",
    );

    // the message deleted: the file goes too
    login("fla");
    ok("redact#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq))]);
    assert!(
        !ctx.factory().resource().get(&location).unwrap().exists(),
        "the file is gone"
    );
    {
        let _authz = ctx.disable_authz();
        assert!(facade
            .find("moqui.chat.ChatAttachment")
            .unwrap()
            .eq("chatAttachmentId", text(&attachment))
            .one()
            .unwrap()
            .is_none());
    }

    // a file never sent goes after a day
    let pending = out_text(
        &ok(
            "upload#Attachment",
            &[("chatRoomId", text(&chat)), ("file", upload("draft.txt", b"draft"))],
        ),
        "chatAttachmentId",
    );
    set_time(&ctx, now() + 25 * 60 * MINUTE);
    {
        let _authz = ctx.disable_authz();
        let cleaned = ok("clean#PendingAttachments", &[]);
        assert_eq!(out_number(&cleaned, "removedCount"), 1);
        assert!(facade
            .find("moqui.chat.ChatAttachment")
            .unwrap()
            .eq("chatAttachmentId", text(&pending))
            .one()
            .unwrap()
            .is_none());
    }
    ctx.set_effective_time(None);
    moqui_lib::env::clear_property("chat_attachment_root");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The agent entry point: an agent member's service gets each message from another member, with the message in its
/// parameters, and answers in the room as the agent.
#[test]
fn an_agent_member_gets_each_message_through_its_entry_point() {
    let (ctx, facade, dir) = setup("agent", crate::service::ServicePools::inline());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["aga", "agm", "agbot"]);
    login("aga");
    let chat = room("private", "Assist", &["agm"]);
    // a person is no agent member: only an agent's chat user takes the role
    refused(
        "add#Member",
        &[
            ("chatRoomId", text(&chat)),
            ("userId", text("agbot")),
            ("roleEnumId", text("agent")),
        ],
        "is no agent's chat user",
    );
    row(
        &facade,
        "moqui.agent.AgentDefinition",
        &[("agentId", text("agbot")), ("chatUserId", text("agbot"))],
    );
    // a member who is not a moderator adds no agent
    login("agm");
    refused(
        "add#Member",
        &[
            ("chatRoomId", text(&chat)),
            ("userId", text("agbot")),
            ("roleEnumId", text("agent")),
        ],
        "Only a moderator or owner",
    );
    // the owner adds the agent, then names its service
    login("aga");
    ok(
        "add#Member",
        &[
            ("chatRoomId", text(&chat)),
            ("userId", text("agbot")),
            ("roleEnumId", text("agent")),
        ],
    );
    ok(
        "set#MemberRole",
        &[
            ("chatRoomId", text(&chat)),
            ("userId", text("agbot")),
            ("roleEnumId", text("agent")),
            ("agentRef", text("test.ChatStub.receive#AgentMessage")),
        ],
    );
    send(&chat, "<p>what is due today?</p>");
    let ran = ctx.factory().service_pools().run_pending();
    assert!(ran >= 1, "the agent's delivery was queued");
    let page = timeline(&chat, &[("limit", int(2))]);
    let shown = events(&page);
    assert_eq!(field(&shown[0], "bodyText"), "what is due today?");
    assert_eq!(field(&shown[1], "bodyText"), "echo: what is due today?");
    assert_eq!(field(&shown[1], "senderUserId"), "agbot");
    // the agent's own answer is not handed back to it
    assert_eq!(ctx.factory().service_pools().run_pending(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

fn bool_text(out: &BTreeMap<String, Value>, key: &str) -> bool {
    out_text(out, key) == "true"
}

fn ints(items: &[i64]) -> Value {
    Value::List(items.iter().map(|item| int(*item)).collect())
}

fn preference(facade: &EntityFacade, user_id: &str, key: &str, value: &str) {
    row(
        facade,
        "moqui.security.UserPreference",
        &[("userId", text(user_id)), ("preferenceKey", text(key)), ("preferenceValue", text(value))],
    );
}

fn one_event(room: &str, seq: i64) -> IndexMap<String, Value> {
    let found = events(&timeline(room, &[("eventSeq", int(seq))]));
    assert_eq!(found.len(), 1, "event {seq}");
    found[0].clone()
}

/// A send tried again with its clientRef is answered with the message it made and pushes nothing; get#Timeline reads one
/// event by its eventSeq, a thread reply too; a thread's root knows its last reply.
#[test]
fn a_retried_send_is_answered_with_its_message_and_one_event_is_read_by_its_seq() {
    let (ctx, facade, dir) = setup("retry", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["rta", "rtb"]);
    let tape = Tape::start(&["rta", "rtb"]);
    login("rta");
    let chat = room("private", "Retries", &["rtb"]);
    tape.clear();
    let parameters = [("chatRoomId", text(&chat)), ("body", text("<p>once</p>")), ("clientRef", text("c1.a:b_c-d"))];
    let first = ok("send#Message", &parameters);
    assert!(!bool_text(&first, "duplicate"));
    let seq = out_number(&first, "eventSeq");
    let pushed = tape.take("chatRoom:");
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].2["event"]["clientRef"], "c1.a:b_c-d");
    let again = ok("send#Message", &parameters);
    assert!(bool_text(&again, "duplicate"));
    assert_eq!(out_number(&again, "eventSeq"), seq);
    assert_eq!(field(&out_map(&again, "event"), "bodyText"), "once");
    assert!(tape.take("chatRoom:").is_empty(), "a retry pushes nothing");
    assert_eq!(texts(&timeline(&chat, &[])), ["once"]);
    refused(
        "send#Message",
        &[("chatRoomId", text(&chat)), ("body", text("<p>x</p>")), ("clientRef", text("not a ref!"))],
        "clientRef",
    );
    // the same clientRef from another sender is another message; its clientRef is its sender's only
    login("rtb");
    let other = ok("send#Message", &parameters);
    assert!(!bool_text(&other, "duplicate"));
    assert_ne!(out_number(&other, "eventSeq"), seq);
    assert_ne!(field(&one_event(&chat, seq), "clientRef"), "c1.a:b_c-d");
    login("rta");
    assert_eq!(field(&one_event(&chat, seq), "clientRef"), "c1.a:b_c-d");

    // one event by its eventSeq: a thread reply, its root with the last reply, and none for a number not there
    let reply = out_number(
        &ok(
            "send#Message",
            &[("chatRoomId", text(&chat)), ("body", text("<p>in the thread</p>")), ("threadRootEventSeq", int(seq))],
        ),
        "eventSeq",
    );
    let shown = one_event(&chat, reply);
    assert_eq!(number(&shown, "threadRootEventSeq"), seq);
    let root = one_event(&chat, seq);
    assert_eq!(number(&root, "threadLastEventSeq"), reply);
    assert_eq!(field(&root, "threadLastUserId"), "rta");
    assert_eq!(field(&root, "threadLastUserName"), "Rta");
    assert!(events(&timeline(&chat, &[("eventSeq", int(999))])).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The files of a room, newest first, by type and name, paged at whole messages; the threads of a room, the latest reply
/// first, mine, paged, and a thread's last reply after a redaction.
#[test]
fn the_files_and_the_threads_of_a_room_are_listed() {
    let (ctx, facade, dir) = setup("lists", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["lsa", "lsb", "lsc"]);
    let (_root_lock, _root) = attachment_root(&dir);
    login("lsa");
    let chat = room("private", "Lists", &["lsb"]);
    let upload = |name: &str, kind: &str| {
        let file = crate::web::multipart::UploadedFile::in_memory("file", name, kind, b"bytes".to_vec()).into_value();
        out_text(&ok("upload#Attachment", &[("chatRoomId", text(&chat)), ("file", file)]), "chatAttachmentId")
    };
    let send_files = |ids: &[&str]| {
        out_number(
            &ok("send#Message", &[("chatRoomId", text(&chat)), ("chatAttachmentIdList", list(ids))]),
            "eventSeq",
        )
    };
    let (a, b) = (upload("a.png", "image/png"), upload("b.txt", "text/plain"));
    let first = send_files(&[&a, &b]);
    let c = upload("c.png", "image/png");
    let second = send_files(&[&c]);
    let d = upload("d.txt", "text/plain");
    let gone = send_files(&[&d]);
    ok("redact#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(gone))]);
    let names = |out: &BTreeMap<String, Value>| -> Vec<String> {
        maps(out.get("fileList")).iter().map(|file| field(file, "fileName")).collect()
    };
    let all = ok("find#RoomFiles", &[("chatRoomId", text(&chat))]);
    assert_eq!(names(&all), ["c.png", "a.png", "b.txt"]);
    assert_eq!(number(&maps(all.get("fileList"))[0], "eventSeq"), second);
    assert_eq!(field(&maps(all.get("fileList"))[0], "senderName"), "Lsa");
    assert_eq!(out_text(&all, "nextBeforeEventSeq"), "");
    let images = ok("find#RoomFiles", &[("chatRoomId", text(&chat)), ("contentTypePrefix", text("image/"))]);
    assert_eq!(names(&images), ["c.png", "a.png"]);
    let named = ok("find#RoomFiles", &[("chatRoomId", text(&chat)), ("searchText", text("B.T"))]);
    assert_eq!(names(&named), ["b.txt"]);
    // a page of 2 ends at a whole message: the second message's files come on the next page together
    let page = ok("find#RoomFiles", &[("chatRoomId", text(&chat)), ("limit", int(2))]);
    assert_eq!(names(&page), ["c.png"]);
    assert_eq!(out_number(&page, "nextBeforeEventSeq"), second);
    let next = ok(
        "find#RoomFiles",
        &[("chatRoomId", text(&chat)), ("limit", int(2)), ("beforeEventSeq", int(second))],
    );
    assert_eq!(names(&next), ["a.png", "b.txt"]);
    assert_eq!(number(&maps(next.get("fileList"))[0], "eventSeq"), first);
    login("lsc");
    refused("find#RoomFiles", &[("chatRoomId", text(&chat))], "not found");
    refused("find#RoomThreads", &[("chatRoomId", text(&chat))], "not found");

    // threads: the one with the latest reply first
    let start = now();
    let reply = |user: &str, root: i64, body: &str, minute: i64| {
        set_time(&ctx, start + minute * MINUTE);
        login(user);
        out_number(
            &ok(
                "send#Message",
                &[("chatRoomId", text(&chat)), ("body", text(body)), ("threadRootEventSeq", int(root))],
            ),
            "eventSeq",
        )
    };
    login("lsa");
    let one = send(&chat, "<p>first topic</p>");
    login("lsb");
    let two = send(&chat, "<p>second topic</p>");
    reply("lsb", one, "<p>b on one</p>", 1);
    let early = reply("lsa", two, "<p>a on two</p>", 2);
    let late = reply("lsa", two, "<p>a again on two</p>", 3);
    let roots = |out: &BTreeMap<String, Value>| -> Vec<i64> {
        maps(out.get("threadList")).iter().map(|thread| number(thread, "eventSeq")).collect()
    };
    login("lsa");
    let threads = ok("find#RoomThreads", &[("chatRoomId", text(&chat))]);
    assert_eq!(roots(&threads), [two, one]);
    let top = &maps(threads.get("threadList"))[0];
    assert_eq!(number(top, "threadReplyCount"), 2);
    assert_eq!(number(top, "threadLastEventSeq"), late);
    assert_eq!(field(top, "threadLastUserName"), "Lsa");
    assert_eq!(field(top, "lastSnippet"), "a again on two");
    assert_eq!(field(top, "snippet"), "second topic");
    // mine: started by me or replied in
    let mine = ok("find#RoomThreads", &[("chatRoomId", text(&chat)), ("mine", Value::Bool(true))]);
    assert_eq!(roots(&mine), [two, one]);
    // paged
    let first_page = ok("find#RoomThreads", &[("chatRoomId", text(&chat)), ("limit", int(1))]);
    assert!(bool_text(&first_page, "hasMore"));
    let before = number(&maps(first_page.get("threadList"))[0], "threadLastDate");
    let rest = ok(
        "find#RoomThreads",
        &[("chatRoomId", text(&chat)), ("limit", int(1)), ("beforeDate", int(before))],
    );
    assert_eq!(roots(&rest), [one]);
    assert!(!bool_text(&rest, "hasMore"));
    // the last reply redacted: the one before it is the last
    ok("redact#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(late))]);
    let root = one_event(&chat, two);
    assert_eq!(number(&root, "threadReplyCount"), 1);
    assert_eq!(number(&root, "threadLastEventSeq"), early);
    ctx.set_effective_time(None);
    moqui_lib::env::clear_property("chat_attachment_root");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A forward: made by the server from a message of a room the sender is in, its mentions inert, nothing of its room, its
/// files copied into the room it goes to; not from a room the sender is not in, nor a redacted message.
#[test]
fn a_forward_is_made_from_a_message_the_sender_may_read() {
    let (ctx, facade, dir) = setup("forward", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["fwa", "fwb", "fwc"]);
    let (_root_lock, _root) = attachment_root(&dir);
    login("fwb");
    let source = room("private", "Source", &["fwa"]);
    let file = crate::web::multipart::UploadedFile::in_memory("file", "notes.txt", "text/plain", b"the notes".to_vec()).into_value();
    let attachment = out_text(&ok("upload#Attachment", &[("chatRoomId", text(&source)), ("file", file)]), "chatAttachmentId");
    let original = out_number(
        &ok(
            "send#Message",
            &[
                ("chatRoomId", text(&source)),
                ("body", text(r#"<p>For <span data-mention-id="fwa">@Fwa</span>: the plan</p>"#)),
                ("chatAttachmentIdList", list(&[&attachment])),
            ],
        ),
        "eventSeq",
    );
    login("fwa");
    let target = room("private", "Target", &["fwc"]);
    let sent = ok(
        "send#Message",
        &[
            ("chatRoomId", text(&target)),
            ("body", text("<p>see this</p>")),
            ("forwardOfChatRoomId", text(&source)),
            ("forwardOfEventSeq", int(original)),
        ],
    );
    let seq = out_number(&sent, "eventSeq");
    login("fwc");
    let shown = one_event(&target, seq);
    let forward = sub(&shown, "forwardOf");
    assert_eq!(field(&forward, "senderName"), "Fwb");
    assert_eq!(field(&forward, "bodyText"), "For @Fwa: the plan");
    assert!(!field(&forward, "body").contains("data-mention-id"), "{}", field(&forward, "body"));
    assert!(field(&forward, "body").contains("@Fwa"));
    assert!(!forward.contains_key("chatRoomId") && !forward.contains_key("roomName"));
    assert_eq!(field(&shown, "body"), "<p>see this</p>");
    assert_eq!(out_number(&counts("fwc"), "mentions"), 0);
    let files = maps(shown.get("attachments"));
    assert_eq!(files.len(), 1);
    let copy = field(&files[0], "chatAttachmentId");
    assert_ne!(copy, attachment);
    let info = ok("get#Attachment", &[("chatAttachmentId", text(&copy))]);
    assert_eq!(out_text(&info, "chatRoomId"), target);
    let bytes = ctx.factory().resource().get(&out_text(&info, "location")).unwrap().read_bytes().unwrap();
    assert_eq!(bytes, b"the notes");
    refused("get#Attachment", &[("chatAttachmentId", text(&attachment))], "not found");
    // not from a room I am not in
    refused(
        "send#Message",
        &[("chatRoomId", text(&target)), ("forwardOfChatRoomId", text(&source)), ("forwardOfEventSeq", int(original))],
        "not found",
    );
    // with no comment; not a redacted message
    login("fwa");
    ok(
        "send#Message",
        &[("chatRoomId", text(&target)), ("forwardOfChatRoomId", text(&source)), ("forwardOfEventSeq", int(original))],
    );
    login("fwb");
    ok("redact#Message", &[("chatRoomId", text(&source)), ("eventSeq", int(original))]);
    login("fwa");
    refused(
        "send#Message",
        &[("chatRoomId", text(&target)), ("forwardOfChatRoomId", text(&source)), ("forwardOfEventSeq", int(original))],
        "not found",
    );
    moqui_lib::env::clear_property("chat_attachment_root");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Polls: 2 to 10 options; a vote replaces the voter's, an empty one takes it back; one option unless multiple; the
/// update push is the same for everyone (no mine; no voters of an anonymous poll); closed by its creator or a
/// moderator, then no votes; not edited; a redaction removes it.
#[test]
fn a_poll_takes_votes_until_it_is_closed() {
    let (ctx, facade, dir) = setup("polls", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["pla", "plb", "plc"]);
    let tape = Tape::start(&["pla", "plb", "plc"]);
    login("pla");
    let chat = room("private", "Polls", &["plb", "plc"]);
    let poll = |options: &[&str], more: &[(&'static str, Value)]| -> Vec<(&'static str, Value)> {
        let mut parameters = vec![
            ("chatRoomId", text(&chat)),
            ("question", text("Lunch?")),
            ("optionList", list(options)),
        ];
        parameters.extend(more.iter().cloned());
        parameters
    };
    refused("create#Poll", &poll(&["only"], &[]), "2 to 10 options");
    refused("create#Poll", &poll(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"], &[]), "2 to 10 options");
    refused("create#Poll", &poll(&["Pizza", "pizza"], &[]), "twice");
    tape.clear();
    let made = ok("create#Poll", &poll(&["Pizza", "Sushi", "Salad"], &[]));
    let seq = out_number(&made, "eventSeq");
    let state = sub(&out_map(&made, "event"), "poll");
    assert_eq!(maps(state.get("options")).len(), 3);
    assert_eq!(field(&maps(state.get("options"))[0], "mine"), "false");
    let pushed = tape.take("chatRoom:");
    assert_eq!(pushed.len(), 2, "the event, then its poll: {pushed:?}");
    assert_eq!(pushed[0].2["kind"], "event");
    assert_eq!(pushed[1].2["kind"], "update");
    assert_eq!(pushed[1].2["eventSeq"], seq);
    assert!(pushed[1].2["fields"]["poll"]["options"][0].get("mine").is_none());
    assert!(pushed[1].2["fields"]["poll"].get("mine").is_none());

    login("plb");
    refused(
        "vote#Poll",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[1, 2]))],
        "Choose one",
    );
    refused(
        "vote#Poll",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[9]))],
        "not found",
    );
    tape.clear();
    let voted = ok("vote#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[1]))]);
    let state = out_map(&voted, "poll");
    assert_eq!(number(&maps(state.get("options"))[0], "voteCount"), 1);
    assert_eq!(field(&maps(state.get("options"))[0], "mine"), "true");
    let update = tape.take("chatRoom:");
    assert_eq!(update.len(), 1);
    assert_eq!(update[0].2["fields"]["poll"]["options"][0]["voterIds"][0], "plb");
    // a new vote replaces the old one
    let changed = ok("vote#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[2]))]);
    let state = out_map(&changed, "poll");
    let options = maps(state.get("options"));
    assert_eq!((number(&options[0], "voteCount"), number(&options[1], "voteCount")), (0, 1));
    assert_eq!(number(&state, "voterCount"), 1);
    login("plc");
    ok("vote#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[2]))]);
    let seen = sub(&one_event(&chat, seq), "poll");
    assert_eq!(number(&maps(seen.get("options"))[1], "voteCount"), 2);
    assert_eq!(field(&maps(seen.get("options"))[1], "mine"), "true");
    assert_eq!(number(&seen, "totalVotes"), 2);
    // taken back
    let back = ok("vote#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[]))]);
    assert_eq!(number(&out_map(&back, "poll"), "voterCount"), 1);
    // closed by its creator, not by another member
    refused("close#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq))], "creator or a moderator");
    login("pla");
    refused(
        "edit#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("body", text("<p>Dinner?</p>"))],
        "A poll is not edited",
    );
    let closed = ok("close#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq))]);
    assert_eq!(field(&out_map(&closed, "poll"), "closed"), "true");
    login("plb");
    refused(
        "vote#Poll",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("optionIdList", ints(&[3]))],
        "closed",
    );

    // anonymous: no voters in the push or the timeline
    login("pla");
    let secret = out_number(
        &ok("create#Poll", &poll(&["Yes", "No"], &[("anonymous", Value::Bool(true)), ("multiple", Value::Bool(true))])),
        "eventSeq",
    );
    login("plb");
    tape.clear();
    ok("vote#Poll", &[("chatRoomId", text(&chat)), ("eventSeq", int(secret)), ("optionIdList", ints(&[1, 2]))]);
    let update = tape.take("chatRoom:");
    assert_eq!(update[0].2["fields"]["poll"]["options"][0]["voteCount"], 1);
    assert_eq!(update[0].2["fields"]["poll"]["options"][0]["voterIds"], serde_json::json!([]));
    login("pla");
    let seen = sub(&one_event(&chat, secret), "poll");
    assert!(maps(seen.get("options")).iter().all(|option| maps(option.get("voterIds")).is_empty()));
    // a redaction removes the poll
    ok("redact#Message", &[("chatRoomId", text(&chat)), ("eventSeq", int(secret))]);
    {
        let _authz = ctx.disable_authz();
        for entity in ["moqui.chat.ChatPoll", "moqui.chat.ChatPollOption", "moqui.chat.ChatPollVote"] {
            let left = facade.find(entity).unwrap().eq("chatRoomId", text(&chat)).eq("eventSeq", int(secret)).list().unwrap();
            assert!(left.is_empty(), "{entity}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Link previews: fetched after the commit (here from a stub of the pages), each link on its own (a broken one has no
/// card), kept on the message and pushed as an update, cached; removed by the sender for good; none for a sender who
/// turned them off.
#[test]
fn the_links_of_a_message_get_text_previews_after_the_commit() {
    let (ctx, facade, dir) = setup("links", crate::service::ServicePools::inline());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["lpa", "lpb"]);
    moqui_lib::env::set_property("chat_link_preview_service", "test.ChatStub.get#LinkPage");
    let tape = Tape::start(&["lpa", "lpb"]);
    login("lpa");
    let chat = room("private", "Links", &["lpb"]);
    let seq = send(
        &chat,
        r#"<p>See <a href="https://example.com/one">https://example.com/one</a> and https://broken.example/x</p>"#,
    );
    // the fetch runs after the commit (inline pools run it at the next drain point)
    ctx.factory().service_pools().run_pending();
    let previews = maps(one_event(&chat, seq).get("linkPreviews"));
    assert_eq!(previews.len(), 1, "{previews:?}");
    assert_eq!(field(&previews[0], "title"), "Page https://example.com/one");
    assert_eq!(field(&previews[0], "description"), "About it");
    assert_eq!(field(&previews[0], "url"), "https://example.com/one");
    let update: Vec<_> = tape.take("chatRoom:").into_iter().filter(|(_, _, message)| message["kind"] == "update").collect();
    assert_eq!(update.len(), 1);
    assert_eq!(update[0].2["fields"]["linkPreviews"][0]["title"], "Page https://example.com/one");
    {
        let _authz = ctx.disable_authz();
        let cached = facade.find("moqui.chat.ChatLinkPreview").unwrap().list().unwrap();
        let mine: Vec<_> = cached
            .iter()
            .filter(|row| {
                let url = row.get("url").map(to_plain_string).unwrap_or_default();
                url == "https://example.com/one" || url == "https://broken.example/x"
            })
            .collect();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().any(|row| row.get("failed").map(to_plain_string).as_deref() == Some("Y")));
    }
    // removed by its sender, not by another member, and not fetched again on an edit
    login("lpb");
    refused("remove#LinkPreviews", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq))], "Only the sender");
    login("lpa");
    ok("remove#LinkPreviews", &[("chatRoomId", text(&chat)), ("eventSeq", int(seq))]);
    assert!(maps(one_event(&chat, seq).get("linkPreviews")).is_empty());
    ok(
        "edit#Message",
        &[("chatRoomId", text(&chat)), ("eventSeq", int(seq)), ("body", text("<p>Now https://example.com/two</p>"))],
    );
    ctx.factory().service_pools().run_pending();
    assert!(maps(one_event(&chat, seq).get("linkPreviews")).is_empty());
    // a sender who turned link previews off; another member's link is previewed
    preference(&facade, "lpa", "CHAT_LINK_PREVIEWS", "N");
    let off = send(&chat, "<p>https://example.com/three</p>");
    login("lpb");
    let on = send(&chat, "<p>https://example.com/four</p>");
    ctx.factory().service_pools().run_pending();
    assert!(maps(one_event(&chat, off).get("linkPreviews")).is_empty());
    assert_eq!(maps(one_event(&chat, on).get("linkPreviews")).len(), 1);
    moqui_lib::env::clear_property("chat_link_preview_service");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Read receipts by preference: a member who turned them off pushes no read marker and shows none; @room mentions every
/// member, from a moderator or owner, or from anyone in a room of 20 members or fewer.
#[test]
fn read_receipts_follow_the_preference_and_room_mentions_reach_everyone() {
    let (ctx, facade, dir) = setup("receipts", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["rra", "rrb", "rrc"]);
    preference(&facade, "rrb", "CHAT_READ_RECEIPTS", "N");
    let tape = Tape::start(&["rra", "rrb", "rrc"]);
    login("rra");
    let chat = room("private", "Receipts", &["rrb", "rrc"]);
    let seq = send(&chat, "<p>read me</p>");
    tape.clear();
    login("rrb");
    ok("mark#Read", &[("chatRoomId", text(&chat))]);
    assert_eq!(tape.take("chatUser:").len(), 1, "my own tabs still hear of it");
    assert!(tape.take("chatRoom:").is_empty(), "no read receipt");
    login("rrc");
    ok("mark#Read", &[("chatRoomId", text(&chat))]);
    assert_eq!(tape.take("chatRoom:").len(), 1);
    login("rra");
    let members = maps(ok("get#Room", &[("chatRoomId", text(&chat))]).get("memberList"));
    let marker = |id: &str| field(members.iter().find(|member| field(member, "userId") == id).unwrap(), "lastReadEventSeq");
    assert_eq!(marker("rrb"), "");
    assert_eq!(marker("rrc"), seq.to_string());
    login("rrb");
    let own = maps(ok("get#Room", &[("chatRoomId", text(&chat))]).get("memberList"));
    assert_eq!(
        field(own.iter().find(|member| field(member, "userId") == "rrb").unwrap(), "lastReadEventSeq"),
        seq.to_string(),
        "my own marker"
    );

    // @room from a member of a small room
    let everyone = r#"<p><span data-mention-id="@room">@room</span> standup</p>"#;
    let mentioned = |out: &BTreeMap<String, Value>| -> Vec<String> {
        let mut ids: Vec<String> = match out_map(out, "event").get("mentions") {
            Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
            _ => Vec::new(),
        };
        ids.sort();
        ids
    };
    let sent = ok("send#Message", &[("chatRoomId", text(&chat)), ("body", text(everyone))]);
    assert_eq!(mentioned(&sent), ["rra", "rrc"]);
    // a room of 21: a member's @room is plain text, the owner's reaches everyone
    let many: Vec<String> = (1..=19).map(|n| format!("rrm{n:02}")).collect();
    let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
    users(&facade, &many_refs);
    login("rra");
    let mut members = many_refs.clone();
    members.push("rrb");
    let big = room("private", "Big", &members);
    login("rrb");
    let plain = ok("send#Message", &[("chatRoomId", text(&big)), ("body", text(everyone))]);
    assert!(mentioned(&plain).is_empty());
    login("rra");
    let loud = ok("send#Message", &[("chatRoomId", text(&big)), ("body", text(everyone))]);
    assert_eq!(mentioned(&loud).len(), 20);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The retention job: each room with retentionDays loses its events older than that, with their mentions, polls and the
/// reactions to them; a room with a longer retention or none keeps its events.
#[test]
fn retention_deletes_each_rooms_old_events() {
    let (ctx, facade, dir) = setup("retention", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["rta", "rtb"]);
    let start = now();
    set_time(&ctx, start - 3 * 24 * 60 * MINUTE);
    login("rta");
    let create = |name: &str, days: Option<i64>| {
        let mut parameters = vec![
            ("roomTypeEnumId", text("private")),
            ("roomName", text(name)),
            ("memberUserIdList", list(&["rtb"])),
        ];
        if let Some(days) = days {
            parameters.push(("retentionDays", int(days)));
        }
        out_text(&ok("create#Room", &parameters), "chatRoomId")
    };
    let short = create("Short", Some(1));
    let long = create("Long", Some(7));
    let always = create("Always", None);
    let old = send(&short, r#"<p>old <span data-mention-id="rtb">@Rtb</span></p>"#);
    let poll = out_number(
        &ok(
            "create#Poll",
            &[
                ("chatRoomId", text(&short)),
                ("question", text("Lunch?")),
                ("optionList", list(&["Yes", "No"])),
            ],
        ),
        "eventSeq",
    );
    send(&long, "<p>old in long</p>");
    send(&always, "<p>old in always</p>");
    set_time(&ctx, start);
    let fresh = send(&short, "<p>fresh</p>");
    // a reaction today to the old message goes with it
    ok(
        "react#Message",
        &[("chatRoomId", text(&short)), ("eventSeq", int(old)), ("key", text("👍"))],
    );
    assert_eq!(out_number(&counts("rtb"), "mentions"), 1);

    let out = {
        let _authz = ctx.disable_authz();
        ok("clean#ChatRetention", &[])
    };
    assert_eq!(out_number(&out, "roomCount"), 2, "{out:?}");
    assert_eq!(out_number(&out, "failedCount"), 0);
    let left = timeline(&short, &[]);
    assert_eq!(texts(&left), ["fresh"]);
    assert_eq!(seqs(&left), [fresh], "the reaction to the old message went with it");
    assert_eq!(texts(&timeline(&long, &[])), ["old in long"]);
    assert_eq!(texts(&timeline(&always, &[])), ["old in always"]);
    let rows = |entity: &str| {
        let _authz = ctx.disable_authz();
        facade
            .find(entity)
            .unwrap()
            .eq("chatRoomId", text(&short))
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(rows("moqui.chat.ChatEventMention"), 0);
    assert_eq!(rows("moqui.chat.ChatPoll"), 0, "poll {poll}");
    assert_eq!(rows("moqui.chat.ChatPollOption"), 0);
    let b = counts("rtb");
    assert_eq!(out_number(&b, "mentions"), 0);
    assert_eq!(out_number(&b, "unread"), 3, "fresh in Short, and the old ones of Long and Always");
    let listed = maps(ok("find#Rooms", &[]).get("roomList"));
    let short_item = listed.iter().find(|item| field(item, "chatRoomId") == short).unwrap();
    assert_eq!(number(&sub(short_item, "lastMessage"), "eventSeq"), fresh);
    // a second run has nothing left to delete
    let again = {
        let _authz = ctx.disable_authz();
        ok("clean#ChatRetention", &[])
    };
    assert_eq!(out_number(&again, "deletedCount"), 0);
    ctx.set_effective_time(None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mentioned user who is not a member of a record room counts when they may see the record, which the hook answers as
/// that user: notified, with no room count. One who may not see it is not mentioned.
#[test]
fn a_record_rooms_mention_reaches_a_non_member_who_may_see_the_record() {
    let (ctx, facade, dir) = setup("record-mention", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["rma", "rmseer", "rmblind"]);
    row(
        &facade,
        "moqui.security.user.NotificationTopic",
        &[
            ("topic", text("ChatMention")),
            ("persistOnSend", text("Y")),
            ("titleTemplate", text("${authorName} mentioned you in ${roomName}")),
        ],
    );
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", int(990)),
            ("resourceTypeEnumId", text("STRT_RECORD_VISIBLE_SERVICE")),
            ("resourceValue", text("test.ChatStub.check#Visible")),
        ],
    );
    login("rma");
    let record = out_text(
        &ok(
            "create#Room",
            &[
                ("roomTypeEnumId", text("record")),
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("rma+rmseer")),
            ],
        ),
        "chatRoomId",
    );
    // the hook answers for the user asked about, as that user, only for internal work
    let visible_to = |user: &str| {
        let _authz = ctx.disable_authz();
        let out = ok(
            "check#RecordVisible",
            &[
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("rma+rmseer")),
                ("userId", text(user)),
            ],
        );
        out.get("recordVisible") == Some(&Value::Bool(true))
    };
    assert!(visible_to("rmseer"));
    assert!(!visible_to("rmblind"));
    let refused_out = ok(
        "check#RecordVisible",
        &[
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("rma+rmseer")),
            ("userId", text("rmseer")),
        ],
    );
    assert_eq!(refused_out.get("recordVisible"), Some(&Value::Bool(false)), "another user only with authorization disabled");
    let level_of = |user: &str| {
        let _authz = ctx.disable_authz();
        out_number(
            &ok("get#RoomAccess", &[("chatRoomId", text(&record)), ("userId", text(user))]),
            "level",
        )
    };
    assert_eq!(level_of("rmseer"), 1);
    assert_eq!(level_of("rmblind"), 0);

    let both = r#"<p>see <span data-mention-id="rmseer">@Rmseer</span> and <span data-mention-id="rmblind">@Rmblind</span></p>"#;
    let sent = ok("send#Message", &[("chatRoomId", text(&record)), ("body", text(both))]);
    let mentions = |out: &BTreeMap<String, Value>| -> Vec<String> {
        match out_map(out, "event").get("mentions") {
            Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
            _ => Vec::new(),
        }
    };
    assert_eq!(mentions(&sent), ["rmseer"]);
    let notified = |user: &str| {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.security.user.NotificationMessageUser")
            .unwrap()
            .eq("userId", text(user))
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(notified("rmseer"), 1);
    assert_eq!(notified("rmblind"), 0);
    let seer = counts("rmseer");
    assert_eq!(out_number(&seer, "total"), 0, "no room count: not a member");
    assert!(out_map(&seer, "roomCounts").is_empty());

    // an edit that adds the mentions: the same
    let plain = send(&record, "<p>plain</p>");
    let edited = ok(
        "edit#Message",
        &[("chatRoomId", text(&record)), ("eventSeq", int(plain)), ("body", text(both))],
    );
    assert_eq!(mentions(&edited), ["rmseer"]);
    assert_eq!(notified("rmseer"), 2);
    assert_eq!(notified("rmblind"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The unread and mention counts of one query each (the views ChatMemberUnread and ChatMemberMentions) equal the counts
/// of each room the services made before: the messages (not redacted) from others after the read marker, and the
/// mentions after it.
#[test]
fn the_counts_of_one_query_equal_the_counts_per_room() {
    let (ctx, facade, dir) = setup("counts-view", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["cva", "cvb", "cvc"]);
    login("cva");
    let first = room("private", "First", &["cvb", "cvc"]);
    let second = room("public", "Second", &["cvb"]);
    let quiet = room("private", "Quiet", &["cvb"]);
    send(&first, r#"<p>one <span data-mention-id="cvb">@Cvb</span></p>"#);
    send(&first, "<p>two</p>");
    send(&second, r#"<p>three <span data-mention-id="cvb">@Cvb</span> <span data-mention-id="cvc">@Cvc</span></p>"#);
    send(&quiet, "<p>read</p>");
    login("cvb");
    ok("mark#Read", &[("chatRoomId", text(&quiet))]);
    send(&first, "<p>my own</p>");
    login("cvc");
    send(&first, r#"<p>four <span data-mention-id="cvb">@Cvb</span></p>"#);
    login("cva");
    let gone = send(&first, r#"<p>gone <span data-mention-id="cvb">@Cvb</span></p>"#);
    ok("redact#Message", &[("chatRoomId", text(&first)), ("eventSeq", int(gone))]);
    send(&second, "<p>five</p>");
    ok("add#Member", &[("chatRoomId", text(&second)), ("userId", text("cvc"))]);
    send(&second, "<p>six</p>");

    // the old counts: two finds per room
    let per_room = |user: &str| -> BTreeMap<String, (i64, i64)> {
        let _authz = ctx.disable_authz();
        let mut out = BTreeMap::new();
        for member in facade.find("moqui.chat.ChatRoomMember").unwrap().eq("userId", text(user)).list().unwrap().iter() {
            if member.get("leftDate").is_some_and(|value| !matches!(value, Value::Null)) {
                continue;
            }
            let room_id = to_plain_string(member.get("chatRoomId").unwrap());
            let last_read: i64 = member.get("lastReadEventSeq").map(to_plain_string).and_then(|seq| seq.parse().ok()).unwrap_or(0);
            let events = facade.find("moqui.chat.ChatEvent").unwrap().eq("chatRoomId", text(&room_id)).list().unwrap();
            let seq = |row: &crate::entity::EntityValue| to_plain_string(row.get("eventSeq").unwrap()).parse::<i64>().unwrap();
            let empty = |row: &crate::entity::EntityValue, name: &str| row.get(name).is_none_or(|value| matches!(value, Value::Null));
            let unread = events
                .iter()
                .filter(|row| to_plain_string(row.get("eventTypeEnumId").unwrap()) == "CetMessage")
                .filter(|row| seq(row) > last_read && empty(row, "redactedDate"))
                .filter(|row| !empty(row, "senderUserId") && to_plain_string(row.get("senderUserId").unwrap()) != user)
                .count();
            let mentions = facade
                .find("moqui.chat.ChatEventMention")
                .unwrap()
                .eq("chatRoomId", text(&room_id))
                .eq("userId", text(user))
                .list()
                .unwrap()
                .iter()
                .filter(|row| seq(row) > last_read)
                .count();
            if unread > 0 || mentions > 0 {
                out.insert(room_id, (unread as i64, mentions as i64));
            }
        }
        out
    };
    for user in ["cva", "cvb", "cvc"] {
        let expected = per_room(user);
        let from_counts: BTreeMap<String, (i64, i64)> = out_map(&counts(user), "roomCounts")
            .iter()
            .map(|(room_id, value)| match value {
                Value::Map(map) => (room_id.clone(), (number(map, "unread"), number(map, "mentions"))),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(from_counts, expected, "get#UnreadCounts of {user}");
        let listed = {
            let _authz = ctx.disable_authz();
            ok("find#Rooms", &[("userId", text(user))])
        };
        let from_rooms: BTreeMap<String, (i64, i64)> = maps(listed.get("roomList"))
            .iter()
            .map(|item| (field(item, "chatRoomId"), (number(item, "unreadCount"), number(item, "mentionCount"))))
            .filter(|(_, (unread, mentions))| *unread > 0 || *mentions > 0)
            .collect();
        assert_eq!(from_rooms, expected, "find#Rooms of {user}");
    }
    assert_eq!(
        per_room("cvb").get(&first),
        Some(&(1, 1)),
        "four, after cvb's own message; gone is redacted"
    );
    assert!(!per_room("cvb").contains_key(&quiet));
    assert_eq!(per_room("cvb").get(&second), Some(&(3, 1)), "three, five and six; three mentions cvb");
    let _ = std::fs::remove_dir_all(&dir);
}
/// An external user (the permission EXTERNAL_USER, which an app grants its client contacts) reaches no record room, not
/// as a member either, nor a public room they are not in, is never added to or mentioned into a record room, hears none
/// of its pushes, sees no other user's presence, and never holds the agent permissions; an internal user keeps all of it.
#[test]
fn an_external_user_reaches_no_record_room_public_room_presence_or_agent() {
    let (ctx, facade, dir) = setup("external", crate::service::ServicePools::idle());
    let _entered = ctx.enter();
    install(&dir);
    users(&facade, &["xtin", "xtpeer", "xtout"]);
    row(
        &facade,
        "moqui.security.UserPermission",
        &[("userPermissionId", text("EXTERNAL_USER"))],
    );
    row(
        &facade,
        "moqui.security.UserPermission",
        &[("userPermissionId", text("AgentUse"))],
    );
    for (group, member, permissions) in [
        ("XT_OUTSIDE", "xtout", &["EXTERNAL_USER", "AgentUse"][..]),
        ("XT_STAFF", "xtpeer", &["AgentUse"][..]),
    ] {
        row(&facade, "moqui.security.UserGroup", &[("userGroupId", text(group))]);
        row(
            &facade,
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", text(group)),
                ("userId", text(member)),
                ("fromDate", int(0)),
            ],
        );
        for permission in permissions {
            row(
                &facade,
                "moqui.security.UserGroupPermission",
                &[
                    ("userGroupId", text(group)),
                    ("userPermissionId", text(permission)),
                    ("fromDate", int(0)),
                ],
            );
        }
    }
    row(
        &facade,
        "moqui.security.user.NotificationTopic",
        &[("topic", text("ChatMention")), ("persistOnSend", text("Y"))],
    );
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", int(990)),
            ("resourceTypeEnumId", text("STRT_RECORD_VISIBLE_SERVICE")),
            ("resourceValue", text("test.ChatStub.check#Visible")),
        ],
    );
    // the agent permissions are withheld from the external user, whatever its group grants
    assert!(crate::user::is_external_user("xtout"));
    assert!(!crate::user::is_external_user("xtpeer"));
    assert!(!crate::user::user_has_permission("xtout", "AgentUse").unwrap());
    assert!(crate::user::user_has_permission("xtpeer", "AgentUse").unwrap());
    assert!(crate::user::user_has_permission("xtout", "EXTERNAL_USER").unwrap());
    // and a model call made for them, by any app's code, is refused; an internal user's goes through
    let llm = crate::llm::LlmFacade::new(
        crate::llm::conf::LlmConf::from_conf_text(
            r#"<llm-facade><endpoint id="external-fake" protocol="fake"/>
                <model id="external-model" endpoint="external-fake" name="fake-1"/>
                <bind feature="summarize" model="external-model" reasoning="none"/></llm-facade>"#,
        )
        .unwrap(),
    );
    crate::llm::provider::fake::clear("external-fake");
    let ask = crate::llm::CompleteRequest {
        feature: Some("summarize".to_string()),
        prompt: Some("the record's text".to_string()),
        ..crate::llm::CompleteRequest::default()
    };
    login("xtout");
    let refused_call = llm.complete(&facade, &ask).unwrap_err();
    assert!(refused_call.to_string().contains("not available to this user"), "{refused_call}");
    assert!(crate::llm::provider::fake::requests("external-fake").is_empty());
    login("xtpeer");
    llm.complete(&facade, &ask).unwrap();

    let tape = Tape::start(&["xtin", "xtpeer", "xtout"]);
    login("xtin");
    // a record room of a record every user may see (OPEN), a public room and a private room with the external user
    let record = out_text(
        &ok(
            "create#Room",
            &[
                ("roomTypeEnumId", text("record")),
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("OPEN-1")),
            ],
        ),
        "chatRoomId",
    );
    let public = room("public", "Lobby", &[]);
    let private = room("private", "With client", &["xtout", "xtpeer"]);
    refused(
        "add#Member",
        &[("chatRoomId", text(&record)), ("userId", text("xtout"))],
        "may not be in a record room",
    );
    refused(
        "create#Room",
        &[
            ("roomTypeEnumId", text("record")),
            ("recordEntityName", text("test.Thing")),
            ("recordId", text("OPEN-2")),
            ("memberUserIdList", list(&["xtout"])),
        ],
        "may not be in a record room",
    );
    // a member row made before (or by a path that did not ask) counts for nothing
    row(
        &facade,
        "moqui.chat.ChatRoomMember",
        &[
            ("chatRoomId", text(&record)),
            ("userId", text("xtout")),
            ("roleEnumId", text("CmrMember")),
            ("joinedDate", int(now())),
            ("lastReadEventSeq", int(0)),
            ("notificationLevelEnumId", text("CnlAll")),
        ],
    );
    let level_of = |room: &str, user: &str| {
        let _authz = ctx.disable_authz();
        out_number(
            &ok("get#RoomAccess", &[("chatRoomId", text(room)), ("userId", text(user))]),
            "level",
        )
    };
    assert_eq!(level_of(&record, "xtout"), 0, "an external member of a record room");
    assert_eq!(
        level_of(&public, "xtout"),
        0,
        "a public room the external user is not in"
    );
    assert_eq!(level_of(&private, "xtout"), 2, "a room the external user was put in");
    assert_eq!(level_of(&record, "xtpeer"), 1, "an internal user reads the record room");
    assert_eq!(level_of(&public, "xtpeer"), 1);
    let visible = |user: &str| {
        let _authz = ctx.disable_authz();
        ok(
            "check#RecordVisible",
            &[
                ("recordEntityName", text("test.Thing")),
                ("recordId", text("OPEN-1")),
                ("userId", text(user)),
            ],
        )
        .get("recordVisible")
            == Some(&Value::Bool(true))
    };
    assert!(!visible("xtout"));
    assert!(visible("xtpeer"));

    tape.clear();
    let mention =
        r#"<p>hi <span data-mention-id="xtout">@Xtout</span> <span data-mention-id="xtpeer">@Xtpeer</span></p>"#;
    let sent = ok(
        "send#Message",
        &[("chatRoomId", text(&record)), ("body", text(mention))],
    );
    let mentions: Vec<String> = match out_map(&sent, "event").get("mentions") {
        Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    assert_eq!(
        mentions,
        ["xtpeer"],
        "an external user is not mentioned into a record room"
    );
    send(&private, mention);
    let record_pushes = tape.take(&format!("chatRoom:{record}"));
    assert!(!record_pushes.is_empty());
    assert!(
        record_pushes
            .iter()
            .all(|(_, users, _)| !users.contains(&"xtout".to_string())),
        "{record_pushes:?}"
    );
    let private_pushes = tape.take(&format!("chatRoom:{private}"));
    assert!(private_pushes
        .iter()
        .any(|(_, users, _)| users.contains(&"xtout".to_string())));

    // the external user's own lists and counts leave the record room and the public room out
    login("xtout");
    let listed = room_ids(&ok("find#Rooms", &[("includeJoinable", Value::Bool(true))]));
    assert_eq!(listed, [private.clone()]);
    let mine = counts("xtout");
    assert!(!out_map(&mine, "roomCounts").contains_key(&record), "{mine:?}");
    refused("get#Timeline", &[("chatRoomId", text(&record))], "not found");
    refused("get#Timeline", &[("chatRoomId", text(&public))], "not found");
    refused(
        "send#Message",
        &[("chatRoomId", text(&record)), ("body", text("<p>me</p>"))],
        "not found",
    );
    refused("add#Member", &[("chatRoomId", text(&public))], "not found");

    // presence: the external user sees their own status only, and hears of no one else's
    ok("heartbeat#UserStatus", &[("idleSeconds", int(0))]);
    let statuses = out_map(
        &ok("get#UserStatuses", &[("userIdList", list(&["xtin", "xtout"]))]),
        "statusMap",
    );
    assert_eq!(statuses.keys().cloned().collect::<Vec<_>>(), ["xtout"]);
    login("xtpeer");
    let statuses = out_map(
        &ok("get#UserStatuses", &[("userIdList", list(&["xtin", "xtout"]))]),
        "statusMap",
    );
    assert_eq!(statuses.len(), 2, "an internal user sees both");
    tape.clear();
    login("xtin");
    ok("heartbeat#UserStatus", &[("idleSeconds", int(0))]);
    let status_pushes = tape.take("userStatus");
    assert!(
        status_pushes
            .iter()
            .any(|(_, users, _)| users.contains(&"xtpeer".to_string())),
        "{status_pushes:?}"
    );
    assert!(
        status_pushes
            .iter()
            .all(|(_, users, _)| !users.contains(&"xtout".to_string())),
        "{status_pushes:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
