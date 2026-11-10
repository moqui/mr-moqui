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

//! An ETL mapping (plans/archive/Tools-Database.md §10.2): its source, its target and its fields, read from the
//! `moqui.etl.EtlMapping` rows or from an inline map with the same names, as a service takes it.

use std::collections::{BTreeMap, HashMap};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};

pub const MAPPING: &str = "moqui.etl.EtlMapping";
pub const MAPPING_FIELD: &str = "moqui.etl.EtlMappingField";
pub const VALUE_MAP: &str = "moqui.etl.EtlValueMap";
pub const VALUE_MAP_ENTRY: &str = "moqui.etl.EtlValueMapEntry";

/// Where the rows come from (`sourceTypeEnumId`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceKind {
    /// `EtlsEntity`: a find of `sourceEntityName` with `sourceConditionsJson`.
    Entity,
    /// `EtlsSql`: `sourceSql` on `sourceDb`.
    Sql,
    /// `EtlsFile`: a CSV or spreadsheet at `sourceLocation`.
    #[default]
    File,
    /// `EtlsFlatXml`: a flat record XML file at `sourceLocation`.
    FlatXml,
}

impl SourceKind {
    fn parse(text: &str) -> Result<Self, Error> {
        Ok(match text {
            "EtlsEntity" => Self::Entity,
            "EtlsSql" => Self::Sql,
            "EtlsFile" | "" => Self::File,
            "EtlsFlatXml" => Self::FlatXml,
            other => return Err(Error::new(format!("unknown sourceTypeEnumId {other}"))),
        })
    }

    #[must_use]
    pub fn enum_id(self) -> &'static str {
        match self {
            Self::Entity => "EtlsEntity",
            Self::Sql => "EtlsSql",
            Self::File => "EtlsFile",
            Self::FlatXml => "EtlsFlatXml",
        }
    }
}

/// What the rows load into (`targetTypeEnumId`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetKind {
    /// `EtltEntity`: records of the entity `targetName`.
    #[default]
    Entity,
    /// `EtltService`: a call of the service `targetName` per row.
    Service,
}

/// How an entity target is written (`loadModeEnumId`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadMode {
    /// `EtllCreate`: a new record; one already there is an error.
    Create,
    /// `EtllStore`: created or updated.
    #[default]
    Store,
    /// `EtllUpdate`: an existing record; one not there is an error.
    Update,
}

impl LoadMode {
    fn parse(text: &str) -> Result<Self, Error> {
        Ok(match text {
            "EtllCreate" | "create" => Self::Create,
            "EtllStore" | "store" | "" => Self::Store,
            "EtllUpdate" | "update" => Self::Update,
            other => return Err(Error::new(format!("unknown loadModeEnumId {other}"))),
        })
    }
}

/// One field of a mapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MappingField {
    pub target_field: String,
    /// A header, or a column letter (`A`) or number (`1`).
    pub source_column: Option<String>,
    pub constant_value: Option<String>,
    pub default_value: Option<String>,
    /// JavaScript with `row`, `value` and `ec`.
    pub expression: Option<String>,
    pub value_map_id: Option<String>,
    /// The value map's entries by lower-case from value: an inline `valueMap`, or the rows of `value_map_id`.
    pub value_map: Option<HashMap<String, String>>,
    pub lookup_entity_name: Option<String>,
    pub lookup_match_field: Option<String>,
    pub lookup_result_field: Option<String>,
    pub format_pattern: Option<String>,
    pub trim: bool,
    pub required: bool,
    pub skip_if_empty: bool,
}

