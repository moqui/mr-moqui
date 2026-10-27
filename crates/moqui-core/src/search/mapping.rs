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

//! The stored shape of a data document, a port of `ElasticFacadeImpl.makeElasticSearchMapping`.
//!
//! Type-one relationships are flattened into their parent object. Type-many relationships are
//! child arrays. A field's kind comes from `DataDocumentField.fieldType` when set (a search type
//! name such as `keyword` or `double`), else from the entity field's dictionary type, else
//! `double` for an expression, as the Java mapping does.

use indexmap::IndexMap;

use crate::entity::document::{Document, DocumentInfo, FieldNode};
use moqui_lib::object::Value;

/// How a field is stored and matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldKind {
    /// Exact, case-sensitive value. Ids and indicators.
    Keyword,
    /// Tokenized, case-insensitive text. Sorts only when sortable.
    Text,
    Long,
    /// Exact decimal on Postgres. `double` in an OpenSearch mapping.
    Double,
    /// A timestamp. Dates are midnight UTC.
    Date,
    Boolean,
    Binary,
}

impl FieldKind {
    /// A search mapping type name: `keyword`, `text`, `long`, `double`, `date`, and the like.
    pub fn from_search_type(name: &str) -> Self {
        match name {
            "text" => Self::Text,
            "long" | "integer" | "short" | "byte" | "unsigned_long" => Self::Long,
            "double" | "float" | "half_float" | "scaled_float" => Self::Double,
            "date" | "date_nanos" => Self::Date,
            "boolean" => Self::Boolean,
            "binary" => Self::Binary,
            _ => Self::Keyword,
        }
    }

    /// `esTypeMap` from the Java facade, keyed by dictionary type.
    pub fn from_entity_type(name: &str) -> Option<Self> {
        Some(match name {
            "id" | "id-long" | "text-indicator" => Self::Keyword,
            "date" | "date-time" => Self::Date,
            "time" | "text-short" | "text-medium" | "text-intermediate" | "text-long" | "text-very-long"
            | "text-fts" => Self::Text,
            "number-integer" => Self::Long,
            "number-decimal" | "number-float" | "currency-amount" | "currency-precise" => Self::Double,
            "binary-very-long" => Self::Binary,
            _ => return None,
        })
    }

