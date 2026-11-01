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

//! The bridge layer (plans/Comms-Framework.md §7.1 C1) against an in-memory adapter registered only here, with crash and
//! failure injection, on a simulated clock (`run#BridgeDeliveries` at `nowMillis`): a crash before and after the network
//! took a message (sent once to a network that dedupes, uncertain to one that cannot), one failing destination while
//! another delivers, permanent failures and the authorized retry, duplicate batches and echoes, edits that arrive before
//! their message, a whole batch refused for security and a malformed item rejected by name, an unsupported edit refused
//! before the change, and the external actors in the room's reads and counts.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::json;

use super::{
    register_adapter, BridgeInfo, Capabilities, ChatBridgeAdapter, DeliveryKind, DeliveryOutcome, DeliveryRequest,
};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const CHAT: &str = "org.moqui.impl.ChatServices";
const BRIDGES: &str = "org.moqui.impl.ChatBridgeServices";
const MINUTE: i64 = 60_000;

/// What the next delivery of a test adapter does.
#[derive(Clone, Copy, Debug)]
enum Inject {
    /// The process dies before the network has it.
    CrashBefore,
    /// The network takes it, then the process dies before it keeps the outcome.
    CrashAfter,
    /// A failure worth trying again.
    Retry,
    /// A permanent failure.
    Fail,
}

/// One message as the network has it.
#[derive(Clone, Debug)]
struct Sent {
    remote_id: String,
    key: String,
    kind: DeliveryKind,
    text: Option<String>,
    target: Option<String>,
}

#[derive(Default)]
struct Network {
    sent: Vec<Sent>,
    by_key: HashMap<String, String>,
    next: u64,
    script: VecDeque<Inject>,
}

/// An in-memory network: it dedupes by idempotency key when `idempotent`.
struct TestAdapter {
    name: String,
    caps: Capabilities,
    network: Mutex<Network>,
}

impl TestAdapter {
    fn new(name: &str, idempotent: bool, edit: bool) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            caps: Capabilities {
                edit,
                delete: true,
                reaction: true,
                idempotent,
                sends_per_minute: 0,
            },
            network: Mutex::new(Network::default()),
        })
    }

    fn inject(&self, steps: &[Inject]) {
        self.network.lock().unwrap().script.extend(steps.iter().copied());
    }

    fn sent(&self) -> Vec<Sent> {
        self.network.lock().unwrap().sent.clone()
    }
}

impl ChatBridgeAdapter for TestAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self, _bridge: &BridgeInfo) -> Capabilities {
        self.caps.clone()
    }

    fn deliver(&self, _bridge: &BridgeInfo, request: &DeliveryRequest) -> DeliveryOutcome {
        let mut network = self.network.lock().unwrap();
        let step = network.script.pop_front();
        if matches!(step, Some(Inject::CrashBefore)) {
            drop(network);
            panic!("injected crash before the network took it");
        }
        match step {
            Some(Inject::Retry) => {
                return DeliveryOutcome::Retry {
                    error: "the network is away".to_string(),
                    not_connected: true,
                };
            }
            Some(Inject::Fail) => {
                return DeliveryOutcome::Failed {
                    reason_code: "tooLong".to_string(),
                    error: "the network refused it".to_string(),
                };
            }
            _ => {}
        }
        let remote_id = match network.by_key.get(&request.idempotency_key) {
            Some(known) if self.caps.idempotent => known.clone(),
            _ => {
                network.next += 1;
                let id = format!("{}-{}", self.name, network.next);
                network.by_key.insert(request.idempotency_key.clone(), id.clone());
                network.sent.push(Sent {
                    remote_id: id.clone(),
                    key: request.idempotency_key.clone(),
                    kind: request.kind,
                    text: request.text.clone(),
                    target: request.target_remote_id.clone(),
                });
                id
            }
        };
        if matches!(step, Some(Inject::CrashAfter)) {
            drop(network);
            panic!("injected crash after the network took it");
        }
        DeliveryOutcome::Accepted {
            remote_event_id: remote_id,
        }
    }
}

