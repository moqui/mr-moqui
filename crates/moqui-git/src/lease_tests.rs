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

use super::{take, try_take, TTL};

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
