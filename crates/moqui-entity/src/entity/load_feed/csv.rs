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

//! Entity CSV data files: the entity or service name and type line, the header line, then a record per line.

use std::collections::BTreeMap;
use std::io::Read;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::loader::{
    call_data_file_service, complete_csv_pk, data_service_name, decode_base64, entity_defined, shape_of, skip_type,
    store_value, Record,
};
use super::{DataFileService, LoadOptions};
use crate::entity::facade::EntityFacade;

pub(super) fn load_csv_reader(
    facade: &EntityFacade,
    location: &str,
    reader: impl Read,
    options: &LoadOptions,
) -> Result<u64, Error> {
    // Java's `csvDelimiter`, `csvCommentStart` and `csvQuoteChar`, one ASCII character each.
    let byte = |value: Option<char>, default: u8, what: &str| -> Result<u8, Error> {
        match value {
            None => Ok(default),
            Some(ch) if ch.is_ascii() => Ok(ch as u8),
            Some(ch) => Err(Error::new(format!(
                "csv {what} {ch:?} for {location} is not one ASCII character"
            ))),
        }
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(byte(options.csv_delimiter, b',', "delimiter")?)
        .comment(Some(byte(options.csv_comment_start, b'#', "comment start")?))
        .quote(byte(options.csv_quote_char, b'"', "quote")?)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(reader);
    let mut records = reader.records();
    // A given entity name or field names stand for the entity line and the header line, which the text then lacks.
    let mut file_type = None;
    let entity_name = if let Some(name) = options.csv_entity_name.as_deref().filter(|name| !name.is_empty()) {
        name.to_string()
    } else {
        let first = loop {
            match records.next() {
                Some(Ok(record)) if !record_blank(&record) => break record,
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(Error::new(format!("could not read csv {location}")).with_source(err)),
                None => return Err(Error::new(format!("csv {location} has no data"))),
            }
        };
        file_type = first
            .get(1)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        first.get(0).unwrap_or("").trim().to_string()
    };
    if entity_name.is_empty() {
        return Err(Error::new(format!("csv {location} has no entity name")));
    }
    if skip_type(file_type.as_deref(), &options.data_types) {
        return Ok(0);
    }
    let names: Vec<String> = if options.csv_field_names.is_empty() {
        let header = loop {
            match records.next() {
                Some(Ok(record)) if !record_blank(&record) => break record,
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(Error::new(format!("could not read csv {location}")).with_source(err)),
                None => return Err(Error::new(format!("csv {location} has no header"))),
            }
        };
        header.iter().map(str::to_string).collect()
    } else {
        options.csv_field_names.clone()
    };
    // Java's header map: a name given twice reads the later column. An empty header cell (a trailing delimiter) is
    // no column here; Java's field lookup fails the file on it.
    let mut headers: Vec<(String, usize)> = Vec::new();
    for (index, field) in names.into_iter().enumerate() {
        if field.is_empty() {
            continue;
        }
        match headers.iter_mut().find(|(name, _)| *name == field) {
            Some(header) => header.1 = index,
            None => headers.push((field, index)),
        }
    }
    // As Java: the entity, else a service `isServiceDefined` takes, else the file fails.
    let service_name = if entity_defined(facade, &entity_name) {
        None
    } else {
        Some(data_service_name(&entity_name).ok_or_else(|| {
            Error::new(format!(
                "csv {location} names {entity_name}, which is not an entity or a service"
            ))
        })?)
    };
    let shape = if service_name.is_some() {
        None
    } else {
        let shape = shape_of(facade, &entity_name)?;
        // Java looks each column's field up and fails on one that is not a field of the entity.
        if let Some((name, _)) = headers
            .iter()
            .find(|(name, _)| !shape.fields.contains_key(name.as_str()))
        {
            return Err(Error::new(format!(
                "csv {location} column {name} is not a field of {}",
                shape.full_name
            )));
        }
        Some(shape)
    };
    let mut total = 0_u64;
    for (line, record) in records.enumerate() {
        let record = record.map_err(|err| Error::new(format!("could not read csv {location}")).with_source(err))?;
        if record_blank(&record) {
            continue;
        }
        if let Some(shape) = &shape {
            let mut fields = BTreeMap::new();
            for (name, text) in &options.default_values {
                if shape.fields.contains_key(name.as_str()) {
                    fields.insert(shape.name(name), Value::string(text));
                }
            }
            for (name, index) in &headers {
                // Java reads every column of the header, so a short row fails the file.
                let Some(text) = record.get(*index) else {
                    return Err(Error::new(format!(
                        "csv {location} data row {} has no column {} ({name})",
                        line + 1,
                        index + 1
                    )));
                };
                // An empty cell is null, so an update clears the field.
                if text.is_empty() {
                    fields.insert(shape.name(name), Value::Null);
                    continue;
                }
                // A BLOB cell is Base64, which Java decodes, failing the file when it is not.
                if shape
                    .fields
                    .get(name.as_str())
                    .is_some_and(|field_type| field_type == "binary-very-long")
                {
                    let bytes = decode_base64(text).ok_or_else(|| {
                        Error::new(format!(
                            "csv {location} field {name} of {} is not Base64",
                            shape.full_name
                        ))
                    })?;
                    fields.insert(shape.name(name), Value::Bytes(bytes));
                    continue;
                }
                fields.insert(shape.name(name), Value::string(text));
            }
            complete_csv_pk(facade, &shape.full_name, &mut fields)?;
            let stored = store_value(
                facade,
                options,
                Record {
                    entity: shape.full_name.clone(),
                    fields,
                    children: Vec::new(),
                },
            )?;
            total = total.saturating_add(stored);
        } else {
            let mut parameters = BTreeMap::new();
            for (name, text) in &options.default_values {
                parameters.insert(name.clone(), text.clone());
            }
            for (name, index) in &headers {
                if let Some(text) = record.get(*index).filter(|text| !text.is_empty()) {
                    parameters.insert(name.clone(), text.to_string());
                }
            }
            call_data_file_service(
                facade,
                options,
                DataFileService {
                    name: service_name.clone().unwrap_or_default(),
                    location: location.to_string(),
                    parameters,
                },
            )?;
        }
    }
    Ok(total)
}

fn record_blank(record: &csv::StringRecord) -> bool {
    record.iter().all(|field| field.is_empty())
}