// ---- the bed ----

struct Bed {
    ctx: ExecutionContext,
    facade: EntityFacade,
    a: Arc<TestAdapter>,
    b: Arc<TestAdapter>,
    room: String,
    now: i64,
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
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

/// The errors so far, cleared.
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

/// A room of alice's with bob in it, mapped to two networks: A (dedupes, carries edits as `edit` says) and B (cannot
/// dedupe, no edits), both ways.
fn bed(name: &str, a_edits: bool) -> Bed {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let a = TestAdapter::new(&format!("{name}-a"), true, a_edits);
    let b = TestAdapter::new(&format!("{name}-b"), false, false);
    register_adapter(ctx.factory(), a.clone());
    register_adapter(ctx.factory(), b.clone());
    let entered_ctx = ctx;
    let bed = {
        let _entered = entered_ctx.enter();
        for id in ["alice", "bob", "carol", "dave"] {
            row(
                &facade,
                "moqui.security.UserAccount",
                &[
                    ("userId", text(id)),
                    ("username", text(id)),
                    ("userFullName", text(&id.to_ascii_uppercase())),
                ],
            );
        }
        row(
            &facade,
            "moqui.basic.Enumeration",
            &[
                ("enumId", text("CbtSms")),
                ("enumTypeId", text("ChatBridgeType")),
                ("enumCode", text("sms")),
                ("description", text("SMS")),
            ],
        );
        for (id, adapter, token) in [("BA", &a, "token-a"), ("BB", &b, "token-b")] {
            row(
                &facade,
                "moqui.chat.ChatBridge",
                &[
                    ("chatBridgeId", text(id)),
                    ("typeEnumId", text("CbtSms")),
                    ("adapterName", text(adapter.name())),
                    ("name", text(&format!("Network {id}"))),
                    ("statusId", text("CbsNotConnected")),
                    ("inboundTokenHash", text(&super::sha256_hex(token))),
                ],
            );
        }
        login("alice");
        let room = out_text(
            &ok(
                &format!("{CHAT}.create#Room"),
                &[
                    ("roomTypeEnumId", text("private")),
                    ("roomName", text("Bridged")),
                    ("memberUserIdList", Value::List(vec![text("bob"), text("carol")])),
                ],
            ),
            "chatRoomId",
        );
        for (id, bridge, external) in [("MA", "BA", "+1555000A"), ("MB", "BB", "+1555000B")] {
            row(
                &facade,
                "moqui.chat.ChatRoomBridge",
                &[
                    ("chatRoomBridgeId", text(id)),
                    ("chatRoomId", text(&room)),
                    ("chatBridgeId", text(bridge)),
                    ("externalRoomId", text(external)),
                    ("directionEnumId", text("CrbBoth")),
                    ("fromDate", Value::I64(now())),
                ],
            );
        }
        room
    };
    Bed {
        ctx: entered_ctx,
        facade,
        a,
        b,
        room: bed,
        now: now(),
    }
}

impl Bed {
    fn send(&self, body: &str) -> i64 {
        out_number(
            &ok(
                &format!("{CHAT}.send#Message"),
                &[("chatRoomId", text(&self.room)), ("body", text(body))],
            ),
            "eventSeq",
        )
    }

    /// One delivery round `offset` milliseconds after the bed was made.
    fn round(&self, offset: i64) -> BTreeMap<String, Value> {
        let _authz = self.ctx.disable_authz();
        ok(
            &format!("{BRIDGES}.run#BridgeDeliveries"),
            &[("nowMillis", Value::I64(self.now + offset))],
        )
    }

