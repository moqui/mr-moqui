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

fn is_data_file(name: &str) -> bool {
    [".xml", ".csv", ".json"].iter().any(|ext| name.ends_with(ext))
}