/// A mapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapping {
    /// Empty for an inline mapping.
    pub mapping_id: Option<String>,
    pub description: String,
    pub source_kind: SourceKind,
    pub source_entity_name: String,
    pub source_conditions: BTreeMap<String, Value>,
    pub source_db: String,
    pub source_sql: String,
    pub source_location: String,
    /// `csv`, `xlsx` (any spreadsheet calamine reads) or `auto` (by the location's extension).
    pub file_format: String,
    pub sheet_name: String,
    /// The header row, from 1; 0 is none.
    pub header_row: usize,
    /// The first data row, from 1; 0 is the row after the header.
    pub first_data_row: usize,
    pub csv_delimiter: u8,
    pub csv_charset: String,
    pub target_kind: TargetKind,
    pub target_name: String,
    pub load_mode: LoadMode,
    pub match_fields: Vec<String>,
    /// 0 is one transaction for the run.
    pub batch_size: usize,
    pub stop_on_error: bool,
    pub dummy_fks: bool,
    pub timeout_seconds: Option<u32>,
    pub fields: Vec<MappingField>,
}

fn text_of(map: &moqui_lib::object::ValueMap, key: &str) -> String {
    map.get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

fn optional(map: &moqui_lib::object::ValueMap, key: &str) -> Option<String> {
    Some(text_of(map, key)).filter(|text| !text.is_empty())
}

fn indicator(map: &moqui_lib::object::ValueMap, key: &str, default: bool) -> bool {
    match map.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) if !is_empty(value) => matches!(to_plain_string(value).as_str(), "Y" | "true" | "y" | "1"),
        _ => default,
    }
}

fn whole(map: &moqui_lib::object::ValueMap, key: &str, default: usize) -> usize {
    let text = text_of(map, key);
    if text.is_empty() {
        return default;
    }
    text.trim()
        .parse::<f64>()
        .map_or(default, |number| number.max(0.0) as usize)
}

fn row_map(row: &EntityValue) -> moqui_lib::object::ValueMap {
    row.fields()
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
}

impl MappingField {
    /// From a map of `EtlMappingField`'s names, an inline `valueMap` (from value to value) allowed.
    fn from_map(map: &moqui_lib::object::ValueMap) -> Self {
        let value_map = match map.get("valueMap") {
            Some(Value::Map(entries)) => Some(
                entries
                    .iter()
                    .map(|(from, to)| (from.to_lowercase(), to_plain_string(to)))
                    .collect(),
            ),
            _ => None,
        };
        Self {
            target_field: text_of(map, "targetField"),
            source_column: optional(map, "sourceColumn"),
            constant_value: optional(map, "constantValue"),
            default_value: optional(map, "defaultValue"),
            expression: optional(map, "expression"),
            value_map_id: optional(map, "valueMapId"),
            value_map,
            lookup_entity_name: optional(map, "lookupEntityName"),
            lookup_match_field: optional(map, "lookupMatchField"),
            lookup_result_field: optional(map, "lookupResultField"),
            format_pattern: optional(map, "formatPattern"),
            trim: indicator(map, "trim", false),
            required: indicator(map, "required", false),
            skip_if_empty: indicator(map, "skipIfEmpty", false),
        }
    }

    /// As an inline map, the value map inline.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let flag = |value: bool| Value::string(if value { "Y" } else { "N" });
        let mut map = IndexMap::default();
        map.insert("targetField".to_string(), Value::string(&self.target_field));
        map.insert("sourceColumn".to_string(), text(&self.source_column));
        map.insert("constantValue".to_string(), text(&self.constant_value));
        map.insert("defaultValue".to_string(), text(&self.default_value));
        map.insert("expression".to_string(), text(&self.expression));
        map.insert("valueMapId".to_string(), text(&self.value_map_id));
        map.insert("lookupEntityName".to_string(), text(&self.lookup_entity_name));
        map.insert("lookupMatchField".to_string(), text(&self.lookup_match_field));
        map.insert("lookupResultField".to_string(), text(&self.lookup_result_field));
        map.insert("formatPattern".to_string(), text(&self.format_pattern));
        map.insert("trim".to_string(), flag(self.trim));
        map.insert("required".to_string(), flag(self.required));
        map.insert("skipIfEmpty".to_string(), flag(self.skip_if_empty));
        // an inline value map (one not named by id) stays with its field
        if let (None, Some(entries)) = (&self.value_map_id, &self.value_map) {
            let mut sorted: Vec<(&String, &String)> = entries.iter().collect();
            sorted.sort();
            map.insert(
                "valueMap".to_string(),
                Value::Map(
                    sorted
                        .into_iter()
                        .map(|(from, to)| (from.clone(), Value::string(to)))
                        .collect(),
                ),
            );
        }
        Value::Map(map)
    }
}

