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

//! The data grid's file source (plans/archive/Tools-Database.md §9.1): CSV, or XLSX, XLSM, XLSB, XLS or ODS through
//! calamine, from a resource location or an upload, read only from the places `etl::access` allows (the caller's own
//! uploads, the import roots, any `dbresource://` for `DATABASE_ADMIN`). A sheet is read whole (bounded by
//! `MAX_SHEET_CELLS`) and kept in a per-node LRU cache keyed by location, last-modified time, sheet and header row, at
//! most [`CACHE_BYTES`] by an estimate of its memory; a CSV is streamed on each call. Columns are named and rows
//! numbered as an ETL run names and numbers them (`moqui_lib::etl::unique_column_names`, the header row a line of the
//! file). [`file_columns`], [`file_rows`], [`file_count`] and [`file_query`] are what `grid.rs` calls for a `type:
//! "file"` source; `clean#GridUploads` removes uploads after seven days and the grid's export files after a day.

use std::collections::{BTreeMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

use crate::etl::access;
use crate::etl::source::{csv_text_bytes, is_csv};
use crate::group_db::{add_error, Out};

/// The most the sheet cache holds, by the size of its cells' text.
pub const CACHE_BYTES: usize = 64 * 1024 * 1024;
/// The rows a column's type is inferred from.
pub const TYPE_SAMPLE_ROWS: usize = 200;
/// Where uploads go: `<root>/<userId>/<uploadId>/<name>`.
pub const UPLOAD_ROOT: &str = access::UPLOAD_ROOT;
/// The largest upload `upload#GridFile` keeps.
pub const MAX_UPLOAD_BYTES: i64 = 100 * 1024 * 1024;
/// The most rows a sorted or filtered file source reads into memory.
pub const MAX_QUERY_ROWS: usize = 200_000;
/// Where `grid.rs` writes its exports.
pub const EXPORT_ROOT: &str = "runtime://tmp/grid-export";
/// Uploads are removed this long after they were written.
pub const UPLOAD_KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Export files are removed this long after they were written.
pub const EXPORT_KEEP_MS: i64 = 24 * 60 * 60 * 1000;

/// A file source: where, which sheet, which row is the header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSpec {
    pub location: String,
    /// Empty for the first sheet; not used for a CSV.
    pub sheet: String,
    /// The header row, from 1; 0 for none (the columns are named by number).
    pub header_row: usize,
    /// `csv`, `xlsx` or `auto` (by the extension).
    pub format: String,
    /// A CSV's delimiter; `,`, or a tab for a `.tsv`.
    pub delimiter: Option<u8>,
}

impl FileSpec {
    /// From a grid source map: `location`, `sheet`, `headerRow` (default 1), `format`, `delimiter`.
    #[must_use]
    pub fn from_map(map: &moqui_lib::object::ValueMap) -> Self {
        let text = |key: &str| map.get(key).map(to_plain_string).unwrap_or_default();
        Self {
            location: text("location"),
            sheet: text("sheet"),
            header_row: text("headerRow").parse().unwrap_or(1),
            format: Some(text("format"))
                .filter(|format| !format.is_empty())
                .unwrap_or_else(|| "auto".to_string()),
            delimiter: text("delimiter").bytes().next(),
        }
    }
}

/// A column of a file source, its type inferred from the first [`TYPE_SAMPLE_ROWS`] rows: `number`, `date`,
/// `date-time`, `boolean` or `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileColumn {
    pub name: String,
    pub title: String,
    pub kind: &'static str,
}

impl FileColumn {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::default();
        map.insert("name".to_string(), Value::string(&self.name));
        map.insert("title".to_string(), Value::string(&self.title));
        map.insert("type".to_string(), Value::string(self.kind));
        map.insert("editable".to_string(), Value::Bool(false));
        Value::Map(map)
    }
}

