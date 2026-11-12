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

//! Group revocation under load (plans/High-Level-Review.md HL-023), an acceptance test on embedded Postgres. Writer
//! threads end users' `UserGroupMember` rows, each in a transaction of its own, half by setting `thruDate` and half by
//! deleting the row, while reader threads, each a user's session, reload that user's groups over and over. A user's
//! groups are read through the entity's list cache (`cache="true"`) and kept by the session until the authorization
//! generation moves, so a store of rows read before a commit, landing after its eviction, would keep a revoked
//! membership in force. Only what a read that began after a writer's commit returned is checked: such a read's groups
//! and the cached list of the user's memberships no longer hold the revoked group; when no write was under way across
//! the reads, the cached list is the uncached one and the groups are those it opens; and at the end every user's
//! cached list and groups are what the database holds.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{EntityFacade, EntityList};
use moqui_core::transaction::{PoolConfig, TransactionAttribute};
use moqui_lib::object::{to_plain_string, Value};

const MEMBER: &str = "moqui.security.UserGroupMember";
/// Users, one reader thread each.
const USERS: usize = 4;
/// Groups each user starts in, and the memberships the writers end.
const GROUPS: usize = 30;
const WRITERS: usize = 2;
/// A membership's `fromDate`, well before the test.
const FROM: &str = "2026-01-01 00:00:00";

/// What the writers have done, as the readers see it.
#[derive(Default)]
struct Revocations {
    /// Writes begun, and writes whose commit returned. A reader whose reads fell between two equal readings of both
    /// raced no write.
    started: AtomicUsize,
    done: AtomicUsize,
    /// The (user, group) memberships ended, each added once its commit returned.
    ended: Mutex<HashSet<(String, String)>>,
    stop: AtomicBool,
}

#[test]
#[ignore = "acceptance: group revocation under load on embedded Postgres (plans/High-Level-Review.md HL-023)"]
fn revoked_memberships_are_gone_from_every_read_after_the_commit() {
    use moqui_core::driver::DbDriver;
    use moqui_host::db::embedded::EmbeddedCluster;
    use moqui_host::db::entity_facade::open_entity_facade;

    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    let dir = crate::common::temp_dir("moqui-authz-revocation");
    let _guard = crate::common::DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade(&framework(), Arc::clone(&driver), pool()).unwrap();
    let factory = boot(&dir, &facade);

    let revocations = Arc::new(Revocations::default());
    let readers: Vec<_> = (0..USERS)
        .map(|user| {
            let factory = Arc::clone(&factory);
            let facade = facade.clone();
            let revocations = Arc::clone(&revocations);
            std::thread::spawn(move || read_until_stopped(&factory, &facade, &user_id(user), &revocations))
        })
        .collect();
    let writers: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let facade = facade.clone();
            let revocations = Arc::clone(&revocations);
            std::thread::spawn(move || revoke_share(&facade, writer, &revocations))
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    revocations.stop.store(true, Ordering::SeqCst);
    let mut reads = 0;
    let mut quiet = 0;
    for reader in readers {
        let (all, exact) = reader.join().unwrap();
        reads += all;
        quiet += exact;
    }
    assert_eq!(
        revocations.ended.lock().unwrap().len(),
        USERS * GROUPS,
        "every membership was ended"
    );
    assert!(
        reads > USERS * 10,
        "the readers read while the writers wrote: {reads} reads"
    );
    println!(
        "{reads} reads, {quiet} with no write under way, {} memberships ended",
        USERS * GROUPS
    );

    // After the last commit: each user's cached list is the database's, and a session that read before holds no group.
    for user in 0..USERS {
        let user_id = user_id(user);
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::session::set_user(&user_id, &user_id);
        let _authz = context.disable_authz();
        let (cached, fresh) = (
            members(&facade, &user_id, None),
            members(&facade, &user_id, Some(false)),
        );
        assert_eq!(
            rows(&cached),
            rows(&fresh),
            "{user_id}: the cached list after the last commit"
        );
        let groups = moqui_core::user::current_user_groups().unwrap();
        assert_eq!(
            groups.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from(["ALL_USERS".to_string()]),
            "{user_id}"
        );
    }
    drop(factory);
    drop(facade);
    drop(cluster);
}

fn user_id(user: usize) -> String {
    format!("REVOKE_U{user}")
}

fn group_id(group: usize) -> String {
    format!("REVOKE_G{group:02}")
}

fn framework() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: (USERS * 2 + WRITERS + 2) as u32,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

/// A factory over `facade`, booted in `dir`, with the groups and every user in each of them.
fn boot(dir: &Path, facade: &EntityFacade) -> Arc<ExecutionContextFactory> {
    facade
        .check_tables(&[
            "moqui.security.UserGroup",
            MEMBER,
            "moqui.security.ArtifactGroup",
            "moqui.security.ArtifactGroupMember",
            "moqui.security.ArtifactAuthz",
            "moqui.security.ArtifactAuthzFailure",
            "moqui.security.ArtifactTarpit",
            "moqui.security.ArtifactTarpitLock",
            "moqui.security.EntityFilterSet",
            "moqui.security.EntityFilter",
            "moqui.security.ArtifactAuthzFilter",
            // the membership dates are audit-logged: a write takes an EntityAuditLog id from the sequence
            "moqui.entity.EntityAuditLog",
            "moqui.entity.SequenceValueItem",
        ])
        .unwrap();
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, framework()).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    for group in 0..GROUPS {
        let mut row = facade.make_value("moqui.security.UserGroup").unwrap();
        row.set("userGroupId", Value::string(group_id(group)));
        row.create_or_update(facade).unwrap();
        for user in 0..USERS {
            let mut member = facade.make_value(MEMBER).unwrap();
            member.set("userGroupId", Value::string(group_id(group)));
            member.set("userId", Value::string(user_id(user)));
            member.set("fromDate", Value::string(FROM));
            member.create(facade).unwrap();
        }
    }
    factory
}