impl Mapping {
    /// From a map of `EtlMapping`'s names with its `fields` (a list of maps of `EtlMappingField`'s names): an inline
    /// mapping, or a saved one read into the same shape. Value maps named by id are read in [`Self::resolve`].
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown source type or load mode, or a mapping with no target.
    pub fn from_map(map: &moqui_lib::object::ValueMap) -> Result<Self, Error> {
        let delimiter = text_of(map, "csvDelimiter");
        let delimiter = match delimiter.as_str() {
            "" => b',',
            "\\t" | "tab" => b'\t',
            other => *other.as_bytes().first().unwrap_or(&b','),
        };
        let fields = match map.get("fields") {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Value::Map(field) => Some(MappingField::from_map(field)),
                    _ => None,
                })
                .filter(|field| !field.target_field.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        let source_conditions = match map.get("sourceConditions") {
            Some(Value::Map(conditions)) => conditions.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => {
                let json = text_of(map, "sourceConditionsJson");
                if json.is_empty() {
                    BTreeMap::new()
                } else {
                    let parsed: Option<serde_json::Value> = serde_json::from_str(&json).ok();
                    match parsed.as_ref().map(moqui_lib::json::from_json) {
                        Some(Value::Map(conditions)) => conditions.into_iter().collect(),
                        _ => return Err(Error::new("sourceConditionsJson is not a JSON object")),
                    }
                }
            }
        };
        let mapping = Self {
            mapping_id: optional(map, "mappingId"),
            description: text_of(map, "description"),
            source_kind: SourceKind::parse(&text_of(map, "sourceTypeEnumId"))?,
            source_entity_name: text_of(map, "sourceEntityName"),
            source_conditions,
            source_db: text_of(map, "sourceDb"),
            source_sql: text_of(map, "sourceSql"),
            source_location: text_of(map, "sourceLocation"),
            file_format: optional(map, "fileFormat").unwrap_or_else(|| "auto".to_string()),
            sheet_name: text_of(map, "sheetName"),
            header_row: whole(map, "headerRow", 1),
            first_data_row: whole(map, "firstDataRow", 0),
            csv_delimiter: delimiter,
            csv_charset: text_of(map, "csvCharset"),
            target_kind: match text_of(map, "targetTypeEnumId").as_str() {
                "EtltService" => TargetKind::Service,
                "EtltEntity" | "" => TargetKind::Entity,
                other => return Err(Error::new(format!("unknown targetTypeEnumId {other}"))),
            },
            target_name: text_of(map, "targetName"),
            load_mode: LoadMode::parse(&text_of(map, "loadModeEnumId"))?,
            match_fields: text_of(map, "matchFields")
                .split(',')
                .map(str::trim)
                .filter(|field| !field.is_empty())
                .map(str::to_string)
                .collect(),
            batch_size: whole(map, "batchSize", 0),
            stop_on_error: indicator(map, "stopOnError", true),
            dummy_fks: indicator(map, "dummyFks", false),
            timeout_seconds: optional(map, "timeoutSeconds").and_then(|text| text.parse().ok()),
            fields,
        };
        if mapping.target_name.is_empty() {
            return Err(Error::new("An ETL mapping needs a targetName"));
        }
        Ok(mapping)
    }

    /// Whether a field has an `expression`, which needs the `GROOVY_SHELL_WEB` permission to save, check or run.
    #[must_use]
    pub fn has_expression(&self) -> bool {
        self.fields.iter().any(|field| field.expression.is_some())
    }

