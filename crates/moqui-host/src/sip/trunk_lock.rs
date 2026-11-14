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

//! Which member of an edge cluster registers the trunks (plans/Edge-Node2.md §4.6, "By area", SIP). Every member runs
//! a SIP engine and loads every trunk, so each can place a call out and take one a trunk sends it; but a provider sends
//! its calls to the contact that registered, and two members registering one account would split its calls or replace
//! each other's binding. So on a node that takes links only the holder of the lock row `moqui.edge.EdgeSenderLock`
//! `sipTrunks` (`moqui_core::edge::sip::TRUNK_LOCK`) registers; the others ping the trunks (OPTIONS) as for one that does not register. The lock is claimed
//! and renewed as the replication sender's is (`moqui_core::edge::replica::sender`, the same timing): a 30-second lease
//! by the database's clock, renewed every 10 seconds, asked for every 3 by the others, taken over when it lapses, let
//! go when the engine stops. A holder whose claims fail stops registering once two thirds of its lease have passed
//! since the last one that worked, so two members do not register at once. A change reloads the trunks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::replica::sender;
use moqui_core::edge::sip::{claim_trunk_lock, release_trunk_lock};
use tokio::sync::mpsc::UnboundedSender;

use super::Command;

/// Whether a trunk registers: as its row says, and on a node that takes links only while this member holds the lock.
pub(super) fn registers(register: bool, links: bool, holds: bool) -> bool {
    register && (!links || holds)
}

/// Whether this member holds the lock after a claim: as the claim said; when it did not go through, still while it
/// held it and less than two thirds of `lease` passed since the last claim that did (`since_good`, from before that
/// claim was asked), so it stops registering before another member can take the lease over.
pub(super) fn holds_after(claimed: Option<bool>, held: bool, since_good: Duration, lease: Duration) -> bool {
    match claimed {
        Some(holds) => holds,
        None => held && since_good < lease * 2 / 3,
    }
}

/// The member id this engine claims the lock as: its cluster instance id, else `local` (a node with no cluster, which
/// always gets it).
pub(super) fn member_id(factory: &ExecutionContextFactory) -> String {
    crate::rtc::own_node(factory)
}

/// One claim or renewal: whether this member holds the lock now. `None` when the claim did not go through.
pub(super) async fn claim(factory: &Arc<ExecutionContextFactory>, member: &str) -> Option<bool> {
    let member = member.to_string();
    let lease = sender::timing(factory).lease;
    let claimed = crate::worker::on_pool(&Arc::downgrade(factory), move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        claim_trunk_lock(&facade, &member, lease).map_err(|err| moqui_core::context::Error::new(err.message()))
    })
    .await;
    match claimed {
        Ok(holds) => Some(holds),
        Err(err) => {
            tracing::warn!("SIP: the trunk registration lock was not claimed: {}", err.message());
            None
        }
    }
}

/// Keep claiming the lock while the engine runs, recording whether this member holds it in `holds` and telling the
/// engine to reload its trunks when that changes. Ends when the factory or the engine is gone.
pub(super) async fn keep(
    factory: Weak<ExecutionContextFactory>,
    member: String,
    holds: Arc<AtomicBool>,
    commands: UnboundedSender<Command>,
) {
    let mut last_good = Instant::now();
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let timing = sender::timing(&strong);
        // taken before the claim: its lease runs from no later than this
        let asked = Instant::now();
        let claimed = claim(&strong, &member).await;
        drop(strong);
        if claimed.is_some() {
            last_good = asked;
        }
        let now_holds = holds_after(
            claimed,
            holds.load(Ordering::Acquire),
            last_good.elapsed(),
            timing.lease,
        );
        if holds.swap(now_holds, Ordering::AcqRel) != now_holds {
            if now_holds {
                tracing::info!("SIP: this member registers the trunks now");
            } else {
                tracing::info!("SIP: another member registers the trunks now");
            }
            if commands
                .send(Command::Control(serde_json::json!({"action": "reload"})))
                .is_err()
            {
                return;
            }
        }
        let wait = if now_holds { timing.renew } else { timing.poll };
        tokio::time::sleep(wait.max(Duration::from_millis(50))).await;
    }
}

/// Let the lock go when this member holds it, as the engine stops. A failure is logged: the lease lapses on its own.
pub(super) fn release(factory: &Arc<ExecutionContextFactory>, member: &str) {
    let Some(facade) = factory.entity() else { return };
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    match release_trunk_lock(&facade, member) {
        Ok(true) => tracing::info!("SIP: this member let the trunk registration lock go"),
        Ok(false) => {}
        Err(err) => tracing::warn!("SIP: the trunk registration lock was not let go ({err}); its lease lapses"),
    }
}

#[cfg(test)]
#[path = "trunk_lock_tests.rs"]
mod tests;
