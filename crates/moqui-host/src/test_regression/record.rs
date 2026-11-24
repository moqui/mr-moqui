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

//! The recording of one target's run: the database after boot, and for each case what the spec asked of the engine,
//! what it answered, and the rows that changed.

use moqui_core::driver::DbDriver;
use moqui_lib::error::Error;
use serde_json::{json, Value};

use super::db::{self, RowChange, Snapshot};

/// One case of a spec, as one target ran it.
#[derive(Clone, Debug, Default)]
pub(crate) struct CaseRecord {
    pub spec: String,
    pub case: String,
    /// `passed`, `failed: <message>`, `skipped: <reason>`.
    pub outcome: String,
    /// The calls the case made of the engine (`app.service`, `app.record`), in order.
    pub calls: Vec<Value>,
    pub changes: Vec<RowChange>,
    /// The data clock while the case ran.
    pub clock_ms: i64,
}

impl CaseRecord {
    pub(crate) fn id(&self) -> String {
        format!("{} / {}", self.spec, self.case)
    }

    fn to_json(&self) -> Value {
        json!({
            "spec": self.spec, "case": self.case, "outcome": self.outcome, "clockMs": self.clock_ms,
            "calls": self.calls, "changes": self.changes.iter().map(RowChange::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Recording {
    /// `baseline` or `candidate`.
    pub label: String,
    /// The target with its commit.
    pub target: String,
    /// The database after boot, before the first case.
    pub boot: Snapshot,
    pub cases: Vec<CaseRecord>,
    /// What went wrong outside a case: a spec that did not load, an engine that did not stop.
    pub problems: Vec<String>,
}

impl Recording {
    pub(crate) fn to_json(&self) -> Value {
        let tables: serde_json::Map<String, Value> = self
            .boot
            .iter()
            .map(|(table, rows)| (table.clone(), Value::from(rows.len())))
            .collect();
        json!({
            "label": self.label, "target": self.target, "problems": self.problems,
            "bootRowCounts": tables,
            "cases": self.cases.iter().map(CaseRecord::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Takes the snapshots of one target's database and keeps its recording.
pub(crate) struct Recorder {
    driver: Box<dyn DbDriver>,
    last: Snapshot,
    pub(crate) recording: Recording,
}

impl Recorder {
    /// The recorder of a target that has booted: its boot snapshot taken.
    pub(crate) fn new(driver: Box<dyn DbDriver>, label: &str, target: &str) -> Result<Self, Error> {
        let boot = db::snapshot(driver.as_ref())?;
        Ok(Self {
            driver,
            last: boot.clone(),
            recording: Recording {
                label: label.to_string(),
                target: target.to_string(),
                boot,
                ..Recording::default()
            },
        })
    }

    /// The rows changed since the last snapshot.
    pub(crate) fn changes(&mut self) -> Result<Vec<RowChange>, Error> {
        let now = db::snapshot(self.driver.as_ref())?;
        let changes = db::diff(&self.last, &now);
        self.last = now;
        Ok(changes)
    }
}