    /// The delivery of event `seq` on mapping `mapping`: (state, reason, attempts, remote id).
    fn delivery(&self, seq: i64, mapping: &str) -> (String, String, i64, String) {
        let _authz = self.ctx.disable_authz();
        let row = self
            .facade
            .find("moqui.chat.ChatBridgeDelivery")
            .unwrap()
            .eq("chatRoomId", text(&self.room))
            .eq("eventSeq", Value::I64(seq))
            .eq("chatRoomBridgeId", text(mapping))
            .one()
            .unwrap()
            .unwrap_or_else(|| panic!("no delivery of {seq} on {mapping}"));
        let get = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
        (
            get("statusEnumId"),
            get("reasonCode"),
            get("attemptCount").parse().unwrap_or(0),
            get("remoteEventId"),
        )
    }

    fn deliveries_of(&self, seq: i64) -> usize {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatBridgeDelivery")
            .unwrap()
            .eq("chatRoomId", text(&self.room))
            .eq("eventSeq", Value::I64(seq))
            .list()
            .unwrap()
            .values()
            .len()
    }

    fn ingest(
        &self,
        bridge: &str,
        token: &str,
        transaction: &str,
        items: serde_json::Value,
    ) -> BTreeMap<String, Value> {
        call(
            &format!("{BRIDGES}.ingest#Event"),
            &[
                ("chatBridgeId", text(bridge)),
                ("token", text(token)),
                ("transactionId", text(transaction)),
                ("itemList", moqui_lib::json::from_json(&items)),
            ],
        )
    }

    fn count(&self, entity: &str, fields: &[(&str, Value)]) -> i64 {
        let _authz = self.ctx.disable_authz();
        let mut find = self.facade.find(entity).unwrap();
        for (name, value) in fields {
            find = find.eq(name, value.clone());
        }
        find.count().unwrap()
    }

    fn timeline(&self) -> Vec<IndexMap<String, Value>> {
        maps(ok(&format!("{CHAT}.get#Timeline"), &[("chatRoomId", text(&self.room))]).get("eventList"))
    }
}

fn outcomes(out: &BTreeMap<String, Value>) -> Vec<(String, String, String)> {
    maps(out.get("outcomeList"))
        .iter()
        .map(|item| (field(item, "itemId"), field(item, "outcome"), field(item, "reasonCode")))
        .collect()
}

fn quiet_panics() {
    // the injected crashes are panics the worker catches; keep their reports out of the test output
    std::panic::set_hook(Box::new(|info| {
        let text = info.to_string();
        if !text.contains("injected crash") {
            eprintln!("{text}");
        }
    }));
}

// ---- the tests ----

#[test]
fn local_actors_are_their_users() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let users = vec!["ua".to_string(), "ub".to_string()];
    let first = super::local_actors(&facade, &users).unwrap();
    let second = super::local_actors(&facade, &users).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.get("ua").map(String::as_str), Some("ua"));
}