    /// Refuse what no mapping may reach ([`super::access`]), by name: a restricted entity as the source, the target or
    /// a lookup; a service target that deletes (`delete#`), or an entity-auto service (`verb#Entity`) of a restricted
    /// entity; and a `UserAccount` password field as a target field or a lookup's match or result field. See
    /// [`Self::validate_with`] for views.
    ///
    /// # Errors
    ///
    /// An error naming what is refused.
    pub fn validate(&self) -> Result<(), Error> {
        use super::access::{is_secret_field, refuse_restricted, service_target_parts};
        if self.source_kind == SourceKind::Entity {
            refuse_restricted(&self.source_entity_name, "source")?;
        }
        match self.target_kind {
            TargetKind::Entity => refuse_restricted(&self.target_name, "target")?,
            TargetKind::Service => {
                let (verb, noun, entity_auto) = service_target_parts(&self.target_name);
                if verb == "delete" {
                    return Err(Error::new(format!(
                        "{} deletes: no delete# service is an ETL target",
                        self.target_name
                    )));
                }
                if entity_auto {
                    refuse_restricted(&noun, "target")?;
                }
            }
        }
        for field in &self.fields {
            if is_secret_field(&field.target_field) {
                return Err(Error::new(format!(
                    "{} is not available as an ETL target field",
                    field.target_field
                )));
            }
            if let Some(entity) = &field.lookup_entity_name {
                refuse_restricted(entity, "lookup")?;
            }
            let lookup_fields = [&field.lookup_match_field, &field.lookup_result_field];
            if let Some(name) = lookup_fields.into_iter().flatten().find(|name| is_secret_field(name)) {
                return Err(Error::new(format!("{name} is not available in an ETL lookup")));
            }
        }
        Ok(())
    }

    /// [`Self::validate`], and what only the entity definitions tell: a view over a restricted entity as the source,
    /// the target (also an entity-auto service's) or a lookup, and a lookup's match or result field that is a view's
    /// alias of a `UserAccount` password field under another name.
    ///
    /// # Errors
    ///
    /// An error naming what is refused.
    pub fn validate_with(&self, facade: &EntityFacade) -> Result<(), Error> {
        use super::access::{refuse_restricted_view, secret_fields_of, service_target_parts};
        self.validate()?;
        if self.source_kind == SourceKind::Entity {
            refuse_restricted_view(facade, &self.source_entity_name, "source")?;
        }
        match self.target_kind {
            TargetKind::Entity => refuse_restricted_view(facade, &self.target_name, "target")?,
            TargetKind::Service => {
                let (_, noun, entity_auto) = service_target_parts(&self.target_name);
                if entity_auto {
                    refuse_restricted_view(facade, &noun, "target")?;
                }
            }
        }
        for field in &self.fields {
            let Some(entity) = &field.lookup_entity_name else {
                continue;
            };
            refuse_restricted_view(facade, entity, "lookup")?;
            let secrets = secret_fields_of(facade, entity);
            let lookup_fields = [&field.lookup_match_field, &field.lookup_result_field];
            if let Some(name) = lookup_fields.into_iter().flatten().find(|name| secrets.contains(name)) {
                return Err(Error::new(format!("{name} is not available in an ETL lookup")));
            }
        }
        Ok(())
    }

    /// The saved mapping `mapping_id` with its fields, in `fieldSeqId` order.
    ///
    /// # Errors
    ///
    /// Returns an error when there is no such mapping, or it does not read.
    pub fn load(facade: &EntityFacade, mapping_id: &str) -> Result<Self, Error> {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let Some(row) = facade.find(MAPPING)?.eq("mappingId", Value::string(mapping_id)).one()? else {
            return Err(Error::new(format!("no ETL mapping {mapping_id}")));
        };
        let mut map = row_map(&row);
        let fields: Vec<Value> = facade
            .find(MAPPING_FIELD)?
            .eq("mappingId", Value::string(mapping_id))
            .order_by("fieldSeqId", false)
            .list()?
            .iter()
            .map(|field| Value::Map(row_map(field)))
            .collect();
        map.insert("fields".to_string(), Value::List(fields));
        Self::from_map(&map)
    }

