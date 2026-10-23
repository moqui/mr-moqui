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

//! Data documents, a port of `EntityDataDocument`.
//!
//! A `DataDocument` names a primary entity and field paths (`Rel:Rel:field`). One query joins the
//! primary entity to every relationship on those paths (outer joins), then rows merge into one
//! nested map per primary key. A type-one relationship is flattened into its parent. A type-many
//! relationship is a list of maps under the relationship's document alias. Expression fields
//! (paths that start with `(`) run after the query with the document map as their names.
//!
//! Each document carries the control fields `_type` (the document id), `_id` (primary key values
//! joined with `::`), `_timestamp`, `_index` (lower-case `indexName`), and `_entity`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use indexmap::IndexMap;
use jiff::Timestamp;
use moqui_db::model::FieldAlias;
use moqui_db::sql::{Condition, FieldRef, LimitOffset, OrderBy};
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::facade::EntityFacade;
use super::query_rest::{run_code_view, CodeView};

const PRIMARY_ALIAS: &str = "PRIM";
const DOCUMENT: &str = "moqui.entity.document.DataDocument";
const DOCUMENT_FIELD: &str = "moqui.entity.document.DataDocumentField";
const DOCUMENT_REL_ALIAS: &str = "moqui.entity.document.DataDocumentRelAlias";
const DOCUMENT_CONDITION: &str = "moqui.entity.document.DataDocumentCondition";

/// A document as a nested map, control fields first.
pub type Document = IndexMap<String, Value>;

/// One `DataDocument` definition, resolved against the entity model.
#[derive(Clone, Debug)]
pub struct DocumentInfo {
    pub data_document_id: String,
    /// Lower-case `indexName`, when set.
    pub index_name: Option<String>,
    pub primary_entity: String,
    /// Short alias or full name of the primary entity, the `_entity` control field.
    pub primary_entity_label: String,
    pub primary_pk: Vec<String>,
    pub manual_data_service: Option<String>,
    pub manual_mapping_service: Option<String>,
    pub root: FieldNode,
    pub conditions: Vec<DocCondition>,
    pub has_expression: bool,
}

/// Fields of one entity in the tree, and the relationships below it.
#[derive(Clone, Debug)]
pub struct FieldNode {
    /// Full name of the entity this node reads.
    pub entity: String,
    /// Member alias in the document query.
    pub member_alias: String,
    pub fields: Vec<DocField>,
    pub relationships: Vec<RelNode>,
}

