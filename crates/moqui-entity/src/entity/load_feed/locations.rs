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
//! the data of one component loaded on a running server (plans/Component-Deploy.md).

use std::path::Path;

/// The data files of the component `name` at `location`, in load order: with `seed`, `component://name/entity/file`
/// for each entity definition file (their `seed-data` blocks; not `*.eecas.xml`), then the files of its `data`
/// directory ([`data_directory_locations`]).
#[must_use]
pub fn component_data_locations(name: &str, location: &Path, seed: bool) -> Vec<String> {
    let mut locations = Vec::new();
    if seed {
        if let Ok(read) = std::fs::read_dir(location.join("entity")) {
            let mut names: Vec<String> = read
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|file| file.ends_with(".xml") && !file.ends_with(".eecas.xml"))
                .collect();
            names.sort();
            locations.extend(
                names
                    .into_iter()
                    .map(|file| format!("component://{name}/entity/{file}")),
            );
        }
    }
    locations.extend(data_directory_locations(name, &location.join("data")));
    locations
}

/// Each `.xml`, `.csv` and `.json` file in `data_dir`, then those in each directory directly under it, by file name
/// within each directory. A file in `data/es/` is `component://name/data/es/file`. Deeper directories are not walked.
#[must_use]
pub fn data_directory_locations(component_name: &str, data_dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(data_dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if entry.path().is_file() && is_data_file(&name) {
            files.push(name);
        } else if entry.path().is_dir() {
            directories.push(name);
        }
    }
    files.sort();
    directories.sort();
    let mut locations: Vec<String> = files
        .into_iter()
        .map(|name| format!("component://{component_name}/data/{name}"))
        .collect();
    for directory in directories {
        let Ok(read) = std::fs::read_dir(data_dir.join(&directory)) else {
            continue;
        };
        let mut nested: Vec<String> = read
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| is_data_file(name))
            .collect();
        nested.sort();
        locations.extend(
            nested
                .into_iter()
                .map(|name| format!("component://{component_name}/data/{directory}/{name}")),
        );
    }
    locations
}

/// The data types the data file at `path` declares, as the loader reads them: an XML file's `entity-facade-xml`
/// `type` (or `seed` for the `seed-data` of an entity file), a CSV file's second value on its entity line, a JSON
/// file's `_dataType`. Empty when it declares none, so it loads whatever types are asked for; an XML file that does not
/// parse declares none here and fails when it loads.
#[must_use]
pub fn data_types_of(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
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
