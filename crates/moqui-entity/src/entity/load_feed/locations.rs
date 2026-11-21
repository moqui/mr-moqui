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

//! The data files a component holds, as `EntityDataLoaderImpl` gathers them: the load command's component files and
//! the data of one component loaded on a running server (plans/archive/Component-Deploy.md).

use std::path::{Path, PathBuf};

use moqui_db::model::entity_definition_paths;
use moqui_lib::vfs;

/// The data files of the component `name` at `location`, in load order: with `seed`, the definition files in `entity`
/// and its immediate subdirectories for their `seed-data` blocks, then the files of its `data` directory
/// ([`data_directory_locations`]).
#[must_use]
pub fn component_data_locations(name: &str, location: &Path, seed: bool) -> Vec<String> {
    let mut locations = Vec::new();
    let entity_dir = location.join("entity");
    if seed && vfs::is_dir(&entity_dir) {
        match entity_definition_paths(&entity_dir) {
            Ok(paths) => locations.extend(paths.iter().filter_map(|path| {
                let file = path.strip_prefix(&entity_dir).ok()?.to_str()?;
                let file = file.replace(std::path::MAIN_SEPARATOR, "/");
                Some(format!("component://{name}/entity/{file}"))
            })),
            Err(err) => tracing::error!(component = name, error = %err, "Could not list entity files for seed data"),
        }
    }
    locations.extend(data_directory_locations(name, &location.join("data")));
    locations
}

/// The XML, CSV and JSON files in each directory, in name order, followed depth first by its sorted subdirectories.
#[must_use]
pub fn data_directory_locations(component_name: &str, data_dir: &Path) -> Vec<String> {
    let mut locations = Vec::new();
    collect_data_files(
        data_dir,
        &format!("component://{component_name}/data"),
        &mut Vec::new(),
        &mut locations,
    );
    locations
}

fn collect_data_files(dir: &Path, location: &str, ancestors: &mut Vec<PathBuf>, locations: &mut Vec<String>) {
    let entries = match vfs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            // Components may have no data directory.
            if !ancestors.is_empty() || err.kind() != std::io::ErrorKind::NotFound {
                tracing::error!(%location, error = %err, "Could not list data directory");
            }
            return;
        }
    };
    // A disk link may lead back to a parent. Embedded directories have no disk path to resolve.
    let identity = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
    if ancestors.contains(&identity) {
        tracing::warn!(%location, "Skipping data directory link to an ancestor");
        return;
    }
    ancestors.push(identity);
    let mut directories = Vec::new();
    // The VFS returns the merged disk and embedded entries in name order.
    for entry in entries {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if entry.is_dir() {
            directories.push(name);
        } else if is_data_file(&name) {
            locations.push(format!("{location}/{name}"));
        }
    }
    for name in directories {
        collect_data_files(&dir.join(&name), &format!("{location}/{name}"), ancestors, locations);
    }
    ancestors.pop();
}

/// The data types the data file at `path` declares, as the loader reads them: an XML file's `entity-facade-xml`
/// `type` (or `seed` for the `seed-data` of an entity file), a CSV file's second value on its entity line, a JSON
/// file's `_dataType`. Empty when it declares none, so it loads whatever types are asked for; an XML file that does not
/// parse declares none here and fails when it loads.
#[must_use]
pub fn data_types_of(path: &Path) -> Vec<String> {
    let Ok(text) = vfs::read_to_string(path) else {
        return Vec::new();
    };
    let name = path.to_string_lossy();
    let mut types: Vec<String> = Vec::new();
    let mut add = |kind: &str| {
        let kind = kind.trim();
        if !kind.is_empty() && !types.iter().any(|known| known == kind) {
            types.push(kind.to_string());
        }
    };
    if name.ends_with(".xml") {
        let Ok(root) = moqui_lib::node::MNode::parse_text(&name, &text) else {
            return Vec::new();
        };
        let mut blocks = vec![root];
        while let Some(node) = blocks.pop() {
            if node.has_name("seed-data") {
                add("seed");
            } else if node.has_name("entity-facade-xml") {
                if let Some(kind) = node.attribute("type") {
                    add(&kind);
                }
            } else {
                blocks.extend(node.children());
            }
        }
    } else if name.ends_with(".csv") {
        let first = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'));
        if let Some(kind) = first.and_then(|line| line.split(',').nth(1)) {
            add(kind.trim_matches('"'));
        }
    } else if name.ends_with(".json") {
        if let Some((_, rest)) = text.split_once("\"_dataType\"") {
            let value = rest
                .trim_start()
                .strip_prefix(':')
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix('"'))
                .and_then(|rest| rest.split_once('"'))
                .map(|(kind, _)| kind);
            if let Some(kind) = value {
                add(kind);
            }
        }
    }
    types
}

fn is_data_file(name: &str) -> bool {
    [".xml", ".csv", ".json"].iter().any(|ext| name.ends_with(ext))
}