/// A crash before the network took a message and one after: a network that dedupes has each message once; one that
/// cannot tell ends uncertain, with the authorized retry; the other destination is not held up.
#[test]
fn a_crash_before_and_after_remote_acceptance() {
    quiet_panics();
    let bed = bed("crash", true);
    let _entered = bed.ctx.enter();
    login("alice");
    bed.a.inject(&[Inject::CrashBefore]);
    let first = bed.send("<p>first</p>");
    assert_eq!(
        bed.deliveries_of(first),
        2,
        "one delivery row per mapping, in the event's transaction"
    );
    bed.round(0);
    assert_eq!(
        bed.delivery(first, "MA").0,
        "CdsPending",
        "the crash left it pending, leased"
    );
    assert_eq!(
        bed.delivery(first, "MB").0,
        "CdsAccepted",
        "the other network is not held up"
    );
    bed.round(1_000);
    assert_eq!(bed.a.sent().len(), 0, "the lease holds it");
    bed.round(MINUTE + 1_000);
    assert_eq!(bed.delivery(first, "MA").0, "CdsAccepted");
    assert_eq!(bed.a.sent().len(), 1);

    // the network took it, then the crash: sent again with the same key, the network has it once
    bed.a.inject(&[Inject::CrashAfter]);
    bed.b.inject(&[Inject::CrashAfter]);
    let second = bed.send("<p>second</p>");
    bed.round(2 * MINUTE);
    assert_eq!(bed.a.sent().len(), 2);
    assert_eq!(bed.delivery(second, "MA").0, "CdsPending");
    bed.round(4 * MINUTE);
    let (state, _, attempts, remote) = bed.delivery(second, "MA");
    assert_eq!((state.as_str(), attempts), ("CdsAccepted", 2));
    assert_eq!(
        remote,
        bed.a.sent()[1].remote_id,
        "the same message the network took first"
    );
    assert_eq!(bed.a.sent().len(), 2, "no second copy on a network that dedupes");
    assert_eq!(
        bed.a
            .sent()
            .iter()
            .filter(|sent| sent.key == bed.a.sent()[1].key)
            .count(),
        1
    );
    // B cannot dedupe: it is uncertain, not sent again
    let (state, reason, _, _) = bed.delivery(second, "MB");
    assert_eq!((state.as_str(), reason.as_str()), ("CdsUncertain", "lostInFlight"));
    assert_eq!(bed.b.sent().len(), 2, "B has it once, from before the crash");

    // the timeline says so, and the sender may send it again; another member who is not a moderator may not
    let event = bed
        .timeline()
        .into_iter()
        .find(|event| field(event, "eventSeq") == second.to_string())
        .unwrap();
    let delivery = match event.get("delivery") {
        Some(Value::Map(map)) => map.clone(),
        other => panic!("no delivery: {other:?}"),
    };
    assert_eq!(field(&delivery, "state"), "uncertain");
    let destination = maps(delivery.get("destinations"))
        .into_iter()
        .find(|destination| field(destination, "state") == "uncertain")
        .unwrap();
    let id = field(&destination, "chatBridgeDeliveryId");
    login("bob");
    let refused = call(
        &format!("{BRIDGES}.retry#Delivery"),
        &[("chatBridgeDeliveryId", text(&id))],
    );
    assert!(errors().contains("Only the sender or a moderator"));
    assert_eq!(out_text(&refused, "reasonCode"), "notAllowed");
    login("dave");
    let stranger = call(
        &format!("{BRIDGES}.retry#Delivery"),
        &[("chatBridgeDeliveryId", text(&id))],
    );
    assert!(errors().contains("Delivery not found"));
    assert_eq!(out_text(&stranger, "reasonCode"), "notFound");
    login("alice");
    ok(
        &format!("{BRIDGES}.retry#Delivery"),
        &[("chatBridgeDeliveryId", text(&id))],
    );
    bed.round(5 * MINUTE);
    assert_eq!(bed.delivery(second, "MB").0, "CdsAccepted");
    assert_eq!(
        bed.b.sent().len(),
        3,
        "a retry of an uncertain one may send it twice: the user chose"
    );
}