    pub fn search_type(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Text => "text",
            Self::Long => "long",
            Self::Double => "double",
            Self::Date => "date",
            Self::Boolean => "boolean",
            Self::Binary => "binary",
        }
    }

    /// True for the kinds a text clause can match.
    pub fn is_textual(self) -> bool {
        matches!(self, Self::Keyword | Self::Text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldMapping {
    pub kind: FieldKind,
    /// Text with `sortable=Y`, which keeps an unanalyzed value to sort on.
    pub sortable: bool,
}

impl FieldMapping {
    /// Whether a sort may use this field. Text sorts only when sortable.
    pub fn can_sort(&self) -> bool {
        self.kind != FieldKind::Text || self.sortable
    }
}

/// Fields of one object, and the child arrays below it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectMapping {
    pub fields: IndexMap<String, FieldMapping>,
    pub children: IndexMap<String, ObjectMapping>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentMapping {
    pub document_id: String,
    /// Lower-case `indexName`, the group a search can target.
    pub index_name: Option<String>,
    /// `mantle_party` for `MantleParty`.
    pub physical_name: String,
    pub root: ObjectMapping,
    pub manual_mapping_service: Option<String>,
    /// Root fields with a `searchRole` or a `searchWeight`.
    pub display: DisplayFields,
}

/// One typed view of an authorized multi-store population. Conflicting declarations are
/// refused rather than silently coercing distinct identifiers or dropping a store.
pub(crate) fn combined_mapping(
    mappings: &[std::sync::Arc<DocumentMapping>],
) -> Result<DocumentMapping, moqui_lib::error::Error> {
    fn merge(target: &mut ObjectMapping, source: &ObjectMapping, prefix: &str) -> Result<(), moqui_lib::error::Error> {
        for (name, field) in &source.fields {
            if target.children.contains_key(name) {
                return Err(moqui_lib::error::Error::new(format!(
                    "incompatible object/field mappings for {prefix}{name}"
                )));
            }
            if let Some(existing) = target.fields.get_mut(name) {
                if existing.kind != field.kind {
                    if matches!(
                        (existing.kind, field.kind),
                        (FieldKind::Long, FieldKind::Double) | (FieldKind::Double, FieldKind::Long)
                    ) {
                        existing.kind = FieldKind::Double;
                    } else {
                        return Err(moqui_lib::error::Error::new(format!("incompatible aggregation mappings for {prefix}{name}: {} and {}; align the DataDocument field types",existing.kind.search_type(),field.kind.search_type())));
                    }
                }
                existing.sortable &= field.sortable;
            } else {
                target.fields.insert(name.clone(), *field);
            }
        }
        for (name, child) in &source.children {
            if target.fields.contains_key(name) {
                return Err(moqui_lib::error::Error::new(format!(
                    "incompatible field/object mappings for {prefix}{name}"
                )));
            }
            merge(
                target.children.entry(name.clone()).or_default(),
                child,
                &format!("{prefix}{name}."),
            )?;
        }
        Ok(())
    }
    let mut root = ObjectMapping::default();
    for mapping in mappings {
        merge(&mut root, &mapping.root, "")?;
    }
    Ok(DocumentMapping {
        document_id: "__combined_population".into(),
        index_name: None,
        physical_name: "__combined_population".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    })
}

/// What a search result shows and how its text is weighted: `DataDocumentField.searchRole` and
/// `searchWeight`, on root fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayFields {
    /// Role (`title`, `subtitle`, `code`, `category`, `quality`) to the root field that has it.
    pub roles: IndexMap<String, String>,
    /// Root text field to its weight, `A` (highest) through `D`.
    pub weights: IndexMap<String, char>,
}

impl DisplayFields {
    /// The root field with this role.
    pub fn role(&self, role: &str) -> Option<&str> {
        self.roles.get(role).map(String::as_str)
    }

    /// The roles a field may have.
    pub const ROLES: [&'static str; 5] = ["title", "subtitle", "code", "category", "quality"];
}

impl ObjectMapping {
    /// The date fields of a document as times, at the root and in each element of a child array: a number or digits are
    /// epoch millis (how a script sees a timestamp, so `index#DataDocuments` in JavaScript sends them), and other text is
    /// ISO-8601 or the Moqui `yyyy-MM-dd HH:mm:ss.SSS` form, read as UTC. Postgres stores a child element's date only in
    /// the JSON body, where it must be text that casts to `timestamptz`; OpenSearch reads ISO-8601 as well. A
    /// `yyyy-MM-dd` date and text that does not parse are left as they are.
    pub fn type_dates(&self, body: &mut Document) {
        for (name, field) in &self.fields {
            if field.kind != FieldKind::Date {
                continue;
            }
            if let Some(value) = body.get_mut(name) {
                if let Some(at) = date_value(value) {
                    *value = Value::Timestamp(at);
                }
            }
        }
        for (name, child) in &self.children {
            match body.get_mut(name) {
                Some(Value::List(items)) => {
                    for item in items {
                        if let Value::Map(element) = item {
                            child.type_dates(element);
                        }
                    }
                }
                Some(Value::Map(element)) => child.type_dates(element),
                _ => {}
            }
        }
    }
}

fn date_value(value: &Value) -> Option<jiff::Timestamp> {
    let millis = |text: &str| {
        text.parse::<i64>()
            .ok()
            .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
    };
    match value {
        Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => {
            millis(moqui_lib::object::to_plain_string(value).split('.').next()?)
        }
        Value::String(text) => {
            let text = text.trim();
            let digits = text.strip_prefix('-').unwrap_or(text);
            if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                millis(text)
            } else if text.len() == 10 {
                None
            } else {
                moqui_lib::object::parse_timestamp(text, &jiff::tz::TimeZone::UTC)
            }
        }
        _ => None,
    }
}

/// Where a field name in a query lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// A field on the root object.
    Root { name: String, field: FieldMapping },
    /// A field of an element of a child array. `chain` is the array names from the root.
    Child {
        chain: Vec<String>,
        name: String,
        field: FieldMapping,
    },
    /// Not in this document. A clause on it matches nothing.
    Missing,
}

