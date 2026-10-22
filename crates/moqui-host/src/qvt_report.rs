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

//! `moqui lint --screens --qvt-static`: the compatibility report of `plans/archive/QvtServerStatic.md` for every screen file below the
//! framework and the components. It needs the entity model (forms take their fields from it) and no database, so the entity facade
//! here has a driver that has no connection. A screen is checked as if it were marked `server-static="qvt"`: eligibility, the
//! expressions its static macros write (the `oxc_parser` walk), the work the macros could not write, and the `always-actions` and
//! `pre-actions` that repeat on the form request. The full check, which renders with real actions, is
//! `moqui_core::screen::statics::static_report` with `full` and a database.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::{DbConnection, DbDriver};
use moqui_core::screen::statics::{static_reports_for_files, FileReport};
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use crate::db::entity_facade::{attach, open_entity_facade};

/// A driver for a run that reads the model and never the data.
pub(crate) struct ModelOnly;

impl DbDriver for ModelOnly {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Err(Error::new("this run reads the entity model only and has no database"))
    }
}

/// A factory booted as for a server, with an entity facade over [`ModelOnly`]: the model loads and no data is read. The facade is
/// model-only, so every table counts as absent and a find returns nothing; without that each `SubscreensItem` read fails, no
/// screen definition stays cached, and each `//` link search loads every screen again.
///
/// # Errors
///
/// Returns an error when the factory or the entity model does not load.
pub(crate) fn model_only_factory(runtime: &Path, framework: &Path) -> Result<Arc<ExecutionContextFactory>, Error> {
    let factory =
        ExecutionContextFactory::boot_with_framework(runtime, framework).map_err(|err| Error::new(err.to_string()))?;
    let pool = PoolConfig {
        min: 0,
        max: 1,
        reserve: 0,
        acquire_timeout: std::time::Duration::from_millis(10),
        ..Default::default()
    };
    let facade = open_entity_facade(framework, Arc::new(ModelOnly), pool)?;
    facade.set_model_only();
    attach(&factory, facade);
    Ok(factory)
}

/// Component directories whose screens are scanned. The framework directory is included when it sits outside
/// `base-component` and `component`, so an external `MOQUI_FRAMEWORK` is scanned once along with the runtime components.
pub(crate) fn component_roots(runtime: &Path, framework: &Path) -> Vec<PathBuf> {
    let mut roots = vec![runtime.join("base-component"), runtime.join("component")];
    if !roots.iter().any(|root| framework.starts_with(root)) {
        roots.insert(0, framework.to_path_buf());
    }
    roots.into_iter().filter(|path| path.exists()).collect()
}

/// Screen files below `roots`: `*.xml` in a `screen` directory whose root element is `screen`.
pub(crate) fn screen_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack: Vec<(PathBuf, bool)> = roots.iter().map(|root| (root.clone(), false)).collect();
    while let Some((dir, in_screen)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name.starts_with('.') || name == "node_modules" || name == "build" || name == "libs" {
                    continue;
                }
                stack.push((path, in_screen || name == "screen"));
            } else if in_screen && name.ends_with(".xml") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if MNode::parse_root_only(&path.to_string_lossy(), &text).is_ok_and(|root| root.name() == "screen")
                    {
                        found.push(path);
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The reports of every screen below the roots, and a summary line for each kind of finding.
///
/// # Errors
///
/// Returns an error when the factory or the entity model does not load.
pub fn qvt_static_reports(runtime: &Path, framework: &Path) -> Result<Vec<FileReport>, Error> {
    let factory = model_only_factory(runtime, framework)?;
    let roots = component_roots(runtime, framework);
    let mut files: Vec<String> = screen_files(&roots)
        .into_iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    files.sort();
    files.dedup();
    static_reports_for_files(&factory, &files).map_err(|err| Error::new(err.to_string()))
}

/// The text of the reports: one block for each screen, then the counts.
pub fn render_reports(reports: &[FileReport]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let (mut clean, mut half, mut flagged, mut merged, mut failed) = (0, 0, 0, 0, 0);
    for (location, report) in reports {
        match report {
            Ok(report) => {
                if report.is_clean() {
                    clean += 1;
                } else if report.eligible && report.violations.is_empty() {
                    half += 1;
                } else if report.eligible {
                    flagged += 1;
                } else {
                    merged += 1;
                }
                out.push_str(&report.render());
            }
            Err(err) => {
                failed += 1;
                let _ = writeln!(out, "screen {location}\n  error: {err}");
            }
        }
    }
    let _ = writeln!(
        out,
        "{} screens: {clean} clean, {half} static half only, {flagged} with violations, {merged} stay a server merge, {failed} did not load",
        reports.len()
    );
    out
}
