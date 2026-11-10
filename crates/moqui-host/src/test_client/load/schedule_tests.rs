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

//! The schedule on a fake clock: the times are made up, a millisecond a step, and a user's iteration takes a made-up
//! service time.

use super::*;

const TICK: f64 = 0.001;

/// What a fake run did: the lateness of each start run, by when it was taken.
struct Run {
    late: Vec<f64>,
    /// The time each start that ran was taken.
    taken: Vec<f64>,
}

/// `users` users on `schedule`, each iteration taking `service` seconds, from `from` to `until` on a clock that steps
/// a millisecond; `busy` keeps each user's end of its current iteration across calls.
fn drive(schedule: &mut Schedule, busy: &mut [f64], service: f64, from: f64, until: f64) -> Run {
    let mut run = Run {
        late: Vec::new(),
        taken: Vec::new(),
    };
    let mut step = 0u64;
    loop {
        let now = from + step as f64 * TICK;
        if now >= until {
            return run;
        }
        for (user, free_at) in busy.iter_mut().enumerate() {
            if *free_at > now {
                continue;
            }
            if let Take::Go { scheduled } = schedule.take(user as u64, now) {
                run.late.push(now - scheduled.unwrap_or(now));
                run.taken.push(now);
                *free_at = now + service;
            }
        }
        step += 1;
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn starts_are_on_time_while_a_user_is_free() {
    let mut schedule = Schedule::rate(Profile::constant(10.0), 2, Some(1.0), None);
    let mut busy = vec![0.0; 2];
    let run = drive(&mut schedule, &mut busy, 0.05, 0.0, 1.5);
    schedule.settle(1.0);
    assert_eq!(run.late.len(), 10, "{:?}", run.taken);
    assert!(run.late.iter().all(|late| *late < TICK + 1e-9), "{:?}", run.late);
    assert!(close(run.taken[3], 0.3), "{:?}", run.taken);
    assert_eq!(schedule.dropped(), 0);
    assert_eq!(schedule.take(0, 1.2), Take::Done);
    // a total of starts ends the schedule before its time
    let mut counted = Schedule::rate(Profile::constant(100.0), 1, None, Some(3));
    let run = drive(&mut counted, &mut [0.0], 0.001, 0.0, 1.0);
    assert_eq!(run.late.len(), 3);
    assert_eq!(counted.take(0, 1.0), Take::Done);
}

#[test]
fn a_slow_service_makes_starts_late_and_drops_those_past_an_interval() {
    // 10 a second, one user, an iteration of 130 ms: each start is taken late until one is more than 100 ms late
    let mut schedule = Schedule::rate(Profile::constant(10.0), 1, Some(1.0), None);
    let mut busy = vec![0.0];
    let run = drive(&mut schedule, &mut busy, 0.13, 0.0, 1.0);
    let mut drops = schedule.take_drops();
    schedule.settle(1.0);
    drops.extend(schedule.take_drops());
    // the late ones carry their wait: 0, 30, 60, 90 ms, then the 400 ms start is 120 ms late and dropped
    assert!(close(run.late[1], 0.03) && close(run.late[3], 0.09), "{:?}", run.late);
    assert!(run.late.iter().all(|late| *late <= 0.1 + 1e-9), "{:?}", run.late);
    assert!(drops.iter().any(|at| close(*at, 0.4)), "{drops:?}");
    assert_eq!(
        run.late.len() as u64 + schedule.dropped(),
        10,
        "{:?} {drops:?}",
        run.late
    );
    assert_eq!(drops.len() as u64, schedule.dropped());
    assert!(schedule.dropped() >= 2);
    // at a high rate a start may be 25 ms late, not only one interval
    let mut fast = Schedule::rate(Profile::constant(1000.0), 1, None, None);
    assert_eq!(fast.take(0, 0.0), Take::Go { scheduled: Some(0.0) });
    fast.drop_overdue(0.02);
    assert_eq!(fast.dropped(), 0);
    fast.drop_overdue(0.0262);
    assert_eq!(fast.dropped(), 1);
    assert_eq!(fast.take_drops(), vec![0.001]);
}

#[test]
fn stages_ramp_users_and_rates_linearly() {
    // users: 0 to 10 over 10 s, then down to 4 over 2 s
    let stages = [
        Stage {
            duration: 10.0,
            target: 10.0,
        },
        Stage {
            duration: 2.0,
            target: 4.0,
        },
    ];
    let closed = Schedule::closed(Profile::stages(&stages), 10, Some(12.0));
    assert_eq!(closed.threads_wanted(0.5), 0);
    assert_eq!(closed.threads_wanted(1.0), 1);
    assert_eq!(closed.threads_wanted(5.5), 5);
    assert_eq!(closed.target(10.0), Target::Users(10));
    assert_eq!(closed.target(11.0), Target::Users(7));
    assert!(closed.may_want_more(0.0, 0));
    assert!(!closed.may_want_more(10.0, 10));
    let mut closed = closed;
    assert_eq!(closed.take(6, 11.0), Take::Go { scheduled: None });
    // above the target: waits, and after the end is done
    assert_eq!(closed.take(7, 11.0), Take::Wait(11.0 + POLL));
    assert_eq!(closed.take(0, 12.0), Take::Done);
    // a cap below the stages holds the users to it
    let capped = Schedule::closed(Profile::stages(&stages), 3, Some(12.0));
    assert_eq!(capped.threads_wanted(9.0), 3);
    assert!(!capped.may_want_more(5.0, 3));

    // a rate: 0 to 10 a second over 1 s, then 10 for 1 s: 5 starts in the first second and 10 in the next
    let profile = Profile::stages(&[
        Stage {
            duration: 1.0,
            target: 10.0,
        },
        Stage {
            duration: 1.0,
            target: 10.0,
        },
    ]);
    assert!(close(profile.count(1.0), 5.0) && close(profile.count(2.0), 15.0));
    // the second start where 5 t^2 = 1
    assert!(close(profile.time_of_count(1.0).unwrap(), 0.2f64.sqrt()));
    assert!(close(profile.time_of_count(5.0).unwrap(), 1.0));
    assert!(close(profile.time_of_count(6.0).unwrap(), 1.1));
    let mut rate = Schedule::rate(profile, 4, Some(2.0), None);
    let run = drive(&mut rate, &mut [0.0; 4], 0.01, 0.0, 2.5);
    assert_eq!(run.taken.len(), 15, "{:?}", run.taken);
    assert_eq!(run.taken.iter().filter(|at| **at < 1.0).count(), 5);
    assert_eq!(rate.dropped(), 0);
}

#[test]
fn a_live_target_holds_from_its_change() {
    let mut rate = Schedule::rate(Profile::constant(10.0), 2, Some(2.0), None);
    let mut busy = vec![0.0; 2];
    let before = drive(&mut rate, &mut busy, 0.01, 0.0, 0.5);
    assert_eq!(before.taken.len(), 5);
    rate.set_target(0.5, Target::Rate(20.0)).unwrap();
    assert_eq!(rate.target(0.7), Target::Rate(20.0));
    let after = drive(&mut rate, &mut busy, 0.01, 0.5, 1.0);
    // 0.5, 0.55, ... 0.95
    assert_eq!(after.taken.len(), 10, "{:?}", after.taken);
    assert!(close(after.taken[1], 0.55), "{:?}", after.taken);
    // a rate of 0 starts nothing after the start already due at the change, and a rate again places the next start
    // an interval on from the change
    rate.set_target(1.0, Target::Rate(0.0)).unwrap();
    assert_eq!(drive(&mut rate, &mut busy, 0.01, 1.0, 1.5).taken, vec![1.0]);
    rate.set_target(1.5, Target::Rate(10.0)).unwrap();
    let again = drive(&mut rate, &mut busy, 0.01, 1.5, 2.0);
    assert_eq!(again.taken.len(), 4, "{:?}", again.taken);
    assert!(close(again.taken[0], 1.6), "{:?}", again.taken);
    // users of a rate scenario are its most at once: one user left, the other waits
    rate.set_target(2.0, Target::Users(1)).unwrap();
    assert_eq!(rate.cap(), 1);
    assert_eq!(rate.dropped(), 0);

    let mut closed = Schedule::closed(Profile::constant(4.0), 4, None);
    assert_eq!(closed.take(3, 1.0), Take::Go { scheduled: None });
    closed.set_target(1.0, Target::Users(2)).unwrap();
    assert_eq!(closed.take(3, 1.1), Take::Wait(1.1 + POLL));
    assert_eq!(closed.take(1, 1.1), Take::Go { scheduled: None });
    closed.set_target(2.0, Target::Users(6)).unwrap();
    assert_eq!(closed.threads_wanted(2.0), 6);
    assert!(closed.may_want_more(2.0, 4));
    let refused = closed.set_target(2.0, Target::Rate(5.0)).unwrap_err();
    assert_eq!(refused, "it has no rate: give it users");
}