/// The writer `writer`'s share of the memberships, ended in turn, each in a transaction of its own: a delete for an
/// even group, a `thruDate` a second back for an odd one. A short pause after some leaves the readers quiet moments.
fn revoke_share(facade: &EntityFacade, writer: usize, revocations: &Revocations) {
    let tx = facade.transactions_for(MEMBER).unwrap();
    let mut index = 0_usize;
    for group in 0..GROUPS {
        for user in 0..USERS {
            index += 1;
            if index % WRITERS != writer {
                continue;
            }
            let (user_id, group_id) = (user_id(user), group_id(group));
            revocations.started.fetch_add(1, Ordering::SeqCst);
            tx.run(TransactionAttribute::ForceNew, None, || {
                let mut member = facade.make_value(MEMBER)?;
                member.set("userGroupId", Value::string(&group_id));
                member.set("userId", Value::string(&user_id));
                member.set("fromDate", Value::string(FROM));
                if group % 2 == 0 {
                    member.delete(facade)
                } else {
                    let ended = moqui_core::user::stamp_value(jiff::Timestamp::now().as_millisecond() - 1000);
                    member.set("thruDate", ended);
                    member.update(facade)
                }
            })
            .unwrap_or_else(|err| panic!("ending {user_id} in {group_id}: {err}"));
            revocations.ended.lock().unwrap().insert((user_id, group_id));
            revocations.done.fetch_add(1, Ordering::SeqCst);
            if index.is_multiple_of(7) {
                std::thread::sleep(Duration::from_millis(3));
            }
        }
    }
}

/// `user_id`'s session, reading its groups and its memberships until the writers stop. Out: the reads, and those that
/// raced no write.
fn read_until_stopped(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    user_id: &str,
    revocations: &Revocations,
) -> (usize, usize) {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::session::set_user(user_id, user_id);
    let _authz = context.disable_authz();
    let deadline = Instant::now() + Duration::from_secs(120);
    let (mut reads, mut quiet) = (0, 0);
    while !revocations.stop.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the writers did not finish");
        let started = revocations.started.load(Ordering::SeqCst);
        let done = revocations.done.load(Ordering::SeqCst);
        // Ended before these reads began: their commits returned.
        let ended: Vec<String> = revocations
            .ended
            .lock()
            .unwrap()
            .iter()
            .filter(|(user, _)| user == user_id)
            .map(|(_, group)| group.clone())
            .collect();
        let groups = moqui_core::user::current_user_groups().unwrap();
        let cached = members(facade, user_id, None);
        let fresh = members(facade, user_id, Some(false));
        reads += 1;
        let open_cached = open_groups(&cached);
        for group in &ended {
            assert!(
                !groups.contains(group),
                "{user_id} still in {group}, ended before the read began"
            );
            assert!(
                !open_cached.contains(group),
                "the cached memberships of {user_id} still open {group}, ended before the read began"
            );
        }
        let raced = started != done
            || revocations.started.load(Ordering::SeqCst) != started
            || revocations.done.load(Ordering::SeqCst) != done;
        if !raced {
            quiet += 1;
            assert_eq!(
                rows(&cached),
                rows(&fresh),
                "{user_id}: the cached list with no write under way"
            );
            let mut expected = open_groups(&fresh);
            expected.insert("ALL_USERS".to_string());
            assert_eq!(
                groups.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "{user_id}: the groups with no write under way"
            );
        }
    }
    (reads, quiet)
}

/// `user_id`'s memberships, through the entity's cache (`use_cache` `None`, as the group reload reads them) or not.
fn members(facade: &EntityFacade, user_id: &str, use_cache: Option<bool>) -> EntityList {
    let mut find = facade.find(MEMBER).unwrap().eq("userId", Value::string(user_id));
    if let Some(flag) = use_cache {
        find = find.use_cache(flag);
    }
    find.list().unwrap()
}

/// Each membership as text: group, from and thru.
fn rows(list: &EntityList) -> BTreeSet<(String, String, String)> {
    let text = |row: &moqui_core::entity::EntityValue, field: &str| {
        row.get(field)
            .filter(|value| !value.is_null())
            .map(to_plain_string)
            .unwrap_or_default()
    };
    list.iter()
        .map(|row| (text(row, "userGroupId"), text(row, "fromDate"), text(row, "thruDate")))
        .collect()
}

/// The groups of the memberships open now.
fn open_groups(list: &EntityList) -> BTreeSet<String> {
    let now = jiff::Timestamp::now().as_millisecond();
    list.iter()
        .filter(|row| match row.get("thruDate") {
            None | Some(Value::Null) => true,
            Some(Value::Timestamp(thru)) => thru.as_millisecond() > now,
            Some(_) => false,
        })
        .filter_map(|row| row.get("userGroupId").map(to_plain_string))
        .collect()
}
