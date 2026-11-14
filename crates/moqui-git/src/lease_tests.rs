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

/// A renewal a test holds back, as a database that does not answer would: renewals of the lease named wait so long.
static HOLD: std::sync::Mutex<Option<(&'static str, Duration)>> = std::sync::Mutex::new(None);

pub(super) fn hold_renewal(name: &str) {
    let hold = *moqui_lib::sync::lock(&HOLD);
    if let Some((held, wait)) = hold {
        if held == name {
            std::thread::sleep(wait);
        }
    }
}

/// One renewal the database holds holds no other lease's: each is made on its own, so the other lease stays valid,
/// and the held one is marked lost once it has gone unrenewed for nearly its time.
#[test]
fn a_renewal_held_by_the_database_does_not_lose_the_other_leases() {
    let entity = moqui_core::testing::open();
    let ttl = Duration::from_millis(600);
    *moqui_lib::sync::lock(&HOLD) = Some(("t:held-renewal", Duration::from_secs(3)));
    let held = try_take(&entity, "t:held-renewal", ttl).unwrap().expect("free");
    let quick = try_take(&entity, "t:quick-renewal", ttl).unwrap().expect("free");
    std::thread::sleep(Duration::from_millis(1300));
    let quick_checked = quick.check();
    let held_checked = held.check();
    *moqui_lib::sync::lock(&HOLD) = None;
    quick_checked.expect("renewed beside the held one");
    let lost = held_checked.unwrap_err();
    assert!(lost.message().starts_with("GitLeaseLost: "), "{lost}");
}

/// A lease marked lost is renewed no more and its row is let go, though its holder has not dropped it yet: another
/// member takes it, where a renewal that went on would keep it held for a holder whose writes have stopped.
#[test]
fn a_lost_lease_is_renewed_no_more_and_its_row_is_let_go() {
    let entity = moqui_core::testing::open();
    let ttl = Duration::from_millis(600);
    let lease = try_take(&entity, "t:lost-renewed", ttl).unwrap().expect("free");
    std::thread::sleep(Duration::from_millis(250));
    lease.check().expect("held and renewed");
    // as the renewer or a check finds it: lost
    lease
        .held
        .state
        .valid
        .store(false, std::sync::atomic::Ordering::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !free_elsewhere(&entity, "t:lost-renewed") {
        assert!(
            std::time::Instant::now() < deadline,
            "the row of a lost lease is still held"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(lease.check().is_err());
    drop(lease);
}

/// A lease taken again on its thread after the outer take was marked lost is taken anew in the database: refused
/// while another member holds it, held anew (and checked good) once it is free, instead of sharing the lost state.
#[test]
fn a_reentry_of_a_lost_lease_takes_it_again_in_the_database() {
    let entity = moqui_core::testing::open();
    let outer = try_take(&entity, "t:relost", TTL).unwrap().expect("free");
    update(
        &entity,
        "t:relost",
        vec![("holderId", Value::string("another-member"))],
        Condition::is_not_null("holderId"),
    )
    .unwrap();
    assert!(outer.check().is_err(), "lost to another member");
    assert!(
        try_take(&entity, "t:relost", TTL).unwrap().is_none(),
        "not the lost state shared: another member holds it"
    );
    update(
        &entity,
        "t:relost",
        vec![("holderId", Value::Null), ("leaseUntil", Value::Null)],
        Condition::is_not_null("holderId"),
    )
    .unwrap();
    let again = try_take(&entity, "t:relost", TTL).unwrap().expect("free again");
    again.check().expect("a take of its own");
    // a later re-entry shares the new take, and the outer one stays lost
    drop(take(&entity, "t:relost", TTL, Duration::ZERO).unwrap());
    assert!(outer.check().is_err());
    drop(outer);
    again.check().expect("the outer one let go only what it held");
    assert!(!free_elsewhere(&entity, "t:relost"));
}

/// A read of the database's clock that fails keeps the offset last read and is not kept; a good one is kept, and an
/// answer with no time is this host's clock.
#[test]
fn a_failed_clock_read_keeps_the_last_offset_and_is_not_cached() {
    use super::next_offset;
    assert_eq!(next_offset(Ok(Some(1500)), Some(10)), (1500, true));
    assert_eq!(next_offset(Ok(None), Some(10)), (0, true));
    assert_eq!(next_offset(Err("connection refused"), Some(1500)), (1500, false));
    assert_eq!(next_offset(Err("connection refused"), None), (0, false));
}
