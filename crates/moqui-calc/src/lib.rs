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

//! `CalcGrid`, database-driven spreadsheets (plans/IronCalc-DB.md): a workbook on the `IronCalc` engine
//! ([`moqui_core::workbook`]) whose data sheets are filled from the data grid's sources, with a saved overlay of
//! formulas and charts on top. The engine and the workbook are in the server; a view gets the cells on its screen.
//!
//! - [`fill`]: the engine work, with no `ExecutionContext`: a data sheet filled with rows, the stored form.
//! - [`log`]: a saved document's edit log, which every room of it follows, and each person's undo and redo.
//! - [`profile`]: the access profiles, which say what a person who is not an administrator may read.
//! - [`room`]: the open documents, each a workbook in memory, read a viewport at a time and dropped when idle.
//! - [`scope`]: the fingerprint of a reader's scope, which decides who shares a room and who may be sent an export.
//! - [`service`]: `org.moqui.calc.CalcServices`, which open, read, refresh, save, export and close a document.
//! - [`together`]: editing together: a document's room for each scope, brought up to its log, and who is where.
//!
//! This crate is on moqui-core, which reaches it through the [`moqui_core::extension`] entry below, and on moqui-tools
//! for the grid's reader. moqui-host links it under its `xlsx` feature; a build without it reports that `xlsx` is not
//! built in.

pub mod charts;
pub mod fill;
pub mod log;
pub mod profile;
pub mod room;
pub mod scope;
pub mod service;
pub mod together;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["calc"],
        register_services: service::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The registry has a function at each `moqui_calc::` location the framework's service files name, and no other.
    #[test]
    fn the_services_register_at_the_locations_the_service_files_name() {
        let mut map = std::collections::BTreeMap::new();
        super::service::register(&mut map).unwrap();
        let registered: Vec<&str> = map.keys().copied().collect();
        let mut files = Vec::new();
        service_files(&moqui_core::context::framework_dir().join("service"), &mut files);
        let mut named: Vec<String> = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            named.extend(
                text.split("location=\"")
                    .skip(1)
                    .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
                    .filter(|location| location.starts_with("moqui_calc::")),
            );
        }
        named.sort_unstable();
        named.dedup();
        assert_eq!(registered, named);
    }

    fn service_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                service_files(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "xml") {
                out.push(path);
            }
        }
    }
}
