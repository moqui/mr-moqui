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

//! Where a scheduled report may save its file (plans/archive/Security-Review-Medium.md SEC-TLS-06): under a root the conf's
//! `report` `save-roots` names, never through `..`, never in a component, the conf or a `.env` file. A report's text is
//! its author's, so a write anywhere else could replace a file the server reads.

use super::ExecutionContextFactory;

/// The roots a report file may be saved under when the conf's `report` `save-roots` is empty.
pub const DEFAULT_REPORT_SAVE_ROOTS: &str = "dbresource://report/";

/// The conf's `report` `save-roots`, or [`DEFAULT_REPORT_SAVE_ROOTS`].
pub fn report_save_roots(factory: &ExecutionContextFactory) -> String {
    let roots = factory.conf_setting("report", "save-roots");
    if roots.trim().is_empty() {
        DEFAULT_REPORT_SAVE_ROOTS.to_string()
    } else {
        roots
    }
}

/// Why a report may not be saved at `location`, or `None` when it may: it is under one of `roots` (separated by commas
/// or white space; a root is taken as a directory), has no `..` segment (nor an encoded one or a backslash), is not a
/// `component://` location, and names no `conf` directory and no `.env` file.
pub fn report_save_location_problem(location: &str, roots: &str) -> Option<String> {
    let location = location.trim();
    if location.is_empty() {
        return Some("the save location is empty".to_string());
    }
    let lower = location.to_ascii_lowercase();
    if location.contains('\\') || lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") {
        return Some(format!("the save location {location} has an escaped or backslash path"));
    }
    if lower.starts_with("component:") {
        return Some(format!("a report is not saved in a component: {location}"));
    }
    let path = lower.split_once("://").map_or(lower.as_str(), |(_, rest)| rest);
    let path = path.split_once(':').filter(|(scheme, _)| !scheme.contains('/')).map_or(path, |(_, rest)| rest);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.contains(&"..") {
        return Some(format!("the save location {location} has a .. segment"));
    }
    if segments.iter().any(|segment| *segment == "conf" || *segment == ".env" || segment.ends_with(".env")) {
        return Some(format!("a report is not saved in the conf or a .env file: {location}"));
    }
    let under_root = roots
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .any(|root| {
            let root = if root.ends_with('/') { root.to_string() } else { format!("{root}/") };
            location.starts_with(&root) && location.len() > root.len()
        });
    if under_root {
        None
    } else {
        Some(format!("the save location {location} is not under a report save root ({roots})"))
    }
}

#[cfg(test)]
#[path = "save_location_tests.rs"]
mod tests;