#[derive(Clone, Debug)]
pub struct DocField {
    /// Entity field name, or the expression text for an expression field.
    pub path_element: String,
    /// Name in the document.
    pub alias: String,
    /// `DataDocumentField.fieldType`: a search mapping type such as `keyword` or `double`.
    pub field_type: Option<String>,
    /// Dictionary type of the entity field. `None` for an expression.
    pub entity_type: Option<String>,
    pub sortable: bool,
    pub function: Option<String>,
    pub is_expression: bool,
    /// `searchRole`: title, subtitle, code, category, or quality.
    pub search_role: Option<String>,
    /// `searchWeight`: A, B, C, or D.
    pub search_weight: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RelNode {
    pub relationship: String,
    /// Key in the parent map for a type-many relationship.
    pub document_alias: String,
    pub type_one: bool,
    pub node: FieldNode,
}

#[derive(Clone, Debug)]
pub struct DocCondition {
    pub field_alias: String,
    pub operator: String,
    pub field_value: Option<String>,
    pub to_field_alias: Option<String>,
    pub post_query: bool,
}

#[derive(Default)]
struct InfoCache {
    /// The feed definition generation the infos were read at. See [`super::cache::feed_definition_generation`].
    generation: u64,
    infos: HashMap<String, Arc<DocumentInfo>>,
}

impl DocumentInfo {
    /// The document id for a query row, from the primary key aliases.
    fn doc_id(&self, row: &BTreeMap<String, Value>) -> String {
        self.primary_pk
            .iter()
            .map(|name| row.get(&pk_alias(name)).map(to_plain_string).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Every field node with the relationship path that reaches it.
    pub fn walk(&self, mut visit: impl FnMut(&[&RelNode], &FieldNode)) {
        fn step<'a>(path: &mut Vec<&'a RelNode>, node: &'a FieldNode, visit: &mut dyn FnMut(&[&RelNode], &FieldNode)) {
            visit(path, node);
            for rel in &node.relationships {
                path.push(rel);
                step(path, &rel.node, visit);
                path.pop();
            }
        }
        let mut path = Vec::new();
        step(&mut path, &self.root, &mut visit);
    }
}

/// Load and cache a document definition. A write to a `moqui.entity.document` entity, or
/// [`clear_document_cache`], drops the cache.
///
/// # Errors
///
/// Returns an error when the document, its primary entity, a relationship, or a field is not
/// defined.
pub fn document_info(facade: &EntityFacade, data_document_id: &str) -> Result<Arc<DocumentInfo>, Error> {
    let generation = super::cache::feed_definition_generation(facade);
    let found = facade.with_slot(InfoCache::default, |cache| {
        if cache.generation != generation {
            cache.infos.clear();
            cache.generation = generation;
        }
        cache.infos.get(data_document_id).cloned()
    });
    if let Some(found) = found {
        return Ok(found);
    }
    let info = Arc::new(load_info(facade, data_document_id)?);
    facade.with_slot(InfoCache::default, |cache| {
        if cache.generation == generation {
            cache.infos.insert(data_document_id.to_string(), Arc::clone(&info));
        }
    });
    Ok(info)
}

/// Forget cached document definitions, after a `DataDocument` record changes.
pub fn clear_document_cache(facade: &EntityFacade) {
    facade.with_slot(InfoCache::default, |cache| cache.infos.clear());
}

/// Document ids whose `indexName` is `index_name`, case-insensitive.
///
/// # Errors
///
/// Returns an error when the `DataDocument` table cannot be read.
pub fn documents_in_index(facade: &EntityFacade, index_name: &str) -> Result<Vec<String>, Error> {
    let wanted = index_name.to_ascii_lowercase();
    let rows = facade.find(DOCUMENT)?.order_by("dataDocumentId", false).list()?;
    Ok(rows
        .iter()
        .filter(|row| text(row.get("indexName")).is_some_and(|name| name.to_ascii_lowercase() == wanted))
        .filter_map(|row| text(row.get("dataDocumentId")))
        .collect())
}

/// `getDataDocuments`. `condition` names document field aliases. `from` and `thru` keep documents
/// where any member's `lastUpdatedStamp` is in `[from, thru)` or null.
///
/// # Errors
///
/// Returns an error when the definition is bad or the query fails.
pub fn data_documents(
    facade: &EntityFacade,
    data_document_id: &str,
    condition: Option<Condition>,
    from: Option<Timestamp>,
    thru: Option<Timestamp>,
) -> Result<Vec<Document>, Error> {
    let info = document_info(facade, data_document_id)?;
    let stamp = thru.unwrap_or_else(moqui_lib::clock::now);
    let rows = query_rows(facade, &info, condition, from, thru, LimitOffset::default())?;
    let docs = merge_rows(&info, &rows, stamp);
    post_process(facade, &info, docs)
}

/// Documents for the given `_id` values. An id with no matching document is left out.
///
/// # Errors
///
/// Returns an error when the definition is bad or the query fails.
pub fn documents_by_ids(facade: &EntityFacade, data_document_id: &str, ids: &[String]) -> Result<Vec<Document>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let info = document_info(facade, data_document_id)?;
    let mut out = Vec::new();
    for chunk in ids.chunks(200) {
        let condition = ids_condition(&info, chunk)?;
        out.extend(data_documents(facade, data_document_id, Some(condition), None, None)?);
    }
    Ok(out)
}

/// `feedDataDocuments`: read documents in primary-key order and hand them to `sink` in batches of
/// `batch_size`. `sink` returns false to stop. Returns how many documents were read.
///
/// # Errors
///
/// Returns an error when the definition is bad, a query fails, or `sink` fails.
pub fn feed_documents(
    facade: &EntityFacade,
    data_document_id: &str,
    from: Option<Timestamp>,
    thru: Option<Timestamp>,
    batch_size: usize,
    mut sink: impl FnMut(Vec<Document>) -> Result<bool, Error>,
) -> Result<usize, Error> {
    let info = document_info(facade, data_document_id)?;
    let batch_size = batch_size.max(1);
    let stamp = thru.unwrap_or_else(moqui_lib::clock::now);
    let mut after: Option<Vec<Value>> = None;
    let mut count = 0usize;
    loop {
        // Page on the primary entity alone, so one document never splits across pages. Each page starts after the
        // last key of the one before, not at an offset, so a row written while the pages are read shifts nothing:
        // no row is read twice or skipped.
        let mut find = facade.find(&info.primary_entity)?;
        for name in &info.primary_pk {
            find = find.select_field(name).order_by(name, false);
        }
        if let Some(last) = &after {
            find = find.condition(after_key(&info.primary_pk, last));
        }
        let keys = find.limit(i64::try_from(batch_size).unwrap_or(i64::MAX)).list()?;
        let Some(last) = keys.iter().last() else {
            break;
        };
        after = Some(
            info.primary_pk
                .iter()
                .map(|name| last.get(name).cloned().unwrap_or(Value::Null))
                .collect(),
        );
        let ids: Vec<String> = keys
            .iter()
            .map(|value| {
                info.primary_pk
                    .iter()
                    .map(|name| value.get(name).map(to_plain_string).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("::")
            })
            .collect();
        let condition = ids_condition(&info, &ids)?;
        let rows = query_rows(facade, &info, Some(condition), from, thru, LimitOffset::default())?;
        let docs = post_process(facade, &info, merge_rows(&info, &rows, stamp))?;
        count += docs.len();
        if !docs.is_empty() && !sink(docs)? {
            break;
        }
        if keys.len() < batch_size {
            break;
        }
    }
    Ok(count)
}

/// Rows whose primary key sorts after `last` in the order the pages are read: `(a, b) > (la, lb)` written out as
/// `a > la OR (a = la AND b > lb)`.
fn after_key(names: &[String], last: &[Value]) -> Condition {
    let mut ors = Vec::with_capacity(names.len());
    for (at, (name, value)) in names.iter().zip(last).enumerate() {
        let mut ands: Vec<Condition> = names
            .iter()
            .zip(last)
            .take(at)
            .map(|(name, value)| Condition::equals(name.as_str(), value.clone()))
            .collect();
        ands.push(Condition::greater(name.as_str(), value.clone()));
        ors.push(Condition::and(ands));
    }
    Condition::or(ors)
}

fn ids_condition(info: &DocumentInfo, ids: &[String]) -> Result<Condition, Error> {
    if info.primary_pk.len() == 1 {
        let values = ids.iter().map(Value::string).collect();
        return Ok(Condition::is_in(FieldRef::new(pk_alias(&info.primary_pk[0])), values));
    }
    let mut ors = Vec::with_capacity(ids.len());
    for id in ids {
        let parts: Vec<&str> = id.split("::").collect();
        if parts.len() != info.primary_pk.len() {
            return Err(Error::new(format!(
                "document id {id} does not have {} key values for {}",
                info.primary_pk.len(),
                info.data_document_id
            )));
        }
        let ands = info
            .primary_pk
            .iter()
            .zip(parts)
            .map(|(name, part)| Condition::equals(FieldRef::new(pk_alias(name)), Value::string(part)))
            .collect();
        ors.push(Condition::and(ands));
    }
    Ok(Condition::or(ors))
}

fn pk_alias(name: &str) -> String {
    format!("_pk_{name}")
}

fn load_info(facade: &EntityFacade, data_document_id: &str) -> Result<DocumentInfo, Error> {
    let document = facade
        .find(DOCUMENT)?
        .eq("dataDocumentId", Value::string(data_document_id))
        .one()?
        .ok_or_else(|| Error::new(format!("No DataDocument found with ID {data_document_id}")))?;
    let mut field_rows: Vec<_> = facade
        .find(DOCUMENT_FIELD)?
        .eq("dataDocumentId", Value::string(data_document_id))
        .list()?
        .values()
        .to_vec();
    field_rows.sort_by_key(|row| {
        let seq = text(row.get("fieldSeqId")).unwrap_or_default();
        let number = match row.get("sequenceNum") {
            Some(Value::I32(number)) => i64::from(*number),
            Some(Value::I64(number)) => *number,
            _ => seq.parse::<i64>().unwrap_or(i64::MAX),
        };
        (number, text(row.get("fieldPath")).unwrap_or_default())
    });
    let mut rel_aliases = HashMap::new();
    for row in facade
        .find(DOCUMENT_REL_ALIAS)?
        .eq("dataDocumentId", Value::string(data_document_id))
        .list()?
        .iter()
    {
        if let (Some(name), Some(alias)) = (text(row.get("relationshipName")), text(row.get("documentAlias"))) {
            rel_aliases.insert(name, alias);
        }
    }
    let mut conditions = Vec::new();
    for row in facade
        .find(DOCUMENT_CONDITION)?
        .eq("dataDocumentId", Value::string(data_document_id))
        .order_by("conditionSeqId", false)
        .list()?
        .iter()
    {
        let Some(field_alias) = text(row.get("fieldNameAlias")) else {
            continue;
        };
        conditions.push(DocCondition {
            field_alias,
            operator: text(row.get("operator")).unwrap_or_else(|| "equals".to_string()),
            field_value: text(row.get("fieldValue")),
            to_field_alias: text(row.get("toFieldNameAlias")),
            post_query: text(row.get("postQuery")).as_deref() == Some("Y"),
        });
    }

    let primary_name = text(document.get("primaryEntityName"))
        .ok_or_else(|| Error::new(format!("DataDocument {data_document_id} has no primaryEntityName")))?;
    let (primary_entity, primary_label, primary_pk) = facade.with_entity(&primary_name, |entity, _, _| {
        Ok((
            entity.full_name.clone(),
            entity.short_alias.clone().unwrap_or_else(|| entity.full_name.clone()),
            entity
                .pk_field_names()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ))
    })?;
    let mut root = FieldNode {
        entity: primary_entity.clone(),
        member_alias: PRIMARY_ALIAS.to_string(),
        fields: Vec::new(),
        relationships: Vec::new(),
    };
    let mut member_count = 0usize;
    let mut has_expression = false;
    for row in &field_rows {
        let Some(path) = text(row.get("fieldPath")) else {
            continue;
        };
        let elements = field_path_to_list(&path);
        let Some((last, rels)) = elements.split_last() else {
            continue;
        };
        let mut node = &mut root;
        for rel_name in rels {
            node = child_node(
                facade,
                node,
                rel_name,
                &rel_aliases,
                &mut member_count,
                data_document_id,
            )?;
        }
        let is_expression = last.starts_with('(');
        let alias = text(row.get("fieldNameAlias")).unwrap_or_else(|| last.clone());
        if is_expression && text(row.get("fieldNameAlias")).is_none() {
            return Err(Error::new(format!(
                "DataDocument {data_document_id} expression field {path} needs a fieldNameAlias"
            )));
        }
        let entity_type = if is_expression {
            None
        } else {
            let entity_name = node.entity.clone();
            Some(facade.with_entity(&entity_name, |entity, _, _| {
                entity.field(last).map(|field| field.field_type.clone()).ok_or_else(|| {
                    Error::new(format!(
                        "Could not find field [{last}] for entity [{entity_name}] in DataDocument [{data_document_id}]"
                    ))
                })
            })?)
        };
        has_expression |= is_expression;
        node.fields.push(DocField {
            path_element: last.clone(),
            alias,
            field_type: text(row.get("fieldType")),
            entity_type,
            sortable: text(row.get("sortable")).as_deref() == Some("Y"),
            function: text(row.get("functionName")),
            is_expression,
            search_role: text(row.get("searchRole")),
            search_weight: text(row.get("searchWeight")),
        });
    }
    Ok(DocumentInfo {
        data_document_id: data_document_id.to_string(),
        index_name: text(document.get("indexName")).map(|name| name.to_ascii_lowercase()),
        primary_entity,
        primary_entity_label: primary_label,
        primary_pk,
        manual_data_service: text(document.get("manualDataServiceName")),
        manual_mapping_service: text(document.get("manualMappingServiceName")),
        root,
        conditions,
        has_expression,
    })
}

fn child_node<'a>(
    facade: &EntityFacade,
    node: &'a mut FieldNode,
    rel_name: &str,
    rel_aliases: &HashMap<String, String>,
    member_count: &mut usize,
    data_document_id: &str,
) -> Result<&'a mut FieldNode, Error> {
    if let Some(index) = node.relationships.iter().position(|rel| rel.relationship == rel_name) {
        return Ok(&mut node.relationships[index].node);
    }
    let parent = node.entity.clone();
    let (related, type_one, short_alias) = facade.with_entity(&parent, |entity, _, _| {
        let rel = entity.relationship(rel_name).ok_or_else(|| {
            Error::new(format!(
                "Could not find relationship [{rel_name}] for entity [{parent}] in DataDocument [{data_document_id}]"
            ))
        })?;
        Ok((
            rel.related.clone(),
            rel.relationship_type.starts_with("one"),
            rel.short_alias.clone(),
        ))
    })?;
    let related = facade.with_entity(&related, |entity, _, _| Ok(entity.full_name.clone()))?;
    let document_alias = rel_aliases
        .get(rel_name)
        .cloned()
        .or(short_alias)
        .unwrap_or_else(|| rel_name.to_string());
    let member_alias = format!("MBR{member_count}");
    *member_count += 1;
    node.relationships.push(RelNode {
        relationship: rel_name.to_string(),
        document_alias,
        type_one,
        node: FieldNode {
            entity: related,
            member_alias,
            fields: Vec::new(),
            relationships: Vec::new(),
        },
    });
    let last = node.relationships.len() - 1;
    Ok(&mut node.relationships[last].node)
}

