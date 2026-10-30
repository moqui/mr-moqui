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

//! Instance draining, the `isolate-instance` recovery action (plans/Monitoring.md, Later: Recovery). A drained
//! instance answers `/health` with 503 and `draining`, so a load balancer stops sending it new traffic, while it goes
//! on serving the requests that reach it, so an operator can still use it. The state is a `moqui.monitor.InstanceDrain`
//! row per instance id (the telemetry `service.instance.id`), a cached entity, so a change made on any server reaches
//! the instance through the cluster's cache invalidation; each server reads its own row at most every few seconds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;

const DRAIN: &str = "moqui.monitor.InstanceDrain";
const RECHECK: Duration = Duration::from_secs(5);

#[derive(Default)]
struct State {
    drained: AtomicBool,
    checked: Mutex<Option<Instant>>,
}

/// This server's instance id, as its telemetry names it.
pub fn instance_id(factory: &ExecutionContextFactory) -> String {
    crate::telemetry::for_factory(factory).resource().instance_id
}

/// Whether this instance is drained. A failed read keeps the last answer.
pub fn is_draining(factory: &ExecutionContextFactory) -> bool {
    let state = factory.extension(|_| State::default());
    {
        let mut checked = lock(&state.checked);
        if checked.is_some_and(|at| at.elapsed() < RECHECK) {
            return state.drained.load(Ordering::Relaxed);
        }
        *checked = Some(Instant::now());
    }
    match read(factory, &instance_id(factory)) {
        Ok(drained) => state.drained.store(drained, Ordering::Relaxed),
        Err(error) => tracing::warn!(%error, "could not read whether this instance is drained"),
    }
    state.drained.load(Ordering::Relaxed)
}

fn read(factory: &ExecutionContextFactory, instance: &str) -> Result<bool, Error> {
    let Some(entity) = factory.entity() else {
        return Ok(false);
    };
    // Framework state, not the caller's data.
    let _authz = crate::context::ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let row = entity
        .find(DRAIN)?
        .eq("instanceId", Value::string(instance))
        .use_cache(true)
        .one()?;
    Ok(row.is_some_and(|row| row.get("drained").map(moqui_lib::object::to_plain_string).as_deref() == Some("Y")))
}

/// Drain an instance or bring it back, with the reason and the recovery action that did it. This server's own answer
/// changes at once; another's within a few seconds of the cache invalidation reaching it.
pub fn set_drained(
    factory: &ExecutionContextFactory,
    instance: &str,
    drained: bool,
    reason: &str,
    action: Option<&str>,
) -> Result<(), Error> {
    let entity = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _authz = crate::context::ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let mut row = entity.make_value(DRAIN)?;
    row.set("instanceId", Value::string(instance));
    row.set("drained", Value::string(if drained { "Y" } else { "N" }));
    row.set("reason", Value::string(reason));
    row.set("actionId", action.map_or(Value::Null, Value::string));
    row.set("changedDate", Value::Timestamp(jiff::Timestamp::now()));
    row.create_or_update(&entity)?;
    if instance == instance_id(factory) {
        let state = factory.extension(|_| State::default());
        state.drained.store(drained, Ordering::Relaxed);
        *lock(&state.checked) = Some(Instant::now());
    }
    tracing::warn!(instance, drained, reason, "instance drain changed");
    Ok(())
}