/// A sheet as the grid reads it: its columns and its rows by column name.
#[derive(Debug, Default)]
pub struct FileTable {
    pub sheet_names: Vec<String>,
    pub sheet_name: String,
    pub columns: Vec<FileColumn>,
    pub rows: Vec<moqui_lib::object::ValueMap>,
    /// The cache's measure of it.
    bytes: usize,
}

type CacheKey = (String, Option<i64>, String, usize);

#[derive(Default)]
struct Cache {
    entries: VecDeque<(CacheKey, Arc<FileTable>)>,
    bytes: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// The last-modified time of a file under the clean roots, as the framework.
fn last_modified(location: &str) -> Option<i64> {
    ExecutionContext::with_current(|ec| ec.factory().resource().stat(location))
        .and_then(Result::ok)
        .flatten()
        .and_then(|stat| stat.last_modified)
}

/// An estimate of the memory a cell of a cached sheet takes: its map entry (the name and the value) and their text.
fn cell_bytes(name: &str, value: &Value) -> usize {
    let heap = match value {
        Value::String(text) => text.len(),
        Value::Null | Value::Bool(_) | Value::I32(_) | Value::I64(_) | Value::F64(_) => 0,
        other => to_plain_string(other).len(),
    };
    // the entry's hash and the key's and the value's own size, then the key's text
    std::mem::size_of::<(u64, String, Value)>() + name.len() + heap
}

/// The type of a column's values: the one kind every non-empty sample has, else `text`.
fn infer_kind<'a>(values: impl Iterator<Item = &'a Value>) -> &'static str {
    let mut kind: Option<&'static str> = None;
    for value in values {
        let this = match value {
            Value::Null => continue,
            Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => "number",
            Value::Timestamp(_) => "date-time",
            Value::Date(_) => "date",
            Value::Bool(_) => "boolean",
            Value::String(text) if text.trim().is_empty() => continue,
            Value::String(text) => {
                let text = text.trim();
                if text.parse::<f64>().is_ok() {
                    "number"
                } else if text.len() <= 10 && text.parse::<jiff::civil::Date>().is_ok() {
                    "date"
                } else if text.parse::<jiff::civil::DateTime>().is_ok() || text.parse::<jiff::Timestamp>().is_ok() {
                    "date-time"
                } else {
                    "text"
                }
            }
            _ => "text",
        };
        kind = match kind {
            None => Some(this),
            Some(known) if known == this => Some(known),
            // dates in a column of date-times read as date-times
            Some("date") if this == "date-time" => Some("date-time"),
            Some("date-time") if this == "date" => Some("date-time"),
            Some(_) => return "text",
        };
    }
    kind.unwrap_or("text")
}

fn columns_of(names: &[String], rows: &[moqui_lib::object::ValueMap]) -> Vec<FileColumn> {
    names
        .iter()
        .map(|name| FileColumn {
            name: name.clone(),
            title: name.clone(),
            kind: infer_kind(rows.iter().take(TYPE_SAMPLE_ROWS).filter_map(|row| row.get(name))),
        })
        .collect()
}

/// Column names from a header's cells: a blank one is named by its number, a repeated one gets `_2`, `_3`, as an ETL
/// run names them.
fn names_of(header: &[String], width: usize) -> Vec<String> {
    moqui_lib::etl::unique_column_names(header, width)
}

fn read_sheet_table(spec: &FileSpec, bytes: &[u8]) -> Result<FileTable, Error> {
    let sheet = moqui_core::service::spreadsheet::read_sheet_bytes(bytes, &spec.sheet, 0)?;
    let header: Vec<String> = sheet
        .rows
        .iter()
        .zip(&sheet.row_numbers)
        .find(|(_, number)| **number as usize == spec.header_row)
        .map(|(cells, _)| cells.iter().map(to_plain_string).collect())
        .unwrap_or_default();
    let width = sheet.rows.iter().map(Vec::len).max().unwrap_or(0);
    let names = names_of(&header, width);
    let mut bytes_used = 0;
    let rows: Vec<moqui_lib::object::ValueMap> = sheet
        .rows
        .into_iter()
        .zip(sheet.row_numbers)
        .filter(|(_, number)| *number as usize > spec.header_row)
        .map(|(cells, _)| {
            names
                .iter()
                .cloned()
                .zip(cells.into_iter().chain(std::iter::repeat(Value::Null)))
                .map(|(name, value)| {
                    bytes_used += cell_bytes(&name, &value);
                    (name, value)
                })
                .collect()
        })
        .collect();
    // each row's map and its slot in the list
    bytes_used += rows.len() * (std::mem::size_of::<moqui_lib::object::ValueMap>() + 16);
    Ok(FileTable {
        sheet_names: sheet.sheet_names,
        sheet_name: sheet.sheet_name,
        columns: columns_of(&names, &rows),
        rows,
        bytes: bytes_used,
    })
}