/// `Rel:Rel:field`. An expression keeps its colons: `issuances:(a ? b : c)` is `issuances` and the
/// expression.
pub fn field_path_to_list(path: &str) -> Vec<String> {
    match path.find('(') {
        None => path.split(':').map(str::to_string).collect(),
        Some(0) => vec![path.to_string()],
        Some(open) => {
            let mut out: Vec<String> = path[..open]
                .trim_end_matches(':')
                .split(':')
                .map(str::to_string)
                .collect();
            out.push(path[open..].to_string());
            out
        }
    }
}

fn code_view(info: &DocumentInfo) -> CodeView {
    let mut relationships = Vec::new();
    let mut aliases = Vec::new();
    for name in &info.primary_pk {
        aliases.push(FieldAlias::column(pk_alias(name), PRIMARY_ALIAS, Some(name.clone())));
    }
    info.walk(|path, node| {
        if let Some(rel) = path.last() {
            let parent = if path.len() > 1 {
                path[path.len() - 2].node.member_alias.clone()
            } else {
                PRIMARY_ALIAS.to_string()
            };
            relationships.push((rel.node.member_alias.clone(), parent, rel.relationship.clone(), true));
        }
        for field in &node.fields {
            if field.is_expression {
                continue;
            }
            aliases.push(FieldAlias {
                function: field.function.clone(),
                ..FieldAlias::column(
                    field.alias.clone(),
                    node.member_alias.clone(),
                    Some(field.path_element.clone()),
                )
            });
        }
    });
    CodeView {
        name: format!("DataDocument.{}", info.data_document_id),
        primary_alias: PRIMARY_ALIAS.to_string(),
        primary_entity: info.primary_entity.clone(),
        relationships,
        aliases,
    }
}

