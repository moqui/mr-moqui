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

//! The calls observed on the artifact stack of a dev instance (plans/design-dev-aide.md 7.1): each pair of a frame and
//! the frame pushed on it (a screen, transition, service, entity or REST path), counted per thread, merged each second,
//! and written to `DevArtifactCall` rows each minute, so they survive a restart and join the reference lists. Between
//! writes at most [`MAX_PAIRS`] pairs are held, the longest unseen dropped first. Off, a push costs one atomic load.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::entity::fence;
use crate::transaction::TransactionAttribute;

const CALL: &str = "moqui.devtools.DevArtifactCall";

/// The most pairs held in memory between writes.
pub const MAX_PAIRS: usize = 200_000;

static ENABLED: AtomicBool = AtomicBool::new(false);
static TICK: AtomicU64 = AtomicU64::new(0);

/// Each caller and callee with its count and when it was last seen (a tick).
type Pairs = HashMap<(String, String), (u64, u64)>;

/// The pairs merged from the threads.
static MERGED: Mutex<Option<Pairs>> = Mutex::new(None);

struct Local {
    counts: HashMap<(String, String), u64>,
    merged_at: Instant,
}

thread_local! {
    static LOCAL: RefCell<Local> = RefCell::new(Local { counts: HashMap::new(), merged_at: Instant::now() });
}

/// Whether calls are being counted.
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The index id of a frame on the artifact stack, for the kinds the index has.
#[must_use]
pub fn artifact_id(artifact_type: &str, name: &str) -> Option<String> {
    Some(match artifact_type {
        "AT_XML_SCREEN" => format!("screen:{name}"),
        "AT_XML_SCREEN_TRANS" => {
            let (screen, transition) = name.rsplit_once('/')?;
            format!("transition:{screen}#{transition}")
        }
        "AT_SERVICE" => format!("service:{name}"),
        "AT_ENTITY" => format!("entity:{name}"),
        "AT_REST_PATH" => format!("rest:{name}"),
        _ => return None,
    })
}

/// Count a push of `child` on `parent`. Called from the artifact stack when [`enabled`].
pub fn observe(parent_type: &str, parent_name: &str, child_type: &str, child_name: &str) {
    let (Some(from), Some(to)) = (
        artifact_id(parent_type, parent_name),
        artifact_id(child_type, child_name),
    ) else {
        return;
    };
    if from == to {
        return;
    }
    let merge = LOCAL.with(|local| {
        let mut local = local.borrow_mut();
        *local.counts.entry((from, to)).or_insert(0) += 1;
        local.merged_at.elapsed() >= Duration::from_secs(1)
    });
    if merge {
        merge_local();
    }
}

/// Merge this thread's counts into the shared ones.
pub fn merge_local() {
    let counts = LOCAL.with(|local| {
        let mut local = local.borrow_mut();
        local.merged_at = Instant::now();
        std::mem::take(&mut local.counts)
    });
    if counts.is_empty() {
        return;
    }
    let tick = TICK.load(Ordering::Relaxed);
    let mut merged = MERGED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let merged = merged.get_or_insert_with(HashMap::new);
    for (pair, count) in counts {
        let entry = merged.entry(pair).or_insert((0, tick));
        entry.0 += count;
        entry.1 = tick;
    }
    if merged.len() > MAX_PAIRS {
        // drop the tenth seen longest ago
        let mut ticks: Vec<u64> = merged.values().map(|(_, seen)| *seen).collect();
        ticks.sort_unstable();
        let cut = ticks[ticks.len() / 10];
        merged.retain(|_, (_, seen)| *seen > cut);
    }
}

/// Start counting, and write the counts each minute, once per process.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if ENABLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("artifact-calls".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            TICK.fetch_add(1, Ordering::Relaxed);
            let Some(factory) = weak.upgrade() else {
                ENABLED.store(false, Ordering::SeqCst);
                return;
            };
            if let Err(err) = flush(&factory) {
                tracing::error!("Artifact index: the observed calls were not written: {err}");
            }
        });
    if let Err(err) = spawned {
        ENABLED.store(false, Ordering::SeqCst);
        tracing::error!("Artifact index: could not start counting calls: {err}");
    }
}

/// Write the counts merged so far (this thread's merged first) to the rows, adding to what they hold.
///
/// # Errors
///
/// Returns an error when the rows cannot be written; the counts are then dropped.
pub fn flush(factory: &Arc<ExecutionContextFactory>) -> Result<usize, moqui_lib::error::Error> {
    merge_local();
    let taken = MERGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .unwrap_or_default();
    if taken.is_empty() {
        return Ok(0);
    }
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let facade = factory
        .entity()
        .ok_or_else(|| moqui_lib::error::Error::new("no entity facade"))?;
    let tx = facade.transactions_for(CALL)?;
    let now = crate::user::now_value();
    let write = |from: &str, to: &str, count: u64| -> Result<(), moqui_lib::error::Error> {
        let found = facade
            .find(CALL)?
            .eq("fromArtifactId", Value::string(from))
            .eq("toArtifactId", Value::string(to))
            .one()?;
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        match found {
            Some(mut row) => {
                let before = row
                    .get("callCount")
                    .map(moqui_lib::object::to_plain_string)
                    .and_then(|text| text.parse::<i64>().ok())
                    .unwrap_or(0);
                row.set("callCount", Value::I64(before.saturating_add(count)));
                row.set("lastCallDate", now.clone());
                row.update(&facade)?;
            }
            None => {
                let mut row = facade.make_value(CALL)?;
                row.set("fromArtifactId", Value::string(from));
                row.set("toArtifactId", Value::string(to));
                row.set("callCount", Value::I64(count));
                row.set("lastCallDate", now.clone());
                row.create(&facade)?;
            }
        }
        Ok(())
    };
    let all = tx.run(TransactionAttribute::ForceNew, None, || {
        for ((from, to), (count, _)) in &taken {
            write(from, to, *count)?;
        }
        Ok(())
    });
    if all.is_ok() {
        return Ok(taken.len());
    }
    // a row that fails is that call's alone: the others are written one by one
    let mut written = 0;
    for ((from, to), (count, _)) in &taken {
        match tx.run(TransactionAttribute::ForceNew, None, || write(from, to, *count)) {
            Ok(()) => written += 1,
            Err(err) => tracing::error!("Artifact index: the observed calls of {from} to {to} were not written: {err}"),
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_map_to_index_ids() {
        assert_eq!(
            artifact_id("AT_XML_SCREEN_TRANS", "component://example/screen/Ex.xml/update").as_deref(),
            Some("transition:component://example/screen/Ex.xml#update")
        );
        assert_eq!(
            artifact_id("AT_SERVICE", "a.B.run#Thing").as_deref(),
            Some("service:a.B.run#Thing")
        );
        assert_eq!(artifact_id("AT_OTHER", "x"), None);
    }
}