/// A sheet's table, from the cache when its location, last-modified time, sheet and header row are the same.
fn sheet_table(spec: &FileSpec) -> Result<Arc<FileTable>, Error> {
    access::readable(&spec.location)?;
    let key: CacheKey = (
        spec.location.clone(),
        access::last_modified(&spec.location),
        spec.sheet.clone(),
        spec.header_row,
    );
    {
        let mut cache = lock(cache());
        if let Some(at) = cache
            .entries
            .iter()
            .position(|(known, _)| *known == key && key.1.is_some())
        {
            let entry = cache.entries.remove(at).expect("the position is in the list");
            let table = Arc::clone(&entry.1);
            cache.entries.push_back(entry);
            return Ok(table);
        }
    }
    let bytes = access::read_bytes(&spec.location)?;
    let table = Arc::new(read_sheet_table(spec, &bytes)?);
    if key.1.is_some() && table.bytes <= CACHE_BYTES {
        let mut cache = lock(cache());
        cache.bytes += table.bytes;
        cache.entries.push_back((key, Arc::clone(&table)));
        while cache.bytes > CACHE_BYTES {
            let Some((_, evicted)) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes = cache.bytes.saturating_sub(evicted.bytes);
        }
    }
    Ok(table)
}

/// What [`csv_rows`] read.
struct CsvRead {
    names: Vec<String>,
    rows: Vec<moqui_lib::object::ValueMap>,
    count: usize,
}

/// The rows of a CSV, `offset` and `limit` of the data rows applied as it streams, and how many data rows it has when
/// `count` (read to the end). The header row is a line of the file, as an ETL run counts it (`CsvExtractor`).
fn csv_rows(spec: &FileSpec, offset: usize, limit: Option<usize>, count: bool) -> Result<CsvRead, Error> {
    let bytes = csv_text_bytes(access::read_bytes(&spec.location)?, "");
    let delimiter = spec
        .delimiter
        .unwrap_or(if spec.location.to_ascii_lowercase().ends_with(".tsv") {
            b'\t'
        } else {
            b','
        });
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(Cursor::new(bytes));
    let mut header: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut seen = 0usize;
    let mut record = csv::StringRecord::new();
    let mut line = 0usize;
    let mut names: Vec<String> = Vec::new();
    while reader
        .read_record(&mut record)
        .map_err(|err| Error::new(format!("Could not read {}: {err}", spec.location)))?
    {
        line = record
            .position()
            .and_then(|position| usize::try_from(position.line()).ok())
            .unwrap_or(line + 1);
        if line == spec.header_row {
            header = record.iter().map(str::to_string).collect();
            continue;
        }
        if line < spec.header_row || record.iter().all(str::is_empty) {
            continue;
        }
        if names.len() < record.len() || names.is_empty() {
            names = names_of(&header, record.len().max(names.len()));
        }
        seen += 1;
        if seen <= offset {
            continue;
        }
        if limit.is_some_and(|limit| rows.len() >= limit) {
            if count {
                continue;
            }
            break;
        }
        let row: moqui_lib::object::ValueMap = names
            .iter()
            .cloned()
            .zip(
                record
                    .iter()
                    .map(|cell| {
                        if cell.is_empty() {
                            Value::Null
                        } else {
                            Value::string(cell)
                        }
                    })
                    .chain(std::iter::repeat(Value::Null)),
            )
            .collect();
        rows.push(row);
    }
    if names.is_empty() {
        names = names_of(&header, header.len());
    }
    Ok(CsvRead {
        names,
        rows,
        count: seen,
    })
}