impl Resolved {
    pub fn field(&self) -> Option<FieldMapping> {
        match self {
            Self::Root { field, .. } | Self::Child { field, .. } => Some(*field),
            Self::Missing => None,
        }
    }
}

impl DocumentMapping {
    pub fn from_info(info: &DocumentInfo) -> Self {
        let mut root = ObjectMapping::default();
        root.fields.insert(
            "_entity".to_string(),
            FieldMapping {
                kind: FieldKind::Keyword,
                sortable: false,
            },
        );
        let mut display = DisplayFields::default();
        add_node(&info.root, &mut root, Some(&mut display));
        // Primary key fields not aliased explicitly, as the Java mapping adds them.
        for name in &info.primary_pk {
            let aliased = info.root.fields.iter().any(|field| &field.path_element == name);
            if !aliased && !root.fields.contains_key(name) {
                root.fields.insert(
                    name.clone(),
                    FieldMapping {
                        kind: FieldKind::Keyword,
                        sortable: false,
                    },
                );
            }
        }
        Self {
            document_id: info.data_document_id.clone(),
            index_name: info.index_name.clone(),
            physical_name: physical_name(&info.data_document_id),
            root,
            manual_mapping_service: info.manual_mapping_service.clone(),
            display,
        }
    }

    /// Resolve a query field name. A trailing `.keyword` names the unanalyzed value and resolves
    /// to the same field. Dotted names walk child arrays.
    pub fn resolve(&self, name: &str) -> Resolved {
        let name = name.strip_suffix(".keyword").unwrap_or(name);
        if let Some(field) = self.root.fields.get(name) {
            return Resolved::Root {
                name: name.to_string(),
                field: *field,
            };
        }
        let parts: Vec<&str> = name.split('.').collect();
        let Some((last, chain)) = parts.split_last() else {
            return Resolved::Missing;
        };
        if chain.is_empty() {
            return Resolved::Missing;
        }
        let mut object = &self.root;
        for part in chain {
            match object.children.get(*part) {
                Some(child) => object = child,
                None => return Resolved::Missing,
            }
        }
        match object.fields.get(*last) {
            Some(field) => Resolved::Child {
                chain: chain.iter().map(|part| (*part).to_string()).collect(),
                name: (*last).to_string(),
                field: *field,
            },
            None => Resolved::Missing,
        }
    }

    /// The child object at a dotted array path, for a `Child` query.
    pub fn child(&self, path: &str) -> Option<&ObjectMapping> {
        let mut object = &self.root;
        for part in path.split('.') {
            object = object.children.get(part)?;
        }
        Some(object)
    }

    /// Text and keyword fields anywhere in the document, as dotted names.
    pub fn textual_fields(&self) -> Vec<(String, FieldMapping)> {
        fn walk(prefix: &str, object: &ObjectMapping, out: &mut Vec<(String, FieldMapping)>) {
            for (name, field) in &object.fields {
                if field.kind.is_textual() {
                    out.push((format!("{prefix}{name}"), *field));
                }
            }
            for (name, child) in &object.children {
                walk(&format!("{prefix}{name}."), child, out);
            }
        }
        let mut out = Vec::new();
        walk("", &self.root, &mut out);
        out
    }

    /// Stable text of the stored shape. A change means the store must be rebuilt.
    pub fn signature(&self) -> String {
        fn write(object: &ObjectMapping, out: &mut String) {
            for (name, field) in &object.fields {
                out.push_str(name);
                out.push(':');
                out.push_str(field.kind.search_type());
                if field.sortable {
                    out.push_str("+s");
                }
                out.push(';');
            }
            for (name, child) in &object.children {
                out.push_str(name);
                out.push('{');
                write(child, out);
                out.push('}');
            }
        }
        let mut out = String::new();
        write(&self.root, &mut out);
        // Roles and weights change the derived title and weighted columns.
        for (role, field) in &self.display.roles {
            out.push_str(&format!("@{role}={field};"));
        }
        for (field, weight) in &self.display.weights {
            out.push_str(&format!("^{field}={weight};"));
        }
        out
    }