fn query_rows(
    facade: &EntityFacade,
    info: &DocumentInfo,
    condition: Option<Condition>,
    from: Option<Timestamp>,
    thru: Option<Timestamp>,
    page: LimitOffset,
) -> Result<Vec<BTreeMap<String, Value>>, Error> {
    let view = code_view(info);
    let mut conditions = Vec::new();
    for cond in &info.conditions {
        if cond.post_query {
            continue;
        }
        conditions.push(document_condition(cond)?);
    }
    if from.is_some() || thru.is_some() {
        let mut members = vec![PRIMARY_ALIAS.to_string()];
        members.extend(view.relationships.iter().map(|(alias, ..)| alias.clone()));
        let mut ors = Vec::new();
        for member in members {
            let field = FieldRef::aliased(member, "lastUpdatedStamp");
            let mut ands = Vec::new();
            if let Some(from) = from {
                ands.push(Condition::or(vec![
                    Condition::is_null(field.clone()),
                    Condition::greater_equal(field.clone(), Value::Timestamp(from)),
                ]));
            }
            if let Some(thru) = thru {
                ands.push(Condition::or(vec![
                    Condition::is_null(field.clone()),
                    Condition::less(field, Value::Timestamp(thru)),
                ]));
            }
            ors.push(Condition::and(ands));
        }
        conditions.push(Condition::or(ors));
    }
    conditions.extend(condition);
    let condition = match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ => Some(Condition::and(conditions)),
    };
    let order = info
        .primary_pk
        .iter()
        .map(|name| OrderBy::asc(FieldRef::new(pk_alias(name))))
        .collect();
    let _authz = crate::context::ExecutionContext::with_current(|ec| ec.disable_authz());
    run_code_view(facade, &view, condition, order, page)
}