/// What [`file_info`] reads: the columns, the sheet names (empty for a CSV), the first rows and the row count.
pub type FileInfo = (Vec<FileColumn>, Vec<String>, Vec<moqui_lib::object::ValueMap>, usize);

/// A file source's columns, sheet names, first `limit` rows and row count, a CSV read once.
///
/// # Errors
///
/// Returns an error when the file is not there, does not read, or is not in a place the user may read.
pub fn file_info(spec: &FileSpec, limit: usize) -> Result<FileInfo, Error> {
    if is_csv(&spec.format, &spec.location) {
        let read = csv_rows(spec, 0, Some(limit.max(TYPE_SAMPLE_ROWS)), true)?;
        let columns = columns_of(&read.names, &read.rows);
        let mut rows = read.rows;
        rows.truncate(limit);
        return Ok((columns, Vec::new(), rows, read.count));
    }
    let table = sheet_table(spec)?;
    Ok((
        table.columns.clone(),
        table.sheet_names.clone(),
        table.rows.iter().take(limit).cloned().collect(),
        table.rows.len(),
    ))
}

/// The columns of a file source, with their inferred types, and its sheet names (empty for a CSV).
///
/// # Errors
///
/// Returns an error when the file is not there or does not read.
pub fn file_columns(spec: &FileSpec) -> Result<(Vec<FileColumn>, Vec<String>), Error> {
    if is_csv(&spec.format, &spec.location) {
        let read = csv_rows(spec, 0, Some(TYPE_SAMPLE_ROWS), false)?;
        let (names, rows) = (read.names, read.rows);
        return Ok((columns_of(&names, &rows), Vec::new()));
    }
    let table = sheet_table(spec)?;
    Ok((table.columns.clone(), table.sheet_names.clone()))
}

/// The data rows `offset` to `offset + limit` of a file source, by column name.
///
/// # Errors
///
/// Returns an error when the file is not there or does not read.
pub fn file_rows(spec: &FileSpec, offset: usize, limit: usize) -> Result<Vec<moqui_lib::object::ValueMap>, Error> {
    if is_csv(&spec.format, &spec.location) {
        return Ok(csv_rows(spec, offset, Some(limit), false)?.rows);
    }
    let table = sheet_table(spec)?;
    Ok(table.rows.iter().skip(offset).take(limit).cloned().collect())
}

/// How many data rows a file source has.
///
/// # Errors
///
/// Returns an error when the file is not there or does not read.
pub fn file_count(spec: &FileSpec) -> Result<usize, Error> {
    if is_csv(&spec.format, &spec.location) {
        return Ok(csv_rows(spec, 0, Some(0), true)?.count);
    }
    Ok(sheet_table(spec)?.rows.len())
}

/// Whether a cell matches one of the grid's filters: `=`, `!=`, `like` (`%` any text, case ignored), `starts`, `ends`,
/// `<`, `<=`, `>`, `>=` (as numbers when both are, else as text), `in` (a list or comma-separated text), `keywords`
/// (every word, case ignored), `empty` and `!empty` (no text, or some).
fn cell_matches(cell: Option<&Value>, kind: &str, wanted: &Value) -> bool {
    let text = cell.map(to_plain_string).unwrap_or_default();
    let lower = text.to_lowercase();
    let want = to_plain_string(wanted);
    let want_lower = want.to_lowercase();
    let compare = || match (text.trim().parse::<f64>(), want.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => a.partial_cmp(&b),
        _ => Some(text.as_str().cmp(want.as_str())),
    };
    match kind {
        "=" => text == want,
        "!=" => text != want,
        "like" => {
            let parts: Vec<&str> = want_lower.split('%').filter(|part| !part.is_empty()).collect();
            let mut at = 0;
            parts.iter().all(|part| match lower[at..].find(part) {
                Some(found) => {
                    at += found + part.len();
                    true
                }
                None => false,
            })
        }
        "starts" => lower.starts_with(&want_lower),
        "ends" => lower.ends_with(&want_lower),
        "<" => compare() == Some(std::cmp::Ordering::Less),
        "<=" => matches!(compare(), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)),
        ">" => compare() == Some(std::cmp::Ordering::Greater),
        ">=" => matches!(compare(), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)),
        "in" => match wanted {
            Value::List(items) => items.iter().any(|item| to_plain_string(item) == text),
            _ => want.split(',').any(|item| item.trim() == text),
        },
        "keywords" => want_lower.split_whitespace().all(|word| lower.contains(word)),
        "empty" => text.is_empty(),
        "!empty" => !text.is_empty(),
        _ => true,
    }
}

