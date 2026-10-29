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

//! Zip data files, read with the `zip` crate: each XML, CSV, JSON, or nested zip entry is loaded in turn. An XML, CSV,
//! or JSON entry streams from the archive as it inflates, so its size is bounded only as a plain file's is.

use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;

use moqui_lib::error::Error;

use super::csv::load_csv_reader;
use super::json::load_json_reader;
use super::loader::extension;
use super::xml::load_xml_reader;
use super::LoadOptions;
use crate::entity::facade::EntityFacade;

/// Largest nested zip entry. It is read into memory, because a zip is read from its end.
const MAX_NESTED_ZIP: u64 = 256 * 1024 * 1024;
/// Deepest zip-in-zip nesting.
const MAX_ZIP_DEPTH: u32 = 4;

/// Load each entry of the zip `reader`, in archive order. `depth` is how many zips enclose this one.
pub(super) fn load_zip_reader<R: Read + Seek>(
    facade: &EntityFacade,
    location: &str,
    reader: R,
    options: &LoadOptions,
    depth: u32,
) -> Result<u64, Error> {
    if depth > MAX_ZIP_DEPTH {
        return Err(Error::new(format!("zip nesting is too deep at {location}")));
    }
    let mut archive = ::zip::ZipArchive::new(reader)
        .map_err(|err| Error::new(format!("could not read zip {location}")).with_source(err))?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| Error::new(format!("could not read entry {index} of zip {location}")).with_source(err))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|err| {
                Error::new(format!("could not read the name of entry {index} of zip {location}")).with_source(err)
            })?
            .into_owned();
        if unsafe_zip_name(&name) {
            return Err(Error::new(format!("zip entry {name} is not a safe path")));
        }
        let started = std::time::Instant::now();
        let records = load_entry(facade, location, &name, entry, options, depth)?;
        if !matches!(extension(Path::new(&name)).as_str(), "zip") {
            tracing::info!(
                "Loaded {records} records from {name} in zip file {location} in {:.2}s",
                started.elapsed().as_secs_f64()
            );
        }
        total = total.saturating_add(records);
    }
    Ok(total)
}

fn load_entry(
    facade: &EntityFacade,
    location: &str,
    name: &str,
    entry: impl Read,
    options: &LoadOptions,
    depth: u32,
) -> Result<u64, Error> {
    let entry_location = format!("{location}!{name}");
    match extension(Path::new(name)).as_str() {
        "xml" => load_xml_reader(facade, &entry_location, BufReader::new(entry), options),
        "csv" => load_csv_reader(facade, &entry_location, entry, options),
        "json" => load_json_reader(facade, &entry_location, BufReader::new(entry), options),
        "zip" => {
            let mut bytes = Vec::new();
            entry
                .take(MAX_NESTED_ZIP + 1)
                .read_to_end(&mut bytes)
                .map_err(|err| Error::new(format!("could not read {entry_location}")).with_source(err))?;
            if bytes.len() as u64 > MAX_NESTED_ZIP {
                return Err(Error::new(format!("{entry_location} is too large to load")));
            }
            load_zip_reader(facade, &entry_location, Cursor::new(bytes), options, depth + 1)
        }
        _ => Ok(0),
    }
}

fn unsafe_zip_name(name: &str) -> bool {
    name.starts_with('/') || name.starts_with('\\') || name.split(['/', '\\']).any(|part| part == "..")
}
