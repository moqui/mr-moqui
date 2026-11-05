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

//! The extractors of an ETL run (plans/Tools-Database.md §10.3): a CSV file streamed (`moqui_lib::etl::CsvExtractor`),
//! a spreadsheet's sheet ([`SheetExtractor`], over `moqui_core::service::spreadsheet::read_sheet_bytes`), an entity
//! find read a page at a time, a SQL query on an entity group's database run read only, and a flat record XML file.
//! Each entry carries its row number under `ROW_NUMBER_KEY`. A file is read only from the places
//! [`super::access`] allows; an entity source is not a restricted entity and its rows leave out the `UserAccount`
//! password fields; a SQL source reads an entity group's database or an external connection (`ext:<id>`, through
//! [`crate::ext::gate`] and its pool, the one vague error on any refusal), read only, and writes a `DbaSql` audit row.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::etl::{
    column_number_name, unique_column_names, CsvExtractor, CsvOptions, Extractor, SharedHeaders, SimpleEntry,
    SimpleEtl, ROW_NUMBER_KEY,
};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

use super::mapping::{Mapping, SourceKind};
use crate::DbRef;

/// The most rows a SQL source reads: it is read whole before the load, outside the load's transaction.
pub const MAX_SQL_ROWS: usize = 200_000;
/// The page size of an entity source.
pub const ENTITY_PAGE: i64 = 1000;

/// What a run reads, beyond its mapping.
#[derive(Clone, Debug, Default)]
pub struct SourceOptions {
    /// In place of the mapping's `sourceLocation`; `${date}` and the like are expanded ([`expand_location`]).
    pub location: Option<String>,
    /// The file's bytes, in place of a location (an upload, `load#SpreadsheetEntity`'s `contentBytes`).
    pub content: Option<Vec<u8>>,
    /// The sheet's index when the mapping names no sheet.
    pub sheet_index: usize,
    /// No row before this one, from 1.
    pub start_row: usize,
    /// At most this many rows.
    pub max_rows: Option<usize>,
    /// A check, not a run: what a SQL source's audit row says.
    pub check: bool,
}

/// An opened source: the extractor, the header it reads, and the source as resolved.
pub struct Source {
    pub extractor: Box<dyn Extractor>,
    pub headers: SharedHeaders,
    pub resolved: String,
}

pub use super::access::expand_location;

/// Whether a file is CSV: `fileFormat` `csv`, or `auto` and a `.csv`, `.tsv` or `.txt` name.
#[must_use]
pub fn is_csv(format: &str, location: &str) -> bool {
    match format {
        "csv" => true,
        "xlsx" | "sheet" => false,
        _ => {
            let lower = location.to_ascii_lowercase();
            [".csv", ".tsv", ".txt"]
                .iter()
                .any(|extension| lower.ends_with(extension))
        }
    }
}

/// The bytes at `location`, through the resource facade, as the caller: no check of the place (see
/// [`super::access::read_bytes`], which ETL runs and the grid use).
///
/// # Errors
///
/// Returns an error when there is no file there.
pub fn read_location(location: &str) -> Result<Vec<u8>, Error> {
    let found = ExecutionContext::with_current(|ec| ec.factory().resource().location_bytes(location))
        .ok_or_else(|| Error::new("no execution context is entered"))?;
    match found {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(Error::new(format!("no file at {location}"))),
        Err(err) => Err(Error::new(format!("Could not read {location}: {err}"))),
    }
}

/// CSV bytes as UTF-8 text bytes: a UTF-8 byte order mark dropped, and Latin-1 (`ISO-8859-1`, `windows-1252`) read
/// byte for character.
#[must_use]
pub fn csv_text_bytes(bytes: Vec<u8>, charset: &str) -> Vec<u8> {
    let charset = charset.to_ascii_lowercase();
    if matches!(
        charset.as_str(),
        "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252"
    ) {
        return bytes
            .iter()
            .map(|byte| char::from(*byte))
            .collect::<String>()
            .into_bytes();
    }
    match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => rest.to_vec(),
        None => bytes,
    }
}

/// The rows of one sheet as entries: the header at `header_row` (a sheet row, from 1; 0 for none), data from
/// `first_data_row` (0 is the row after the header), cells keep their types (numbers, dates as time stamps).
pub struct SheetExtractor {
    sheet: Option<moqui_core::service::spreadsheet::SheetData>,
    header_row: usize,
    first_data_row: usize,
    start_row: usize,
    max_rows: Option<usize>,
    etl_type: String,
    headers: SharedHeaders,
}

impl SheetExtractor {
    #[must_use]
    pub fn new(sheet: moqui_core::service::spreadsheet::SheetData, mapping: &Mapping, options: &SourceOptions) -> Self {
        Self {
            sheet: Some(sheet),
            header_row: mapping.header_row,
            first_data_row: mapping.first_data_row,
            start_row: options.start_row,
            max_rows: options.max_rows,
            etl_type: String::new(),
            headers: SharedHeaders::default(),
        }
    }
}