/// One destination failing while the other delivers: its rows wait in order behind the one that failed, back off, and
/// go once it is back; a permanent failure is visible and the next one goes.
#[test]
fn one_failing_destination_does_not_block_another() {
    let bed = bed("failing", true);
    let _entered = bed.ctx.enter();
    login("alice");
    bed.a.inject(&[Inject::Retry, Inject::Retry]);
    let one = bed.send("<p>one</p>");
    let two = bed.send("<p>two</p>");
    bed.round(0);
    assert_eq!(bed.delivery(one, "MB").0, "CdsAccepted");
    assert_eq!(bed.delivery(two, "MB").0, "CdsAccepted", "B delivers both");
    assert_eq!(bed.delivery(one, "MA").0, "CdsPending");
    assert_eq!(
        bed.delivery(two, "MA").2,
        0,
        "A's next waits behind its first, in order"
    );
    assert_eq!(
        bed.count(
            "moqui.chat.ChatBridge",
            &[("chatBridgeId", text("BA")), ("statusId", text("CbsNotConnected"))]
        ),
        1
    );
    bed.round(1_000);
    assert_eq!(
        bed.delivery(one, "MA").2,
        1,
        "backed off: not tried before its next attempt"
    );
    bed.round(6_000);
    assert_eq!(bed.delivery(one, "MA").2, 2);
    bed.round(40_000);
    assert_eq!(bed.delivery(one, "MA").0, "CdsAccepted");
    assert_eq!(bed.delivery(two, "MA").0, "CdsAccepted");
    let texts: Vec<String> = bed.a.sent().iter().filter_map(|sent| sent.text.clone()).collect();
    assert_eq!(texts, vec!["one".to_string(), "two".to_string()], "in order");
    assert_eq!(
        bed.count(
            "moqui.chat.ChatBridge",
            &[("chatBridgeId", text("BA")), ("statusId", text("CbsConnected"))]
        ),
        1
    );

    // a permanent failure is visible, and the next message goes
    bed.a.inject(&[Inject::Fail]);
    let three = bed.send("<p>three</p>");
    let four = bed.send("<p>four</p>");
    bed.round(2 * MINUTE);
    let (state, reason, _, _) = bed.delivery(three, "MA");
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "tooLong"));
    assert_eq!(bed.delivery(four, "MA").0, "CdsAccepted");
    // the admin list shows it, to an administrator only
    for (entity, fields) in [
        ("moqui.security.UserGroup", vec![("userGroupId", text("BRIDGE_ADMINS"))]),
        (
            "moqui.security.UserPermission",
            vec![("userPermissionId", text("ChatBridgeAdmin"))],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", text("BRIDGE_ADMINS")),
                ("userPermissionId", text("ChatBridgeAdmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", text("BRIDGE_ADMINS")),
                ("userId", text("carol")),
                ("fromDate", Value::I64(0)),
            ],
        ),
    ] {
        row(&bed.facade, entity, &fields);
    }
    login("bob");
    let _ = call(&format!("{BRIDGES}.find#Bridges"), &[]);
    assert!(errors().contains("Chat bridges are for administrators"));
    login("carol");
    let admin = ok(&format!("{BRIDGES}.find#Bridges"), &[]);
    let bridge = maps(admin.get("bridgeList"))
        .into_iter()
        .find(|bridge| field(bridge, "chatBridgeId") == "BA")
        .unwrap();
    assert_eq!(field(&bridge, "failedCount"), "1");
    assert_eq!(field(&bridge, "statusId"), "CbsConnected");
    let problem = &maps(admin.get("problemList"))[0];
    assert_eq!(field(problem, "reasonCode"), "tooLong");
    assert_eq!(field(problem, "eventSeq"), three.to_string());
    // an administrator may send it again too
    ok(
        &format!("{BRIDGES}.retry#Delivery"),
        &[("chatBridgeDeliveryId", text(&field(problem, "chatBridgeDeliveryId")))],
    );
    bed.round(3 * MINUTE);
    assert_eq!(bed.delivery(three, "MA").0, "CdsAccepted");
}

