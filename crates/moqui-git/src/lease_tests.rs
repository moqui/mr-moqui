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

//! Leases between two holders (two threads, as two members would be), a lapsed one taken over, and one taken again
//! on its own thread.

use std::time::Duration;

use moqui_core::entity::EntityFacade;

use super::{new_holder, take, try_take, update, TTL};
use moqui_core::entity::Condition;
use moqui_lib::object::Value;

fn other_thread<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(body).join().unwrap()
}

fn free_elsewhere(entity: &EntityFacade, name: &'static str) -> bool {
    let other = entity.clone();
    other_thread(move || try_take(&other, name, TTL).unwrap().is_some())
}

#[test]
fn one_holder_at_a_time_and_a_lapsed_lease_is_taken_over() {
    let entity = moqui_core::testing::open();
    let held = try_take(&entity, "t:one", TTL).unwrap().expect("free");
    assert!(!free_elsewhere(&entity, "t:one"), "held");
    // taken again on its own thread: no wait, and the outer one still holds it after
    drop(take(&entity, "t:one", TTL, Duration::ZERO).unwrap());
    assert!(!free_elsewhere(&entity, "t:one"), "the outer one still holds it");
    drop(held);
    assert!(free_elsewhere(&entity, "t:one"), "let go");
    // a holder that stops renewing (a member that died) loses it once it lapses
    let lapsing = entity.clone();
    other_thread(move || {
        std::mem::forget(
            try_take(&lapsing, "t:lapse", Duration::from_millis(1))
                .unwrap()
                .unwrap(),
        )
    });
    std::thread::sleep(Duration::from_millis(20));
    assert!(free_elsewhere(&entity, "t:lapse"), "lapsed");
}

#[test]
fn a_waiting_take_gets_the_lease_when_it_is_let_go() {
    let entity = moqui_core::testing::open();
    let other = entity.clone();
    let (taken, wait_taken) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let held = try_take(&other, "t:wait", TTL).unwrap().unwrap();
        taken.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
    });
    wait_taken.recv().unwrap();
    let refused = take(&entity, "t:wait", TTL, Duration::from_millis(20)).unwrap_err();
    assert!(refused.message().starts_with("GitLeaseBusy: "), "{refused}");
    let lease = take(&entity, "t:wait", TTL, Duration::from_secs(5)).unwrap();
    holder.join().unwrap();
    drop(lease);
}

/// Fencing: a lease another member took over (its row names another holder) is lost, and a writer's check refuses
/// before it writes; one still held passes the check, which renews it.
#[test]
fn a_lease_taken_over_fails_its_check_and_one_held_passes() {
    let entity = moqui_core::testing::open();
    let lease = try_take(&entity, "t:fence", TTL).unwrap().expect("free");
    lease.check().expect("held");
    update(
        &entity,
        "t:fence",
        vec![("holderId", Value::string("another-member"))],
        Condition::is_not_null("holderId"),
    )
    .unwrap();
    let lost = lease.check().unwrap_err();
    assert!(lost.message().starts_with("GitLeaseLost: "), "{lost}");
    // and stays lost, without asking the database again
    assert!(lease.check().is_err());
    assert!(super::check(None).is_ok(), "no lease to check (a db repository)");
}

/// A lease taken again on its thread shares the outer take: whichever is dropped last lets it go, and one let go on
/// another thread (a transaction's end hook) is not held on this one any more.
#[test]
fn the_last_of_a_lease_and_its_reentries_lets_it_go_on_any_thread() {
    let entity = moqui_core::testing::open();
    let outer = try_take(&entity, "t:share", TTL).unwrap().expect("free");
    let inner = take(&entity, "t:share", TTL, Duration::ZERO).unwrap();
    drop(outer);
    assert!(!free_elsewhere(&entity, "t:share"), "the inner one still holds it");
    drop(inner);
    assert!(free_elsewhere(&entity, "t:share"), "let go with the last");

    let lease = try_take(&entity, "t:moved", TTL).unwrap().expect("free");
    std::thread::spawn(move || drop(lease)).join().unwrap();
    assert!(free_elsewhere(&entity, "t:moved"), "let go on the other thread");
    let again = try_take(&entity, "t:moved", TTL)
        .unwrap()
        .expect("taken anew, not a stale re-entry");
    assert!(!free_elsewhere(&entity, "t:moved"));
    drop(again);
}

/// Holder ids differ on every take and carry 128 random bits beside the process id.
#[test]
fn holder_ids_are_unique_and_random() {
    let (one, two) = (new_holder(), new_holder());
    assert_ne!(one, two);
    let random = one.rsplit('-').next().unwrap();
    assert_eq!(random.len(), 32, "{one}");
    assert!(random.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
