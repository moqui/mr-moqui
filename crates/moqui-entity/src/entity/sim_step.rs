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

//! Steps of a sim run, each under its own savepoint (`org.moqui.impl.SimServices.run#Steps`, docs/ai/Assist.md).
//!
//! Each sim write otherwise commits on its own overlay connection, so a rolled-back transaction cannot undo it and ends
//! the sim ([`super::cache::sim_poisoned`]). [`hold`] keeps one overlay transaction for the sim instead: every overlay
//! read and write of this thread then runs in it. A [`SimSavepoint`] marks the start of a step; when the step fails
//! (an error, a refused outside effect, a transaction it rolled back), [`SimSavepoint::rollback`] undoes its rows and
//! puts back the sim session's state, and the sim goes on with the next step.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use moqui_lib::error::Error;

use super::cache::{self, SimState};
use super::facade::EntityFacade;
use crate::transaction::TransactionManager;

/// How long the held overlay transaction may stay open: a run of many steps, each calling a service.
const HOLD_TIMEOUT: Duration = Duration::from_secs(600);

static SAVEPOINT_SEQ: AtomicU64 = AtomicU64::new(1);

/// The overlay transaction held for this thread's sim until [`SimHold::end`]. Dropped without `end`, it rolls back
/// and ends the sim, whose state then no longer matches its tables.
#[must_use = "the overlay transaction is held until end"]
pub struct SimHold {
    sim: TransactionManager,
    began: bool,
    ended: bool,
}

/// Hold one overlay transaction for this thread's sim, so steps can roll back to savepoints.
///
/// # Errors
///
/// Fails when no sim session is open on this thread or the overlay connection cannot be had.
pub fn hold(facade: &EntityFacade) -> Result<SimHold, Error> {
    if !cache::sim_entered() {
        return Err(Error::new("no sim session is open"));
    }
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    let began = sim.begin(Some(HOLD_TIMEOUT))?;
    let mut hold = SimHold {
        sim,
        began,
        ended: false,
    };
    // Enlist now, so the first step's savepoint has a transaction to mark.
    if let Err(err) = hold.sim.with_connection(|_| Ok(())) {
        hold.finish(false);
        return Err(err);
    }
    Ok(hold)
}

impl SimHold {
    /// Commit the held transaction, so the sim's tables stay as its session state says, and let go of the
    /// connection. The sim stays open.
    ///
    /// # Errors
    ///
    /// Fails when the commit does; the sim is then poisoned.
    pub fn end(mut self) -> Result<(), Error> {
        self.ended = true;
        let committed = self.sim.commit();
        if committed.is_err() && self.began {
            cache::poison_sim("The overlay transaction of the sim could not be committed. Start a new sim.");
        }
        committed
    }

    fn finish(&mut self, poison: bool) {
        self.ended = true;
        if let Err(err) = self.sim.rollback() {
            tracing::warn!(error = %err, "the held sim transaction did not roll back");
        }
        if poison && self.began {
            cache::poison_sim("The overlay transaction of the sim was rolled back. Start a new sim.");
        }
    }
}

impl Drop for SimHold {
    fn drop(&mut self) {
        if !self.ended {
            self.finish(true);
        }
    }
}

/// A step's start in the held overlay transaction: the savepoint and the sim session's state there.
#[must_use = "a step savepoint is rolled back or released"]
pub struct SimSavepoint {
    sim: TransactionManager,
    name: String,
    state: Option<SimState>,
}

/// Mark the start of a step in the held overlay transaction ([`hold`]).
///
/// # Errors
///
/// Fails when no overlay transaction is held on this thread, or the savepoint cannot be set.
pub fn savepoint(facade: &EntityFacade) -> Result<SimSavepoint, Error> {
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    if !sim.in_transaction() {
        return Err(Error::new("no overlay transaction is held for the sim"));
    }
    let state = cache::sim_state().ok_or_else(|| Error::new("no sim session is open"))?;
    let name = format!("moqui_sim_step_{}", SAVEPOINT_SEQ.fetch_add(1, Ordering::Relaxed));
    sim.savepoint(&name)?;
    cache::note_sim_step(true);
    Ok(SimSavepoint {
        sim,
        name,
        state: Some(state),
    })
}

impl SimSavepoint {
    /// Undo what the step wrote to the sim and put back the sim session's state at the savepoint, a poison the step
    /// caused included.
    ///
    /// # Errors
    ///
    /// Fails when the overlay cannot roll back to the savepoint; the sim is then poisoned, since its rows and its
    /// state no longer agree.
    pub fn rollback(mut self) -> Result<(), Error> {
        self.roll_back()
    }

    /// Keep what the step wrote.
    ///
    /// # Errors
    ///
    /// Fails when the overlay cannot release the savepoint. The step is then rolled back, as a failed step is, so the
    /// steps after it do not run on what it wrote.
    pub fn release(mut self) -> Result<(), Error> {
        if let Err(err) = self.sim.release_savepoint(&self.name) {
            if let Err(undo) = self.roll_back() {
                tracing::warn!(error = %undo, "a sim step that did not release did not roll back either");
            }
            return Err(err);
        }
        self.state = None;
        cache::note_sim_step(false);
        Ok(())
    }

    fn roll_back(&mut self) -> Result<(), Error> {
        let Some(state) = self.state.take() else {
            return Ok(());
        };
        cache::note_sim_step(false);
        match self.sim.rollback_to_savepoint(&self.name) {
            Ok(()) => {
                cache::restore_sim_state(state);
                if let Err(err) = self.sim.release_savepoint(&self.name) {
                    tracing::debug!(error = %err, "a rolled-back sim step savepoint was not released");
                }
                Ok(())
            }
            Err(err) => {
                cache::poison_sim(&format!("The sim could not roll back a step ({err}). Start a new sim."));
                Err(err)
            }
        }
    }
}

impl Drop for SimSavepoint {
    fn drop(&mut self) {
        if let Err(err) = self.roll_back() {
            tracing::warn!(error = %err, "a sim step savepoint did not roll back");
        }
    }
}