/// A batch sent again does nothing; an event seen before is a duplicate; the network's echo of what this server sent
/// changes nothing; what came from one bridge goes to the other and not back.
#[test]
fn duplicate_batches_and_echo_loops_change_nothing() {
    let bed = bed("dupes", true);
    let _entered = bed.ctx.enter();
    login("alice");
    let batch = json!([{"itemId": "i1", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "a-m1",
        "externalUserId": "+15559990", "displayName": "Pat Outside", "text": "hello from outside"}]);
    let first = bed.ingest("BA", "token-a", "T1", batch.clone());
    assert_eq!(errors(), "");
    assert_eq!(outcomes(&first), vec![("i1".into(), "accepted".into(), String::new())]);
    let messages = || {
        bed.count(
            "moqui.chat.ChatEvent",
            &[("chatRoomId", text(&bed.room)), ("eventTypeEnumId", text("CetMessage"))],
        )
    };
    assert_eq!(messages(), 1);
    let again = bed.ingest("BA", "token-a", "T1", batch.clone());
    assert_eq!(out_text(&again, "duplicate"), "true");
    assert_eq!(outcomes(&again), outcomes(&first), "the outcomes it had");
    let other = bed.ingest("BA", "token-a", "T2", batch);
    assert_eq!(
        outcomes(&other)[0].1,
        "duplicate",
        "the same event in another transaction"
    );
    assert_eq!(messages(), 1);

    // the external actor is a member with its network, the sender has its name, alice's count went up
    let event = bed
        .timeline()
        .into_iter()
        .find(|event| field(event, "type") == "message")
        .unwrap();
    assert_eq!(field(&event, "senderName"), "Pat Outside");
    assert_eq!(field(&event, "senderUserId"), "");
    let room = ok(&format!("{CHAT}.get#Room"), &[("chatRoomId", text(&bed.room))]);
    let external = maps(room.get("memberList"))
        .into_iter()
        .find(|member| field(member, "external") == "true")
        .unwrap();
    assert_eq!(field(&external, "name"), "Pat Outside");
    assert_eq!(field(&external, "networkName"), "SMS");
    let counts = ok(&format!("{CHAT}.get#UnreadCounts"), &[]);
    assert_eq!(out_number(&counts, "unread"), 1);

    // it came from A: it goes to B, not back to A
    let seq: i64 = field(&event, "eventSeq").parse().unwrap();
    assert_eq!(bed.deliveries_of(seq), 1);
    bed.round(0);
    assert_eq!(bed.delivery(seq, "MB").0, "CdsAccepted");
    assert_eq!(bed.b.sent()[0].text.as_deref(), Some("hello from outside"));
    // B echoes it back: nothing changes, and nothing goes out again
    let echo = bed.ingest(
        "BB",
        "token-b",
        "B1",
        json!([{"itemId": "e1", "type": "message", "externalRoomId": "+1555000B", "externalEventId": bed.b.sent()[0].remote_id,
            "externalUserId": "bridge-bot", "text": "hello from outside"}]),
    );
    assert_eq!(outcomes(&echo)[0].1, "echo");
    assert_eq!(messages(), 1);
    // a local message goes to both; A's echo of it changes nothing
    let local = bed.send("<p>from alice</p>");
    bed.round(1_000);
    let (_, _, _, remote) = bed.delivery(local, "MA");
    let echo = bed.ingest(
        "BA",
        "token-a",
        "T3",
        json!([{"itemId": "e2", "type": "message", "externalRoomId": "+1555000A", "externalEventId": remote,
            "externalUserId": "+15559990", "text": "from alice"}]),
    );
    assert_eq!(outcomes(&echo)[0].1, "echo");
    assert_eq!(messages(), 2);
}

