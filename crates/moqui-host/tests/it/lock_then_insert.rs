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

//! Services that read a row, find none and create it, called at once on embedded Postgres: a lock on a missing row
//! locks nothing there, and a failed insert aborts the whole transaction, so each locks a row that is always there
//! before it looks again. Several joins of a room by one user at once (`ChatServices.add#Member`, which locks the
//! `ChatRoom`) all succeed with one member row and one membership event; several first saves of one wiki page
//! (`WikiServices.update#WikiPage`, which locks the `WikiSpace`) all succeed with one `WikiPage` row.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::{to_plain_string, Value};

/// Racing rounds; each starts with the row missing.
const ROUNDS: usize = 5;
/// The calls of a round.
const CALLS: usize = 6;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="RACE_1" username="race.1" userFullName="Race One"/>
    <moqui.security.UserAccount userId="RACE_2" username="race.2" userFullName="Race Two"/>
    <moqui.security.UserAccount userId="RACE_3" username="race.3" userFullName="Race Three"/>
    <moqui.security.UserAccount userId="RACE_4" username="race.4" userFullName="Race Four"/>
    <moqui.security.UserAccount userId="RACE_5" username="race.5" userFullName="Race Five"/>
</entity-facade-xml>"#;

#[test]
fn racing_first_joins_and_first_page_saves_all_succeed() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_data("lock-then-insert", DATA);
    let factory = Arc::clone(gate.server.factory());

    // a public room of john.doe's, and a wiki space
    let room_id = as_user(&factory, "john.doe", || {
        let room = call(
            "org.moqui.impl.ChatServices.create#Room",
            &[
                ("roomTypeEnumId", Value::string("public")),
                ("roomName", Value::string("Race")),
            ],
        )
        .unwrap();
        call(
            "org.moqui.impl.WikiServices.create#WikiSpace",
            &[
                ("wikiSpaceId", Value::string("RACE")),
                ("description", Value::string("Racing saves")),
            ],
        )
        .unwrap();
        to_plain_string(room.get("chatRoomId").unwrap())
    });

    // the joins of a user not yet in the room
    for round in 0..ROUNDS {
        let username = format!("race.{}", round + 1);
        let user_id = format!("RACE_{}", round + 1);
        let room = room_id.clone();
        let results = race(&factory, &username, move |_| {
            call(
                "org.moqui.impl.ChatServices.add#Member",
                &[("chatRoomId", Value::string(room.clone()))],
            )
        });
        for (index, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "round {round}: join {index}: {result:?}");
        }
        let members = count(
            &factory,
            "moqui.chat.ChatRoomMember",
            &[("chatRoomId", &room_id), ("userId", &user_id)],
        );
        assert_eq!(members, 1, "round {round}: one member row");
        let joins = count(
            &factory,
            "moqui.chat.ChatEvent",
            &[
                ("chatRoomId", &room_id),
                ("eventTypeEnumId", "CetMembership"),
                ("senderUserId", &user_id),
            ],
        );
        assert_eq!(joins, 1, "round {round}: one membership event");
    }

    // the first saves of a page not yet in the space
    for round in 0..ROUNDS {
        let page_name = format!("Page{round}");
        let name = page_name.clone();
        let results = race(&factory, "john.doe", move |index| {
            call(
                "org.moqui.impl.WikiServices.update#WikiPage",
                &[
                    ("wikiSpaceId", Value::string("RACE")),
                    ("pageName", Value::string(name.clone())),
                    ("pageText", Value::string(format!("save {index}"))),
                ],
            )
        });
        for (index, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "round {round}: save {index}: {result:?}");
        }
        let pages = count(
            &factory,
            "moqui.resource.wiki.WikiPage",
            &[("wikiSpaceId", "RACE"), ("pagePath", &page_name)],
        );
        assert_eq!(pages, 1, "round {round}: one page row");
    }
}

/// [`CALLS`] calls of `body` at once, each on a thread of its own as `username`, in a transaction of its own.
fn race<F>(factory: &Arc<ExecutionContextFactory>, username: &str, body: F) -> Vec<Result<(), String>>
where
    F: Fn(usize) -> Result<BTreeMap<String, Value>, String> + Send + Sync + 'static + Clone,
{
    let barrier = Arc::new(Barrier::new(CALLS));
    let threads: Vec<_> = (0..CALLS)
        .map(|index| {
            let factory = Arc::clone(factory);
            let barrier = Arc::clone(&barrier);
            let username = username.to_string();
            let body = body.clone();
            std::thread::spawn(move || {
                as_user(&factory, &username, || {
                    barrier.wait();
                    body(index).map(|_| ())
                })
            })
        })
        .collect();
    threads.into_iter().map(|thread| thread.join().unwrap()).collect()
}

fn as_user<T>(factory: &Arc<ExecutionContextFactory>, username: &str, body: impl FnOnce() -> T) -> T {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user(username).unwrap();
    body()
}

/// The service's results, or its failure or the errors it recorded.
fn call(name: &str, parameters: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, String> {
    let parameters = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).map_err(|err| format!("{name}: {err}"))?;
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let mut message = ec.message();
        let errors = message.errors().to_vec();
        message.clear_errors();
        errors
    })
    .unwrap_or_default();
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(format!("{name}: {errors:?}"))
    }
}

fn count(factory: &Arc<ExecutionContextFactory>, entity: &str, fields: &[(&str, &str)]) -> usize {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut find = factory.entity().unwrap().find(entity).unwrap();
    for (field, value) in fields {
        find = find.eq(field, Value::string(*value));
    }
    find.list().unwrap().len()
}