fn document_condition(cond: &DocCondition) -> Result<Condition, Error> {
    let field = FieldRef::new(cond.field_alias.clone());
    if let Some(to_alias) = &cond.to_field_alias {
        let op = match cond.operator.as_str() {
            "not-equals" => moqui_db::sql::CompareOp::NotEquals,
            "less" | "less-than" => moqui_db::sql::CompareOp::Less,
            "greater" | "greater-than" => moqui_db::sql::CompareOp::Greater,
            "less-equals" | "less-than-equal-to" => moqui_db::sql::CompareOp::LessEqual,
            "greater-equals" | "greater-than-equal-to" => moqui_db::sql::CompareOp::GreaterEqual,
            _ => moqui_db::sql::CompareOp::Equals,
        };
        return Ok(Condition::compare_fields(field, op, FieldRef::new(to_alias.clone())));
    }
    let value = cond.field_value.clone().map_or(Value::Null, Value::String);
    let list = || -> Vec<Value> {
        cond.field_value
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(|part| Value::string(part.trim()))
            .collect()
    };
    Ok(match cond.operator.as_str() {
        "equals" | "" => Condition::equals(field, value),
        "not-equals" => Condition::not_equals(field, value),
        "less" | "less-than" => Condition::less(field, value),
        "greater" | "greater-than" => Condition::greater(field, value),
        "less-equals" | "less-than-equal-to" => Condition::less_equal(field, value),
        "greater-equals" | "greater-than-equal-to" => Condition::greater_equal(field, value),
        "in" => Condition::is_in(field, list()),
        "not-in" => Condition::not_in(field, list()),
        "like" => Condition::like(field, value),
        "not-like" => Condition::not_like(field, value),
        "is-null" => Condition::is_null(field),
        "is-not-null" => Condition::is_not_null(field),
        other => {
            return Err(Error::new(format!(
                "DataDocument condition operator {other} on {} is not supported",
                cond.field_alias
            )));
        }
    })
}