/// An edit, a reaction and a delete that arrive before their message wait, and are applied, in order, once it comes; an
/// edit older than the one applied is stale; the edit goes on to the other network only if it carries edits.
#[test]
fn reordered_edits_wait_for_their_message() {
    let bed = bed("reorder", true);
    let _entered = bed.ctx.enter();
    login("alice");
    let base = bed.now;
    let early = bed.ingest(
        "BA",
        "token-a",
        "R1",
        json!([
            {"itemId": "edit", "type": "edit", "externalRoomId": "+1555000A", "externalEventId": "a-e1", "externalUserId": "+15551",
                "relatesToExternalEventId": "a-m1", "text": "hello, edited", "originDate": base + 2_000},
            {"itemId": "react", "type": "reaction", "externalRoomId": "+1555000A", "externalEventId": "a-r1", "externalUserId": "+15551",
                "relatesToExternalEventId": "a-m1", "key": "👍"}
        ]),
    );
    assert_eq!(
        outcomes(&early)
            .iter()
            .map(|outcome| outcome.1.clone())
            .collect::<Vec<_>>(),
        vec!["waiting", "waiting"]
    );
    let late = bed.ingest(
        "BA",
        "token-a",
        "R2",
        json!([{"itemId": "m1", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "a-m1",
            "externalUserId": "+15551", "displayName": "Sam", "text": "hello", "originDate": base + 1_000}]),
    );
    assert_eq!(outcomes(&late)[0].1, "accepted");
    let message = bed
        .timeline()
        .into_iter()
        .find(|event| field(event, "type") == "message")
        .unwrap();
    assert_eq!(
        field(&message, "bodyText"),
        "hello, edited",
        "the waiting edit applied after its message"
    );
    assert_eq!(field(&message, "edited"), "true");
    assert_eq!(maps(message.get("reactions")).len(), 1);
    let again = call(
        &format!("{BRIDGES}.get#InboundOutcomes"),
        &[("chatBridgeId", text("BA")), ("transactionId", text("R1"))],
    );
    assert_eq!(
        outcomes(&again)
            .iter()
            .map(|outcome| outcome.1.clone())
            .collect::<Vec<_>>(),
        vec!["accepted", "accepted"]
    );
    // an older edit that comes late changes nothing
    let stale = bed.ingest(
        "BA",
        "token-a",
        "R3",
        json!([{"itemId": "old", "type": "edit", "externalRoomId": "+1555000A", "externalEventId": "a-e0", "externalUserId": "+15551",
            "relatesToExternalEventId": "a-m1", "text": "hello, first edit", "originDate": base + 1_500}]),
    );
    assert_eq!(outcomes(&stale)[0].1, "stale");
    let message = bed
        .timeline()
        .into_iter()
        .find(|event| field(event, "type") == "message")
        .unwrap();
    assert_eq!(field(&message, "bodyText"), "hello, edited");
    // another sender may not edit Sam's message
    let other = bed.ingest(
        "BA",
        "token-a",
        "R4",
        json!([{"itemId": "x", "type": "edit", "externalRoomId": "+1555000A", "externalEventId": "a-e9", "externalUserId": "+15552",
            "relatesToExternalEventId": "a-m1", "text": "not mine"}]),
    );
    assert_eq!(outcomes(&other)[0].2, "notOwn");
    // B got the message; B carries no edits, so the edit fails there as unsupported, never shown as synchronized
    bed.round(0);
    let seq: i64 = field(&message, "eventSeq").parse().unwrap();
    assert_eq!(bed.delivery(seq, "MB").0, "CdsAccepted");
    let edits = bed.count(
        "moqui.chat.ChatBridgeDelivery",
        &[
            ("chatRoomBridgeId", text("MB")),
            ("reasonCode", text("bridgeUnsupported")),
        ],
    );
    assert!(edits >= 1, "the edit is refused for B");
    let reactions: Vec<Sent> = bed
        .b
        .sent()
        .into_iter()
        .filter(|sent| sent.kind == DeliveryKind::Reaction)
        .collect();
    assert_eq!(reactions.len(), 1, "the reaction went to B, about B's copy");
    assert_eq!(reactions[0].target.as_deref(), Some(bed.b.sent()[0].remote_id.as_str()));
}