impl Extractor for SheetExtractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        let Some(sheet) = self.sheet.take() else {
            return Ok(());
        };
        let data_start = if self.first_data_row > 0 {
            self.first_data_row
        } else {
            self.header_row + 1
        };
        let mut header: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut taken = 0usize;
        for (cells, number) in sheet.rows.into_iter().zip(sheet.row_numbers) {
            let number = number as usize;
            if number == self.header_row {
                header = cells.iter().map(to_plain_string).collect();
                names = unique_column_names(&header, header.len());
                *lock(&self.headers) = names.clone();
                continue;
            }
            if number < data_start || number < self.start_row {
                continue;
            }
            if self.max_rows.is_some_and(|max| taken >= max) {
                break;
            }
            if cells.len() > names.len() {
                names = unique_column_names(&header, cells.len());
            }
            let mut values = BTreeMap::new();
            for (index, cell) in cells.into_iter().enumerate() {
                if cell.is_null() {
                    continue;
                }
                let name = names.get(index).cloned().unwrap_or_else(|| column_number_name(index));
                values.insert(name, cell);
            }
            taken += 1;
            values.insert(ROW_NUMBER_KEY.to_string(), Value::I64(number as i64));
            if etl
                .process_entry(SimpleEntry::new(self.etl_type.clone(), values))
                .is_err()
            {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Rows already read (a SQL result, a flat XML file's records), numbered from 1.
struct RowsExtractor {
    rows: Vec<BTreeMap<String, Value>>,
    start_row: usize,
}

impl Extractor for RowsExtractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        for (index, mut values) in std::mem::take(&mut self.rows).into_iter().enumerate() {
            let number = index + 1;
            if number < self.start_row {
                continue;
            }
            values.insert(ROW_NUMBER_KEY.to_string(), Value::I64(number as i64));
            if etl.process_entry(SimpleEntry::new(String::new(), values)).is_err() {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// An entity find read a page at a time in its primary key order, outside the load's writes' cursor.
struct EntityExtractor {
    facade: EntityFacade,
    entity_name: String,
    conditions: BTreeMap<String, Value>,
    start_row: usize,
    max_rows: Option<usize>,
    /// The `UserAccount` password fields by name or by origin (a view's renamed alias), left out of each row.
    secrets: Vec<String>,
}

impl Extractor for EntityExtractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        let pk: Vec<String> = self.facade.with_entity(&self.entity_name, |entity, _, _| {
            Ok(entity.pk_field_names().iter().map(|name| (*name).to_string()).collect())
        })?;
        let mut offset = self.start_row.saturating_sub(1) as i64;
        let mut taken = 0usize;
        loop {
            let mut find = self.facade.find(&self.entity_name)?;
            for (field, value) in &self.conditions {
                find = find.eq(field, value.clone());
            }
            for field in &pk {
                find = find.order_by(field, false);
            }
            let page = find.offset(offset).limit(ENTITY_PAGE).list()?;
            if page.is_empty() {
                return Ok(());
            }
            for value in page.iter() {
                offset += 1;
                if self.max_rows.is_some_and(|max| taken >= max) {
                    return Ok(());
                }
                taken += 1;
                let mut values: BTreeMap<String, Value> = value
                    .fields()
                    .iter()
                    .filter(|(name, _)| {
                        !super::access::is_secret_field(name) && !self.secrets.iter().any(|secret| **secret == ***name)
                    })
                    .map(|(name, field)| (name.to_string(), field.clone()))
                    .collect();
                values.insert(ROW_NUMBER_KEY.to_string(), Value::I64(offset));
                if etl.process_entry(SimpleEntry::new(String::new(), values)).is_err() {
                    return Ok(());
                }
            }
            if (page.len() as i64) < ENTITY_PAGE {
                return Ok(());
            }
        }
    }
}

/// Open the source of `mapping`.
///
/// # Errors
///
/// Returns an error when the source is not there, does not read, or is an external database (`Not available` until
/// external connections are built).
pub fn open(facade: &EntityFacade, mapping: &Mapping, options: &SourceOptions) -> Result<Source, Error> {
    let headers = SharedHeaders::default();
    match mapping.source_kind {
        SourceKind::File | SourceKind::FlatXml => {
            let location = match &options.location {
                Some(location) if !location.is_empty() => location.clone(),
                _ => mapping.source_location.clone(),
            };
            let location = expand_location(&location)?;
            let bytes = match &options.content {
                Some(bytes) => bytes.clone(),
                None if location.is_empty() => return Err(Error::new("An ETL file source needs a location")),
                None => super::access::read_bytes(&location)?,
            };
            if mapping.source_kind == SourceKind::FlatXml {
                let rows = flat_xml_rows(&location, &bytes)?;
                return Ok(Source {
                    extractor: Box::new(RowsExtractor {
                        rows,
                        start_row: options.start_row,
                    }),
                    headers,
                    resolved: location,
                });
            }
            if is_csv(&mapping.file_format, &location) {
                let csv_options = CsvOptions {
                    delimiter: if location.to_ascii_lowercase().ends_with(".tsv") && mapping.csv_delimiter == b',' {
                        b'\t'
                    } else {
                        mapping.csv_delimiter
                    },
                    header_row: mapping.header_row,
                    first_data_row: mapping.first_data_row,
                    start_row: options.start_row,
                    max_rows: options.max_rows,
                    etl_type: String::new(),
                };
                let extractor =
                    CsvExtractor::new(Cursor::new(csv_text_bytes(bytes, &mapping.csv_charset)), csv_options);
                let headers = extractor.headers();
                return Ok(Source {
                    extractor: Box::new(extractor),
                    headers,
                    resolved: location,
                });
            }
            let sheet =
                moqui_core::service::spreadsheet::read_sheet_bytes(&bytes, &mapping.sheet_name, options.sheet_index)?;
            let resolved = format!("{location}#{}", sheet.sheet_name);
            let extractor = SheetExtractor::new(sheet, mapping, options);
            let headers = Arc::clone(&extractor.headers);
            Ok(Source {
                extractor: Box::new(extractor),
                headers,
                resolved,
            })
        }
        SourceKind::Entity => {
            super::access::refuse_restricted_view(facade, &mapping.source_entity_name, "source")?;
            Ok(Source {
                extractor: Box::new(EntityExtractor {
                    facade: facade.clone(),
                    entity_name: mapping.source_entity_name.clone(),
                    conditions: mapping.source_conditions.clone(),
                    start_row: options.start_row,
                    max_rows: options.max_rows,
                    secrets: super::access::secret_fields_of(facade, &mapping.source_entity_name),
                }),
                headers,
                resolved: format!("entity {}", mapping.source_entity_name),
            })
        }
        SourceKind::Sql => {
            let limit = options.max_rows.unwrap_or(MAX_SQL_ROWS).min(MAX_SQL_ROWS);
            let run_options = || moqui_core::entity::info::SqlRunOptions {
                limit,
                read_only: true,
                ..Default::default()
            };
            let started = std::time::Instant::now();
            // an entity group's database, or an external connection's through its gate (not a production instance,
            // DATABASE_EXTERNAL, every refusal the one vague error) and its pool, read only either way
            let (db_text, ran) = match DbRef::parse(&mapping.source_db, facade.default_group()) {
                Ok(DbRef::Group(group)) => (
                    DbRef::Group(group.clone()).text(),
                    moqui_core::entity::info::run_sql_with(facade, &group, &mapping.source_sql, run_options()),
                ),
                Ok(DbRef::External(_)) => {
                    let db = crate::group_db::GroupDb::resolve(facade, &mapping.source_db).map_err(Error::new)?;
                    (
                        db.db_text(),
                        moqui_core::entity::info::run_sql_on(&db.tx, &db.dialect, &mapping.source_sql, run_options()),
                    )
                }
                Err(message) => return Err(Error::new(message)),
            };
            crate::audit::record(
                facade,
                &crate::audit::AuditEntry {
                    action: "DbaSql",
                    db: Some(db_text.clone()),
                    statement_text: Some(mapping.source_sql.clone()),
                    is_change: false,
                    rows_returned: ran.as_ref().ok().map(|result| result.records.len() as i64),
                    duration_ms: Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
                    error_text: ran.as_ref().err().map(ToString::to_string),
                    detail_json: Some(
                        serde_json::json!({
                            "etl": if options.check { "check" } else { "run" },
                            "mappingId": mapping.mapping_id,
                        })
                        .to_string(),
                    ),
                    ..crate::audit::AuditEntry::default()
                },
            );
            let result = ran?;
            let rows = result
                .records
                .into_iter()
                .map(|record| {
                    result
                        .columns
                        .iter()
                        .cloned()
                        .zip(record)
                        .filter(|(_, value)| !value.is_null())
                        .collect()
                })
                .collect();
            *lock(&headers) = result.columns.clone();
            Ok(Source {
                extractor: Box::new(RowsExtractor {
                    rows,
                    start_row: options.start_row,
                }),
                headers,
                resolved: format!("{db_text} SQL"),
            })
        }
    }
}

/// The records of a flat record XML file: each element under the root, its attributes and child element texts.
fn flat_xml_rows(location: &str, bytes: &[u8]) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    struct Collect(std::rc::Rc<std::cell::RefCell<Vec<BTreeMap<String, Value>>>>);
    impl moqui_lib::etl::Loader for Collect {
        fn init(&mut self, _timeout: Option<u32>) {}
        fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error> {
            self.0.borrow_mut().push(entry.values.clone());
            Ok(())
        }
        fn complete(&mut self, _etl: &SimpleEtl) {}
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| Error::new(format!("{location} is not UTF-8 text")))?;
    let root = moqui_lib::resource::MemoryResource::new_root("mem://etl");
    let file = moqui_lib::resource::Resource::make_file(&root, "source.xml")?;
    moqui_lib::resource::Resource::write_text(&file, &text)?;
    let rows = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut etl = SimpleEtl::new(
        moqui_lib::etl::FlatXmlExtractor::new(file),
        Collect(std::rc::Rc::clone(&rows)),
    );
    etl.process();
    if let Some(err) = etl.extract_error() {
        return Err(Error::new(err.to_string()));
    }
    let collected = rows.borrow().clone();
    Ok(collected)
}