/// The rows of a file source that match `filter` (the grid's `{field, type, value}`), in the order of `sort`: read
/// into memory, at most [`MAX_QUERY_ROWS`] (the second value says when the file had more). A sort compares numbers as
/// numbers and the rest as text.
///
/// # Errors
///
/// Returns an error when the file is not there, does not read, or is not in a place the user may read.
pub fn file_query(
    spec: &FileSpec,
    sort: &[crate::grid::Sort],
    filter: &[crate::grid::Filter],
) -> Result<(Vec<moqui_lib::object::ValueMap>, bool), Error> {
    let (mut rows, truncated) = if is_csv(&spec.format, &spec.location) {
        let read = csv_rows(spec, 0, Some(MAX_QUERY_ROWS), true)?;
        let truncated = read.count > MAX_QUERY_ROWS;
        (read.rows, truncated)
    } else {
        let table = sheet_table(spec)?;
        (
            table.rows.iter().take(MAX_QUERY_ROWS).cloned().collect(),
            table.rows.len() > MAX_QUERY_ROWS,
        )
    };
    rows.retain(|row| {
        filter
            .iter()
            .all(|filter| cell_matches(row.get(&filter.field), &filter.kind, &filter.value))
    });
    if !sort.is_empty() {
        let key = |value: Option<&Value>| {
            let text = value.map(to_plain_string).unwrap_or_default();
            (text.trim().parse::<f64>().ok(), text)
        };
        rows.sort_by(|a, b| {
            for by in sort {
                let (left, right) = (key(a.get(&by.field)), key(b.get(&by.field)));
                let order = match (left.0, right.0) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => left.1.cmp(&right.1),
                };
                let order = if by.descending { order.reverse() } else { order };
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    Ok((rows, truncated))
}

/// The files under `location`, at any depth, with their last-modified times.
fn files_under(location: &str, out: &mut Vec<(String, Option<i64>)>) {
    let entries = ExecutionContext::with_current(|ec| ec.factory().resource().list(location))
        .and_then(Result::ok)
        .unwrap_or_default();
    for entry in entries {
        if entry.is_file {
            out.push((entry.location.clone(), last_modified(&entry.location)));
        } else {
            files_under(&entry.location, out);
        }
    }
}

/// Delete the files under `root` older than `keep_ms`, then the directories left empty. A file that will not delete
/// is logged and the rest go on. The number deleted.
fn clean(root: &str, keep_ms: i64) -> u64 {
    let cutoff = jiff::Timestamp::now().as_millisecond() - keep_ms;
    let mut files = Vec::new();
    files_under(root, &mut files);
    let mut deleted = 0;
    let mut parents = std::collections::BTreeSet::new();
    for (location, modified) in files {
        if modified.is_none_or(|modified| modified >= cutoff) {
            continue;
        }
        let removed = ExecutionContext::with_current(|ec| ec.factory().resource().delete(&location));
        match removed {
            Some(Ok(true)) => {
                deleted += 1;
                if let Some((parent, _)) = location.rsplit_once('/') {
                    parents.insert(parent.to_string());
                }
            }
            Some(Ok(false)) | None => {}
            Some(Err(err)) => tracing::warn!(error = %err, location, "Could not delete an old grid file"),
        }
    }
    // the upload's directory, then the user's when it is empty too: deepest first
    let mut directories: Vec<String> = parents.into_iter().collect();
    directories.sort_by_key(|location| std::cmp::Reverse(location.len()));
    for directory in directories {
        let mut current = directory;
        while current.len() > root.len() {
            let removed = ExecutionContext::with_current(|ec| ec.factory().resource().delete(&current));
            if !matches!(removed, Some(Ok(true))) {
                break;
            }
            match current.rsplit_once('/') {
                Some((parent, _)) => current = parent.to_string(),
                None => break,
            }
        }
    }
    deleted
}

/// `clean#GridUploads`: delete the grid's uploads older than seven days and its export files older than a day (the
/// daily ServiceJob `clean_GridUploads_daily`). Out: `deleted`.
pub(crate) fn clean_grid_uploads(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, moqui_lib::error::Error> {
    // the ServiceJob runs with no user signed in; a signed-in caller needs DATABASE_ADMIN
    if moqui_core::user::current_user_id().ok().flatten().is_some()
        && !crate::group_db::require_admin("Removing grid uploads")
    {
        return Ok(Out::new());
    }
    let deleted = clean(UPLOAD_ROOT, UPLOAD_KEEP_MS) + clean(EXPORT_ROOT, EXPORT_KEEP_MS);
    if deleted > 0 {
        tracing::info!(deleted, "Removed old grid uploads and export files");
    }
    let mut out = Out::new();
    out.insert(
        "deleted".to_string(),
        Value::I64(i64::try_from(deleted).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

/// `get#GridFileInfo`: a file source's columns with their inferred types, its sheet names and its row count, for
/// "Load into…" and the grid's file picker. In: `location` (a place the user may read: see the module), `sheet`,
/// `headerRow` (default 1), `format`. Out: `columns`, `sheetNames`, `rowCount`, `rows` (the first `limit`, default
/// 100).
pub(crate) fn get_grid_file_info(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, moqui_lib::error::Error> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let map: moqui_lib::object::ValueMap = parameters.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let spec = FileSpec::from_map(&map);
    let limit = crate::group_db::number(parameters, "limit")
        .unwrap_or(100)
        .clamp(0, 10_000) as usize;
    let read = file_info(&spec, limit);
    let mut out = Out::new();
    match read {
        Ok((columns, sheets, rows, count)) => {
            out.insert(
                "columns".to_string(),
                Value::List(columns.iter().map(FileColumn::to_value).collect()),
            );
            out.insert(
                "sheetNames".to_string(),
                Value::List(sheets.into_iter().map(Value::String).collect()),
            );
            out.insert("rowCount".to_string(), Value::I64(count as i64));
            out.insert(
                "rows".to_string(),
                Value::List(rows.into_iter().map(Value::Map).collect()),
            );
        }
        Err(err) => add_error(err.to_string()),
    }
    Ok(out)
}

/// `check#GridUpload`: whether the user may upload a file of `size` bytes for the grid, `upload#GridFile`'s check
/// before it writes: the Tools app, and at most [`MAX_UPLOAD_BYTES`]. Out: `allowed`.
pub(crate) fn check_grid_upload(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, moqui_lib::error::Error> {
    let mut out = Out::new();
    let allowed = crate::service::may_view_tools() && {
        let size = crate::group_db::number(parameters, "size").unwrap_or(0);
        if size > MAX_UPLOAD_BYTES {
            add_error(format!(
                "A file for the grid is at most {} MB",
                MAX_UPLOAD_BYTES / 1024 / 1024
            ));
            false
        } else {
            true
        }
    };
    out.insert("allowed".to_string(), Value::Bool(allowed));
    Ok(out)
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, clean_grid_uploads)?;
    rust_service!(map, get_grid_file_info)?;
    rust_service!(map, check_grid_upload)?;
    Ok(())
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