    /// Read the entries of the value maps the fields name by id, matched case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error when a value map's entries do not read.
    pub fn resolve(&mut self, facade: &EntityFacade) -> Result<(), Error> {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut read: HashMap<String, HashMap<String, String>> = HashMap::new();
        for field in &mut self.fields {
            let Some(id) = field.value_map_id.clone() else {
                continue;
            };
            if field.value_map.is_some() {
                continue;
            }
            if !read.contains_key(&id) {
                let entries = facade
                    .find(VALUE_MAP_ENTRY)?
                    .eq("valueMapId", Value::string(&id))
                    .list()?
                    .iter()
                    .map(|entry| {
                        (
                            entry
                                .get("fromValue")
                                .map(to_plain_string)
                                .unwrap_or_default()
                                .to_lowercase(),
                            entry.get("toValue").map(to_plain_string).unwrap_or_default(),
                        )
                    })
                    .collect();
                read.insert(id.clone(), entries);
            }
            field.value_map = read.get(&id).cloned();
        }
        Ok(())
    }

    /// As an inline map (`fields` a list of maps), for the screens and a copy.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::default();
        let text = |value: &str| Value::string(value);
        map.insert(
            "mappingId".to_string(),
            self.mapping_id.clone().map_or(Value::Null, Value::String),
        );
        map.insert("description".to_string(), text(&self.description));
        map.insert("sourceTypeEnumId".to_string(), text(self.source_kind.enum_id()));
        map.insert("sourceEntityName".to_string(), text(&self.source_entity_name));
        map.insert(
            "sourceConditions".to_string(),
            Value::Map(self.source_conditions.clone().into_iter().collect()),
        );
        map.insert("sourceDb".to_string(), text(&self.source_db));
        map.insert("sourceSql".to_string(), text(&self.source_sql));
        map.insert("sourceLocation".to_string(), text(&self.source_location));
        map.insert("fileFormat".to_string(), text(&self.file_format));
        map.insert("sheetName".to_string(), text(&self.sheet_name));
        map.insert("headerRow".to_string(), Value::I64(self.header_row as i64));
        map.insert("firstDataRow".to_string(), Value::I64(self.first_data_row as i64));
        map.insert(
            "csvDelimiter".to_string(),
            text(&char::from(self.csv_delimiter).to_string()),
        );
        map.insert("csvCharset".to_string(), text(&self.csv_charset));
        map.insert(
            "targetTypeEnumId".to_string(),
            text(match self.target_kind {
                TargetKind::Entity => "EtltEntity",
                TargetKind::Service => "EtltService",
            }),
        );
        map.insert("targetName".to_string(), text(&self.target_name));
        map.insert(
            "loadModeEnumId".to_string(),
            text(match self.load_mode {
                LoadMode::Create => "EtllCreate",
                LoadMode::Store => "EtllStore",
                LoadMode::Update => "EtllUpdate",
            }),
        );
        map.insert("matchFields".to_string(), text(&self.match_fields.join(",")));
        map.insert("batchSize".to_string(), Value::I64(self.batch_size as i64));
        map.insert(
            "stopOnError".to_string(),
            text(if self.stop_on_error { "Y" } else { "N" }),
        );
        map.insert("dummyFks".to_string(), text(if self.dummy_fks { "Y" } else { "N" }));
        map.insert(
            "timeoutSeconds".to_string(),
            self.timeout_seconds
                .map_or(Value::Null, |seconds| Value::I64(i64::from(seconds))),
        );
        map.insert(
            "fields".to_string(),
            Value::List(self.fields.iter().map(MappingField::to_value).collect()),
        );
        Value::Map(map)
    }
}