    /// FNV-1a of [`Self::signature`], as hex.
    pub fn signature_hash(&self) -> String {
        format!("{:016x}", moqui_lib::codec::fnv1a(self.signature().as_bytes()))
    }
}

/// Fields of `node` into `object`. `display` is the document's when `object` is the root, where
/// roles and weights apply; type-one relationships flatten into the same object.
fn add_node(node: &FieldNode, object: &mut ObjectMapping, mut display: Option<&mut DisplayFields>) {
    for field in &node.fields {
        let kind = match (&field.field_type, &field.entity_type) {
            (Some(search_type), _) => FieldKind::from_search_type(search_type),
            (None, Some(entity_type)) => FieldKind::from_entity_type(entity_type).unwrap_or(FieldKind::Text),
            (None, None) => FieldKind::Double,
        };
        object.fields.insert(
            field.alias.clone(),
            FieldMapping {
                kind,
                sortable: field.sortable,
            },
        );
        if let Some(display) = display.as_deref_mut() {
            if let Some(role) = field.search_role.as_deref().map(str::to_ascii_lowercase) {
                if DisplayFields::ROLES.contains(&role.as_str()) {
                    display.roles.insert(role, field.alias.clone());
                }
            }
            let weight = field
                .search_weight
                .as_deref()
                .and_then(|text| text.trim().chars().next())
                .map(|ch| ch.to_ascii_uppercase());
            if let Some(weight @ 'A'..='D') = weight {
                if kind.is_textual() {
                    display.weights.insert(field.alias.clone(), weight);
                }
            }
        }
    }
    for rel in &node.relationships {
        if rel.type_one {
            add_node(&rel.node, object, display.as_deref_mut());
        } else {
            let child = object.children.entry(rel.document_alias.clone()).or_default();
            add_node(&rel.node, child, None);
        }
    }
}

/// `ddIdToEsIndex`: a name with an underscore is lower-cased, otherwise camel case becomes
/// underscored lower case.
pub fn physical_name(data_document_id: &str) -> String {
    if data_document_id.contains('_') {
        return data_document_id.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(data_document_id.len() + 8);
    for (index, ch) in data_document_id.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// `esIndexToDdId`: `mantle_party` back to `MantleParty`.
pub fn document_id_of(physical: &str) -> String {
    physical
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_names_round_trip() {
        assert_eq!(physical_name("MantleSalesOrderItem"), "mantle_sales_order_item");
        assert_eq!(physical_name("mantle_party"), "mantle_party");
        assert_eq!(document_id_of("mantle_sales_order_item"), "MantleSalesOrderItem");
    }

    fn sample() -> DocumentMapping {
        let mut root = ObjectMapping::default();
        root.fields.insert(
            "combinedName".into(),
            FieldMapping {
                kind: FieldKind::Text,
                sortable: true,
            },
        );
        let mut accounts = ObjectMapping::default();
        accounts.fields.insert(
            "username".into(),
            FieldMapping {
                kind: FieldKind::Text,
                sortable: true,
            },
        );
        root.children.insert("userAccounts".into(), accounts);
        DocumentMapping {
            document_id: "MantleParty".into(),
            index_name: Some("mantle".into()),
            physical_name: "mantle_party".into(),
            root,
            manual_mapping_service: None,
            display: Default::default(),
        }
    }

    #[test]
    fn resolve_strips_keyword_and_walks_children() {
        let mapping = sample();
        assert!(matches!(mapping.resolve("combinedName.keyword"), Resolved::Root { .. }));
        match mapping.resolve("userAccounts.username.keyword") {
            Resolved::Child { chain, name, .. } => {
                assert_eq!(chain, vec!["userAccounts"]);
                assert_eq!(name, "username");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(mapping.resolve("nope.field"), Resolved::Missing);
        assert_eq!(mapping.signature_hash(), sample().signature_hash());
    }
}
