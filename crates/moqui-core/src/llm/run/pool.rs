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

//! The agent pool (plan 4.4): a [`RequestPool`] of its own, as the git pool has, so step jobs take no request worker
//! from the screens: the conf's `limits` `max-concurrent-runs` (8) workers and a queue of `run-queue` (64). Each job
//! runs in a fresh execution context. Its workers join the startup check of the connection pool
//! ([`crate::service::check_serve_caps`], which makes room for one): the pool takes as many of its workers as the
//! connections the request and service workers leave ([`crate::service::spare_connections`]), at least one, and says
//! so when that is fewer than the conf asks for.

use std::sync::Arc;

use moqui_lib::error::Error;

use crate::context::ExecutionContextFactory;
use crate::llm::conf::LimitsConf;
use crate::service::RequestPool;

pub struct AgentPool {
    pool: RequestPool,
    workers: usize,
}

impl AgentPool {
    pub(crate) fn start(limits: &LimitsConf, spare: Option<u32>) -> Self {
        let pool = RequestPool::idle();
        let asked = limits.max_concurrent_runs.max(1);
        let fits = spare.map_or(asked, |spare| asked.min(spare.max(1)));
        if fits < asked {
            tracing::warn!(
                "the agent pool runs {fits} of the {asked} concurrent runs llm-facade limits max-concurrent-runs asks for: \
                 the connection pool has room for no more beside the request and service workers"
            );
        }
        let workers = usize::try_from(fits).unwrap_or(8);
        let queue = usize::try_from(limits.run_queue.max(1)).unwrap_or(64);
        if let Err(error) = pool.start(workers, queue) {
            tracing::error!("the agent pool did not start: {error}");
        }
        Self { pool, workers }
    }

    /// The workers it runs.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Step jobs waiting for a worker.
    pub fn queue_depth(&self) -> usize {
        self.pool.queue_depth()
    }
}

/// Queue the run's next step job.
///
/// # Errors
///
/// Returns an error when the pool is full or stopped.
pub(crate) fn submit(factory: &Arc<ExecutionContextFactory>, run_id: &str) -> Result<(), Error> {
    let engine = super::engine(factory);
    let owner = Arc::clone(factory);
    let job_factory = Arc::clone(factory);
    let run_id = run_id.to_string();
    engine
        .pool
        .pool
        .try_submit(owner, move || {
            super::step::run_step(&job_factory, &run_id);
            Ok(())
        })
        .map(|_receiver| ())
        .map_err(|error| Error::new(error.to_string()))
}