/// Formats like Java's `DateTimeFormatter.ISO_INSTANT`, which prints the fraction in groups of three digits
/// (`.840`, not `.84`).
fn iso_instant(stamp: Timestamp) -> String {
    let seconds = stamp.strftime("%Y-%m-%dT%H:%M:%S");
    let nanos = stamp.subsec_nanosecond();
    if nanos == 0 {
        format!("{seconds}Z")
    } else if nanos % 1_000_000 == 0 {
        format!("{seconds}.{:03}Z", nanos / 1_000_000)
    } else if nanos % 1_000 == 0 {
        format!("{seconds}.{:06}Z", nanos / 1_000)
    } else {
        format!("{seconds}.{nanos:09}Z")
    }
}

fn merge_rows(info: &DocumentInfo, rows: &[BTreeMap<String, Value>], stamp: Timestamp) -> Vec<Document> {
    let stamp_text = iso_instant(stamp);
    let mut docs: IndexMap<String, Document> = IndexMap::new();
    for row in rows {
        let id = info.doc_id(row);
        let doc = docs.entry(id.clone()).or_insert_with(|| {
            let mut doc = Document::new();
            doc.insert("_type".to_string(), Value::string(&info.data_document_id));
            if !id.is_empty() {
                doc.insert("_id".to_string(), Value::string(&id));
            }
            doc.insert("_timestamp".to_string(), Value::string(&stamp_text));
            if let Some(index) = &info.index_name {
                doc.insert("_index".to_string(), Value::string(index));
            }
            doc.insert("_entity".to_string(), Value::string(&info.primary_entity_label));
            set_fields(&info.root, row, &mut doc);
            doc
        });
        populate_related(&info.root, row, doc);
    }
    docs.into_values().collect()
}

fn set_fields(node: &FieldNode, row: &BTreeMap<String, Value>, target: &mut Document) {
    for field in &node.fields {
        if field.is_expression {
            continue;
        }
        if let Some(value) = row.get(&field.alias) {
            if !value.is_null() {
                target.insert(field.alias.clone(), value.clone());
            }
        }
    }
}

/// `populateDataDocRelatedMap`: type-one children set fields on `target`; a type-many child
/// joins the list entry whose direct fields match this row, or adds one.
fn populate_related(node: &FieldNode, row: &BTreeMap<String, Value>, target: &mut Document) {
    for rel in &node.relationships {
        if rel.type_one {
            set_fields(&rel.node, row, target);
            populate_related(&rel.node, row, target);
            continue;
        }
        let list = match target.get_mut(&rel.document_alias) {
            Some(Value::List(list)) => Some(list),
            _ => None,
        };
        let matched = list.and_then(|list| {
            list.iter_mut().find(|candidate| {
                let Value::Map(candidate) = candidate else {
                    return false;
                };
                rel.node
                    .fields
                    .iter()
                    .filter(|field| !field.is_expression)
                    .all(|field| {
                        let left = candidate.get(&field.alias).unwrap_or(&Value::Null);
                        let right = row.get(&field.alias).unwrap_or(&Value::Null);
                        left == right
                    })
            })
        });
        match matched {
            Some(Value::Map(existing)) => populate_related(&rel.node, row, existing),
            _ => {
                let mut child = Document::new();
                set_fields(&rel.node, row, &mut child);
                populate_related(&rel.node, row, &mut child);
                if child.is_empty() {
                    continue;
                }
                match target.get_mut(&rel.document_alias) {
                    Some(Value::List(list)) => list.push(Value::Map(child)),
                    _ => {
                        target.insert(rel.document_alias.clone(), Value::List(vec![Value::Map(child)]));
                    }
                }
            }
        }
    }
}

