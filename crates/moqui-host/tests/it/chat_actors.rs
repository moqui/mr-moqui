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

//! `ChatServices.migrate#ChatActors` on embedded Postgres (plans/Comms-Framework.md §4.4 "The C1 migration"): a room made
//! by the services is turned back into the shape from before chat actors (the member, mention and vote tables keyed by
//! userId, no actors, no actor columns filled), with a departed member, a deleted account's history, an agent member,
//! reactions, mentions, a poll's votes and unread markers. The migration gives every user an actor, rebuilds the three
//! tables on their new keys with every row, keeps the old rows for a rollback, and leaves the counts as they were; run again
//! it changes nothing.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::{to_plain_string, Value};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="MIG_ANN" username="mig.ann" userFullName="Ann Migrate"/>
    <moqui.security.UserAccount userId="MIG_BEN" username="mig.ben" userFullName="Ben Migrate"/>
    <moqui.security.UserAccount userId="MIG_CAL" username="mig.cal" userFullName="Cal Migrate"/>
</entity-facade-xml>"#;

const CHAT: &str = "org.moqui.impl.ChatServices";

fn as_user<T>(factory: &Arc<ExecutionContextFactory>, username: &str, body: impl FnOnce() -> T) -> T {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user(username).unwrap();
    body()
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let mut message = ec.message();
        let errors = message.errors().to_vec();
        message.clear_errors();
        errors
    })
    .unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn get(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

/// Run SQL statements in one transaction on a reserve connection.
fn sql(factory: &Arc<ExecutionContextFactory>, statements: &[&str]) {
    let facade = factory.entity().unwrap();
    let tx = facade.transactions_for("moqui.chat.ChatRoomMember").unwrap();
    moqui_core::entity::exec::with_reserve_tx(&tx, |conn| {
        for statement in statements {
            conn.execute(statement, &[])?;
        }
        Ok(())
    })
    .unwrap_or_else(|err| panic!("{statements:?}: {err}"));
}

/// The first column of the first row of a query.
fn scalar(factory: &Arc<ExecutionContextFactory>, query: &str) -> String {
    let facade = factory.entity().unwrap();
    let tx = facade.transactions_for("moqui.chat.ChatRoomMember").unwrap();
    let rows = moqui_core::entity::exec::with_reserve_tx(&tx, |conn| conn.query(query, &[])).unwrap();
    rows.first()
        .and_then(|row| row.columns.first())
        .map(|(_, value)| to_plain_string(value))
        .unwrap_or_default()
}

/// The table back in its shape from before chat actors: keyed by `old_key`, no actor in any row.
fn old_shape(factory: &Arc<ExecutionContextFactory>, table: &str, old_key: &str) {
    sql(
        factory,
        &[
            &format!("CREATE TABLE {table}_old AS SELECT * FROM {table}"),
            &format!("UPDATE {table}_old SET chat_actor_id = NULL"),
            &format!("DROP TABLE {table}"),
            &format!("ALTER TABLE {table}_old RENAME TO {table}"),
            &format!("ALTER TABLE {table} ADD PRIMARY KEY ({old_key})"),
        ],
    );
}

fn counts(factory: &Arc<ExecutionContextFactory>, username: &str) -> (String, String) {
    as_user(factory, username, || {
        let out = call(&format!("{CHAT}.get#UnreadCounts"), &[]);
        (get(&out, "unread"), get(&out, "mentions"))
    })
}

#[test]
fn old_chat_rows_move_to_actors_and_keep_their_counts() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_data("chat-actors", DATA);
    let factory = Arc::clone(gate.server.factory());
    let facade = factory.entity().unwrap();

    // a room with mentions, a reaction, a poll's votes and unread markers, made by the services
    let (room, poll) = as_user(&factory, "mig.ann", || {
        let room = get(
            &call(
                &format!("{CHAT}.create#Room"),
                &[
                    ("roomTypeEnumId", text("private")),
                    ("roomName", text("Before actors")),
                    ("memberUserIdList", Value::List(vec![text("MIG_BEN"), text("MIG_CAL")])),
                ],
            ),
            "chatRoomId",
        );
        let first = get(
            &call(
                &format!("{CHAT}.send#Message"),
                &[
                    ("chatRoomId", text(&room)),
                    ("body", text(r#"<p>hi <span data-mention-id="MIG_BEN">@Ben</span></p>"#)),
                ],
            ),
            "eventSeq",
        );
        call(
            &format!("{CHAT}.react#Message"),
            &[
                ("chatRoomId", text(&room)),
                ("eventSeq", Value::I64(first.parse().unwrap())),
                ("key", text("👍")),
            ],
        );
        let poll = get(
            &call(
                &format!("{CHAT}.create#Poll"),
                &[
                    ("chatRoomId", text(&room)),
                    ("question", text("Lunch?")),
                    ("optionList", Value::List(vec![text("Yes"), text("No")])),
                ],
            ),
            "eventSeq",
        );
        call(
            &format!("{CHAT}.vote#Poll"),
            &[
                ("chatRoomId", text(&room)),
                ("eventSeq", Value::I64(poll.parse().unwrap())),
                ("optionIdList", Value::List(vec![Value::I64(1)])),
            ],
        );
        (room, poll)
    });
    as_user(&factory, "mig.cal", || {
        call(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&room))]);
        call(&format!("{CHAT}.remove#Member"), &[("chatRoomId", text(&room))]);
    });
    let before_ben = counts(&factory, "mig.ben");
    assert_eq!(
        before_ben,
        ("2".to_string(), "1".to_string()),
        "two unread, one mention"
    );

    // back to the old shape, with a deleted account's history and an agent member written as an old server wrote them
    old_shape(&factory, "chat_room_member", "chat_room_id, user_id");
    old_shape(&factory, "chat_event_mention", "chat_room_id, event_seq, user_id");
    old_shape(
        &factory,
        "chat_poll_vote",
        "chat_room_id, event_seq, user_id, option_seq",
    );
    let next: i64 = scalar(
        &factory,
        &format!("SELECT last_event_seq FROM chat_room WHERE chat_room_id = '{room}'"),
    )
    .parse()
    .unwrap();
    sql(
        &factory,
        &[
            "UPDATE chat_event SET sender_actor_id = NULL",
            "UPDATE chat_room SET last_message_actor_id = NULL",
            "DELETE FROM chat_actor",
            &format!(
                "INSERT INTO chat_room_member (chat_room_id, user_id, role_enum_id, joined_date, left_date, last_read_event_seq) \
                 VALUES ('{room}', 'MIG_GONE', 'CmrMember', now(), now(), 0)"
            ),
            &format!(
                "INSERT INTO chat_room_member (chat_room_id, user_id, role_enum_id, agent_ref, joined_date, last_read_event_seq) \
                 VALUES ('{room}', 'MIG_AGENT', 'CmrAgent', 'test.Agent.receive#Message', now(), 0)"
            ),
            &format!(
                "INSERT INTO chat_event (chat_room_id, event_seq, event_type_enum_id, sender_user_id, content_json, body_text, sent_date) \
                 VALUES ('{room}', {}, 'CetMessage', 'MIG_GONE', '{{\"body\":\"<p>bye</p>\"}}', 'bye', now())",
                next + 1
            ),
            &format!("UPDATE chat_room SET last_event_seq = {} WHERE chat_room_id = '{room}'", next + 1),
        ],
    );
    let members_before = scalar(&factory, "SELECT COUNT(*) FROM chat_room_member");
    assert!(!facade
        .table_primary_key("moqui.chat.ChatRoomMember")
        .unwrap()
        .contains(&"chat_actor_id".to_string()));

    // the migration
    let out = as_user(&factory, "mig.ann", || call(&format!("{CHAT}.migrate#ChatActors"), &[]));
    assert_eq!(get(&out, "failedCount"), "0", "{out:?}");
    let rebuilt = get(&out, "rebuiltList");
    for entity in ["ChatRoomMember", "ChatEventMention", "ChatPollVote"] {
        assert!(rebuilt.contains(entity), "{entity} rebuilt: {rebuilt}");
        assert!(
            facade
                .table_primary_key(&format!("moqui.chat.{entity}"))
                .unwrap()
                .contains(&"chat_actor_id".to_string()),
            "{entity} keyed by actor"
        );
    }
    assert_eq!(
        scalar(&factory, "SELECT COUNT(*) FROM chat_room_member"),
        members_before
    );
    assert_eq!(
        scalar(&factory, "SELECT COUNT(*) FROM chat_room_member_pre_c1"),
        members_before,
        "the old rows kept"
    );
    assert_eq!(
        scalar(
            &factory,
            "SELECT COUNT(*) FROM chat_room_member WHERE chat_actor_id IS NULL OR chat_actor_id <> user_id"
        ),
        "0"
    );
    assert_eq!(
        scalar(
            &factory,
            "SELECT COUNT(*) FROM chat_event WHERE sender_user_id IS NOT NULL AND sender_actor_id IS NULL"
        ),
        "0"
    );
    assert_eq!(
        scalar(&factory, "SELECT COUNT(*) FROM chat_actor WHERE user_id IN ('MIG_ANN', 'MIG_BEN', 'MIG_CAL', 'MIG_GONE', 'MIG_AGENT')"),
        "5",
        "a deleted account's history keeps its actor"
    );
    assert_eq!(
        scalar(
            &factory,
            "SELECT COUNT(*) FROM chat_poll_vote WHERE chat_actor_id = 'MIG_ANN'"
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &factory,
            "SELECT role_enum_id FROM chat_room_member WHERE user_id = 'MIG_AGENT'"
        ),
        "CmrAgent"
    );
    assert_eq!(
        scalar(
            &factory,
            "SELECT COUNT(*) FROM chat_room_member WHERE user_id = 'MIG_CAL' AND left_date IS NOT NULL"
        ),
        "1",
        "the departed member stays departed"
    );
    // the counts are as they were, plus the deleted account's message
    assert_eq!(counts(&factory, "mig.ben"), ("3".to_string(), "1".to_string()));
    // the rooms work on the new keys
    as_user(&factory, "mig.ben", || {
        call(
            &format!("{CHAT}.vote#Poll"),
            &[
                ("chatRoomId", text(&room)),
                ("eventSeq", Value::I64(poll.parse().unwrap())),
                ("optionIdList", Value::List(vec![Value::I64(2)])),
            ],
        );
        call(&format!("{CHAT}.mark#Read"), &[("chatRoomId", text(&room))]);
    });
    assert_eq!(counts(&factory, "mig.ben"), ("0".to_string(), "0".to_string()));

    // again: nothing to do
    let again = as_user(&factory, "mig.ann", || call(&format!("{CHAT}.migrate#ChatActors"), &[]));
    assert_eq!(get(&again, "failedCount"), "0", "{again:?}");
    assert_eq!(get(&again, "filledCount"), "0");
    assert_eq!(again.get("rebuiltList"), Some(&Value::List(Vec::new())));
}