/// A security failure rejects the whole batch before anything is stored; a malformed item is rejected by its name and
/// the others go on.
#[test]
fn a_security_failure_rejects_the_batch_and_a_malformed_item_only_itself() {
    let bed = bed("security", true);
    let _entered = bed.ctx.enter();
    let good = json!([{"itemId": "ok", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s1",
        "externalUserId": "+15553", "text": "fine"}]);
    let wrong = bed.ingest("BA", "token-b", "S1", good.clone());
    assert!(errors().contains("token does not match"));
    assert_eq!(out_text(&wrong, "reasonCode"), "badToken");
    let local = bed.ingest(
        "BA",
        "token-a",
        "S2",
        json!([{"itemId": "ok", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s2",
            "externalUserId": "+15553", "text": "fine"},
            {"itemId": "spoof", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s3",
            "externalUserId": "+15553", "senderUserId": "alice", "text": "as alice"}]),
    );
    assert!(errors().contains("may not name a local user"));
    assert_eq!(out_text(&local, "reasonCode"), "localIdentity");
    assert_eq!(bed.count("moqui.chat.ChatBridgeInbound", &[]), 0, "nothing stored");
    assert_eq!(
        bed.count(
            "moqui.chat.ChatEvent",
            &[("chatRoomId", text(&bed.room)), ("eventTypeEnumId", text("CetMessage"))]
        ),
        0
    );

    let mixed = bed.ingest(
        "BA",
        "token-a",
        "S3",
        json!([
            {"itemId": "bad", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s4", "text": "no sender"},
            {"itemId": "files", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s5", "externalUserId": "+15553",
                "text": "a file", "attachments": [{"url": "https://example.com/x.png"}]},
            {"itemId": "unmapped", "type": "message", "externalRoomId": "+19990000", "externalEventId": "s6", "externalUserId": "+15553",
                "text": "elsewhere"},
            {"itemId": "ok", "type": "message", "externalRoomId": "+1555000A", "externalEventId": "s7", "externalUserId": "+15553",
                "text": "fine"}
        ]),
    );
    assert_eq!(errors(), "");
    assert_eq!(
        outcomes(&mixed),
        vec![
            ("bad".into(), "rejected".into(), "malformed".into()),
            ("files".into(), "rejected".into(), "attachmentsUnsupported".into()),
            ("unmapped".into(), "rejected".into(), "noMapping".into()),
            ("ok".into(), "accepted".into(), String::new()),
        ]
    );
    assert_eq!(
        bed.count(
            "moqui.chat.ChatEvent",
            &[("chatRoomId", text(&bed.room)), ("eventTypeEnumId", text("CetMessage"))]
        ),
        1
    );
}

/// An edit, delete or reaction a destination cannot carry is refused before the change, with its reason code; the room
/// says what it carries.
#[test]
fn an_unsupported_edit_is_refused_before_the_change() {
    let bed = bed("unsupported", false);
    let _entered = bed.ctx.enter();
    login("alice");
    let seq = bed.send("<p>keep me</p>");
    bed.round(0);
    let out = call(
        &format!("{CHAT}.edit#Message"),
        &[
            ("chatRoomId", text(&bed.room)),
            ("eventSeq", Value::I64(seq)),
            ("body", text("<p>changed</p>")),
        ],
    );
    assert!(errors().contains("Edits do not reach Network"));
    assert_eq!(out_text(&out, "reasonCode"), "bridgeUnsupported");
    assert_eq!(
        bed.count(
            "moqui.chat.ChatEvent",
            &[("chatRoomId", text(&bed.room)), ("eventTypeEnumId", text("CetEdit"))]
        ),
        0
    );
    let room = ok(&format!("{CHAT}.get#Room"), &[("chatRoomId", text(&bed.room))]);
    let room = match room.get("room") {
        Some(Value::Map(map)) => map.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(field(&room, "bridged"), "true");
    let caps = match room.get("bridgeCapabilities") {
        Some(Value::Map(map)) => map.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(field(&caps, "edit"), "false");
    assert_eq!(field(&caps, "reaction"), "true");
    // a reaction both carry goes
    ok(
        &format!("{CHAT}.react#Message"),
        &[
            ("chatRoomId", text(&bed.room)),
            ("eventSeq", Value::I64(seq)),
            ("key", text("👍")),
        ],
    );
}