fn post_process(facade: &EntityFacade, info: &DocumentInfo, docs: Vec<Document>) -> Result<Vec<Document>, Error> {
    let mut out = Vec::with_capacity(docs.len());
    for mut doc in docs {
        if let Some(service) = &info.manual_data_service {
            doc = manual_data(service, info, doc);
        }
        if info.has_expression {
            run_expressions(facade, &info.root, &mut doc, None);
        }
        if post_conditions_pass(info, &doc) {
            out.push(doc);
        }
    }
    Ok(out)
}

fn manual_data(service: &str, info: &DocumentInfo, doc: Document) -> Document {
    let mut params = BTreeMap::new();
    params.insert("dataDocumentId".to_string(), Value::string(&info.data_document_id));
    params.insert("document".to_string(), Value::Map(doc.clone()));
    match crate::service::call::sync(service, &params) {
        Ok(result) => match result.get("document") {
            Some(Value::Map(out)) if !out.is_empty() => out.clone(),
            _ => doc,
        },
        Err(err) => {
            tracing::error!(
                document = %info.data_document_id,
                service,
                error = %err,
                "Error calling manual data service, document may be missing data"
            );
            doc
        }
    }
}

/// `runDocExpressions`: an expression sees its own map plus every parent map above it.
fn run_expressions(facade: &EntityFacade, node: &FieldNode, doc: &mut Document, parents: Option<&Document>) {
    for rel in &node.relationships {
        if rel.type_one {
            run_expressions(facade, &rel.node, doc, parents);
            continue;
        }
        let mut merged = parents.cloned().unwrap_or_default();
        for (key, value) in doc.iter() {
            merged.insert(key.clone(), value.clone());
        }
        if let Some(Value::List(children)) = doc.get_mut(&rel.document_alias) {
            for child in children.iter_mut() {
                if let Value::Map(child) = child {
                    run_expressions(facade, &rel.node, child, Some(&merged));
                }
            }
        }
    }
    for field in &node.fields {
        if !field.is_expression {
            continue;
        }
        let _frame = crate::script::ContextFrame::push();
        let mut ok = true;
        for (key, value) in parents.into_iter().flat_map(|map| map.iter()).chain(doc.iter()) {
            if crate::script::set_context_name(key, value.clone()).is_err() {
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        match crate::script::eval_value(&field.path_element) {
            Ok(Value::Null) => {}
            Ok(value) => {
                doc.insert(field.alias.clone(), expression_value(field, value));
            }
            Err(err) => tracing::error!(
                expression = %field.path_element,
                error = %err,
                "Error evaluating DataDocumentField expression"
            ),
        }
    }
    let _ = facade;
}

/// A `date` expression field's value as a timestamp: a date or timestamp reaches a script as epoch millis, so an
/// expression that returns one returns millis, as text or a number.
fn expression_value(field: &DocField, value: Value) -> Value {
    if field.field_type.as_deref() != Some("date") {
        return value;
    }
    let millis = match &value {
        Value::String(text) => text.trim().parse::<i64>().ok(),
        Value::I64(millis) => Some(*millis),
        Value::I32(millis) => Some(i64::from(*millis)),
        Value::F64(millis) if millis.fract() == 0.0 => Some(*millis as i64),
        _ => None,
    };
    match millis {
        Some(millis) => moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value,
        None => value,
    }
}

/// A post-query condition must find `fieldValue` among the values of that alias anywhere in the
/// document. With no `fieldValue`, the alias must be absent.
fn post_conditions_pass(info: &DocumentInfo, doc: &Document) -> bool {
    for cond in info.conditions.iter().filter(|cond| cond.post_query) {
        let mut found = Vec::new();
        collect_values(&cond.field_alias, doc, &mut found);
        match &cond.field_value {
            None => {
                if !found.is_empty() {
                    return false;
                }
            }
            Some(expected) => {
                if !found.iter().any(|value| &to_plain_string(value) == expected) {
                    return false;
                }
            }
        }
    }
    true
}

fn collect_values(name: &str, map: &Document, out: &mut Vec<Value>) {
    for (key, value) in map {
        match value {
            Value::Map(child) => collect_values(name, child, out),
            Value::List(items) => {
                for item in items {
                    if let Value::Map(child) = item {
                        collect_values(name, child, out);
                    }
                }
            }
            other if key == name && !other.is_null() => out.push(other.clone()),
            _ => {}
        }
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Null) | None => None,
        Some(value) => {
            let text = to_plain_string(value);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{field_path_to_list, iso_instant};

    #[test]
    fn iso_instant_prints_fraction_in_groups_of_three() {
        let at = |text: &str| iso_instant(text.parse().unwrap());
        assert_eq!(at("2026-10-23T19:27:18.84Z"), "2026-10-23T19:27:18.840Z");
        assert_eq!(at("2026-10-23T19:27:18Z"), "2026-10-23T19:27:18Z");
        assert_eq!(at("2026-10-23T19:27:18.1234Z"), "2026-10-23T19:27:18.123400Z");
        assert_eq!(at("2026-10-23T19:27:18.000000001Z"), "2026-10-23T19:27:18.000000001Z");
    }

    #[test]
    fn a_date_expression_returning_epoch_millis_is_a_timestamp() {
        use moqui_lib::object::Value;
        let field = |field_type: &str| super::DocField {
            path_element: "(x)".into(),
            alias: "x".into(),
            field_type: Some(field_type.into()),
            entity_type: None,
            sortable: false,
            function: None,
            is_expression: true,
            search_role: None,
            search_weight: None,
        };
        let stamp = super::expression_value(&field("date"), Value::string("1791979200000"));
        assert!(matches!(stamp, Value::Timestamp(_)), "{stamp:?}");
        assert!(matches!(
            super::expression_value(&field("date"), Value::F64(1791979200000.0)),
            Value::Timestamp(_)
        ));
        assert_eq!(
            super::expression_value(&field("double"), Value::string("12")),
            Value::string("12")
        );
    }

    #[test]
    fn field_paths_keep_expression_colons() {
        assert_eq!(field_path_to_list("orderId"), vec!["orderId"]);
        assert_eq!(
            field_path_to_list("part:vendorRoles:roleTypeId"),
            vec!["part", "vendorRoles", "roleTypeId"]
        );
        assert_eq!(
            field_path_to_list("issuances:(acquireCost ? issuedQuantity * acquireCost : null)"),
            vec!["issuances", "(acquireCost ? issuedQuantity * acquireCost : null)"]
        );
        assert_eq!(field_path_to_list("(a ?: b)"), vec!["(a ?: b)"]);
    }

    fn create_row(facade: &crate::entity::EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, moqui_lib::object::Value::string(*value));
        }
        row.create(facade).unwrap();
    }

    #[test]
    fn feed_pages_by_key_so_rows_written_meanwhile_are_not_skipped_or_repeated() {
        let facade = crate::testing::open();
        create_row(
            &facade,
            super::DOCUMENT,
            &[
                ("dataDocumentId", "PAGED"),
                ("primaryEntityName", "moqui.test.TestEntity"),
            ],
        );
        create_row(
            &facade,
            super::DOCUMENT_FIELD,
            &[
                ("dataDocumentId", "PAGED"),
                ("fieldSeqId", "01"),
                ("fieldPath", "testMedium"),
            ],
        );
        for id in ["A", "C", "E", "G"] {
            create_row(&facade, "moqui.test.TestEntity", &[("testId", id), ("testMedium", id)]);
        }
        let mut seen = Vec::new();
        let mut pages = 0;
        let count = super::feed_documents(&facade, "PAGED", None, None, 2, |docs| {
            pages += 1;
            seen.extend(
                docs.iter()
                    .map(|doc| doc.get("_id").map(super::to_plain_string).unwrap_or_default()),
            );
            if pages == 1 {
                // One row before the read position, which an offset would read again, and one after it.
                create_row(
                    &facade,
                    "moqui.test.TestEntity",
                    &[("testId", "B"), ("testMedium", "B")],
                );
                create_row(
                    &facade,
                    "moqui.test.TestEntity",
                    &[("testId", "F"), ("testMedium", "F")],
                );
            }
            Ok(true)
        })
        .unwrap();
        assert_eq!(seen, ["A", "C", "E", "F", "G"]);
        assert_eq!(count, 5);
    }

    #[test]
    fn a_document_definition_write_drops_the_cached_info() {
        let facade = crate::testing::open();
        create_row(
            &facade,
            super::DOCUMENT,
            &[
                ("dataDocumentId", "CACHED"),
                ("primaryEntityName", "moqui.test.TestEntity"),
            ],
        );
        let before = super::document_info(&facade, "CACHED").unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &before,
            &super::document_info(&facade, "CACHED").unwrap()
        ));
        create_row(
            &facade,
            super::DOCUMENT_FIELD,
            &[
                ("dataDocumentId", "CACHED"),
                ("fieldSeqId", "01"),
                ("fieldPath", "testMedium"),
            ],
        );
        let after = super::document_info(&facade, "CACHED").unwrap();
        assert!(!std::sync::Arc::ptr_eq(&before, &after), "the field write was seen");
    }
}
