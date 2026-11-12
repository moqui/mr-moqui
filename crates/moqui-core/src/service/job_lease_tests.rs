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

//! The job lease's renewal (job_lease.rs); the scheduler's takeover of a lapsed lease is in schedule_tests.rs.

use std::sync::atomic::Ordering;
use std::time::Duration;

use moqui_lib::object::{to_plain_string, Value};

use super::{renew, stamp, LeaseTiming, LocalRuns};
use crate::context::ExecutionContextFactory;

/// plans/archive/High-Level-Review.md HL-026: one renewal extends the leases of this process's runs, a parked one too, but not
/// that of a run past its job timeout and the grace, whose scripts end; a started run whose lock another run holds now
/// is lost, and its scripts end too.
#[test]
fn a_renewal_skips_a_run_past_its_timeout_and_ends_a_run_that_lost_its_lock() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    ctx.factory().extension(|_| LeaseTiming {
        renew_every: Duration::from_secs(60),
        lease: Duration::from_secs(60),
        grace: Duration::ZERO,
    });
    let instance = super::super::jobs::own_instance(ctx.factory());
    let old_lease = 1_700_000_000_000;
    for (job, holder) in [
        ("OVERDUE", "R-OVERDUE"),
        ("ONTIME", "R-ONTIME"),
        ("UNLIMITED", "R-UNLIMITED"),
        ("PARKED", "R-PARKED"),
        ("LOST", "R-ELSEWHERE"),
    ] {
        let mut lock = facade.make_value("moqui.service.job.ServiceJobRunLock").unwrap();
        lock.set("jobName", Value::string(job));
        lock.set("jobRunId", Value::string(holder));
        lock.set("instanceId", Value::string(&instance));
        lock.set("leaseExpireDate", stamp(old_lease));
        lock.create(&facade).unwrap();
    }
    let runs = ctx.factory().extension(|_| LocalRuns::default());
    let ten = Duration::from_secs(10);
    let five = Some(Duration::from_secs(5));
    let overdue = runs.insert_started("R-OVERDUE", "OVERDUE", ten, five);
    let on_time = runs.insert_started("R-ONTIME", "ONTIME", Duration::ZERO, five);
    let unlimited = runs.insert_started("R-UNLIMITED", "UNLIMITED", ten, None);
    let lost = runs.insert_started("R-LOST", "LOST", Duration::ZERO, five);
    let _parked = super::LocalClaim::new(ctx.factory(), "R-PARKED");
    let renewal = renew(ctx.factory()).unwrap();
    assert_eq!(renewal.overdue, ["R-OVERDUE"]);
    assert_eq!(renewal.lost, ["R-LOST"]);
    assert_eq!(renewal.renewed, 3, "on time, unlimited and parked: {renewal:?}");
    assert!(overdue.load(Ordering::SeqCst), "the overdue run's scripts end");
    assert!(lost.load(Ordering::SeqCst), "the lost run's scripts end");
    assert!(!on_time.load(Ordering::SeqCst) && !unlimited.load(Ordering::SeqCst));
    let lease_of = |job: &str| -> i64 {
        let row = facade
            .find("moqui.service.job.ServiceJobRunLock")
            .unwrap()
            .eq("jobName", Value::string(job))
            .one()
            .unwrap()
            .unwrap();
        match row.get("leaseExpireDate") {
            Some(Value::Timestamp(at)) => at.as_millisecond(),
            other => panic!("{job} lease {:?}", other.map(to_plain_string)),
        }
    };
    assert_eq!(lease_of("OVERDUE"), old_lease, "not renewed");
    assert_eq!(lease_of("LOST"), old_lease, "another run's lock is not renewed");
    let now = jiff::Timestamp::now().as_millisecond();
    for job in ["ONTIME", "UNLIMITED", "PARKED"] {
        assert!(lease_of(job) > now, "{job} renewed a lease's time from now");
    }
}
