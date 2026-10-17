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

//! What the tools entity screens read about the model, from `EntityFacadeImpl` and `EntityDefinition`: the entity
//! names, the entity list with its filters, the counts by package, one definition with its fields, relationships,
//! and dependents, the data source groups, the EECA rules, and the SQL runner's statement.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use moqui_db::model::{EntityDefinition, EntityModel, Relationship};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::facade::EntityFacade;
use crate::transaction::TransactionAttribute;

/// `(?i).*filter.*`, as the Java list filters write it. The matcher has no inline flags, so the case is folded on both
/// sides when the filter has no escape. A filter that is not a pattern the matcher reads is a plain substring.
pub fn name_matches(name: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let pattern = format!(".*{filter}.*");
    if let Ok(found) = moqui_lib::object::regex_full_match(name, &pattern) {
        if found {
            return true;
        }
        if !filter.contains('\\') {
            let folded = format!(".*{}.*", filter.to_lowercase());
            return moqui_lib::object::regex_full_match(&name.to_lowercase(), &folded).unwrap_or(false);
        }
        return false;
    }
    name.to_lowercase().contains(&filter.to_lowercase())
}

/// `isEntityDefined`: a full name, a short name, or a short alias of a defined entity, or a DB view entity. As Java's, a
/// `DataDocument.<id>` name is not one, though a find on it works.
pub fn is_entity_defined(facade: &EntityFacade, name: &str) -> bool {
    !name.is_empty()
        && (facade.with_model(|model| model.entity(name).is_ok())
            || (!name.starts_with("DataDocument.") && facade.with_entity(name, |_, _, _| Ok(())).is_ok()))
}

/// `getAllEntityNames(filterRegexp)`: every full entity name, sorted.
pub fn all_entity_names(facade: &EntityFacade, filter: &str) -> Vec<String> {
    facade.with_model(|model| {
        let names: BTreeSet<String> = model
            .entities()
            .iter()
            .map(|entity| entity.full_name.clone())
            .filter(|name| name.contains('.') && name_matches(name, filter))
            .collect();
        names.into_iter().collect()
    })
}

fn text(value: &str) -> Value {
    Value::string(value)
}

/// `getAllEntitiesInfo`: one map for each entity (`entityName`, `package`, `isView`, `fullEntityName`, `tableName`),
/// ordered by `order_by` (a field, `-` for descending). A master-only list keeps the entities that have dependents, are
/// not a type, enumeration, or status entity, and have one primary key field.
pub fn all_entities_info(
    facade: &EntityFacade,
    order_by: &str,
    filter: &str,
    master_only: bool,
    exclude_views: bool,
) -> Vec<IndexMap<String, Value>> {
    let names = all_entity_names(facade, filter);
    let mut rows: Vec<IndexMap<String, Value>> = facade.with_model(|model| {
        names
            .iter()
            .filter_map(|name| model.entity_full(name))
            .filter(|entity| !(exclude_views && entity.is_view))
            .filter(|entity| {
                !master_only
                    || (entity.has_dependents
                        && !entity.full_name.ends_with("Type")
                        && entity.full_name != "moqui.basic.Enumeration"
                        && entity.full_name != "moqui.basic.StatusItem"
                        && entity.pk_field_names().len() <= 1)
            })
            .map(|entity| {
                let mut row = IndexMap::new();
                row.insert("entityName".to_string(), text(&entity.entity_name));
                row.insert("package".to_string(), text(&entity.package_name));
                row.insert(
                    "isView".to_string(),
                    text(if entity.is_view { "true" } else { "false" }),
                );
                row.insert("fullEntityName".to_string(), text(&entity.full_name));
                row.insert("tableName".to_string(), text(&entity.table_name));
                row
            })
            .collect()
    });
    order_rows(&mut rows, order_by);
    rows
}

/// Sort maps by one field as `CollectionUtilities.orderMapList` does: `-` descends, `^` and `+` are dropped.
fn order_rows(rows: &mut [IndexMap<String, Value>], order_by: &str) {
    let mut field = order_by.trim();
    let descending = field.starts_with('-');
    field = field.trim_start_matches(['-', '+', '^']);
    if field.is_empty() {
        return;
    }
    rows.sort_by(|left, right| {
        let left = left
            .get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        let right = right
            .get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        if descending {
            right.cmp(&left)
        } else {
            left.cmp(&right)
        }
    });
}

/// `getAllEntityInfo(levels, excludeViewEntities)`: entity and view counts by the first `levels` parts of the name.
pub fn all_entity_info(facade: &EntityFacade, levels: usize, exclude_views: bool) -> Vec<IndexMap<String, Value>> {
    let mut counts: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    facade.with_model(|model| {
        for name in all_entity_names(facade, "") {
            let Some(entity) = model.entity_full(&name) else {
                continue;
            };
            if exclude_views && entity.is_view {
                continue;
            }
            let entry = counts.entry(name_prefix(&name, levels)).or_default();
            if entity.is_view {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
    });
    counts
        .into_iter()
        .map(|(name, (entities, views))| {
            let mut row = IndexMap::new();
            row.insert("name".to_string(), Value::string(name));
            row.insert("entities".to_string(), Value::I64(entities));
            row.insert("viewEntities".to_string(), Value::I64(views));
            row
        })
        .collect()
}

/// The name up to the dot after `levels` parts, as the Java counts group it: `indexOf(".", last + 1)` each level, and
/// the whole name when the last search finds no dot. As in Java, a search after a miss starts over at the first dot.
pub fn name_prefix(name: &str, levels: usize) -> String {
    let mut last: Option<usize> = Some(0);
    for _ in 0..levels {
        let from = last.map_or(0, |at| at + 1);
        last = name.get(from..).and_then(|rest| rest.find('.')).map(|at| from + at);
    }
    match last {
        Some(at) => name[..at].to_string(),
        None => name.to_string(),
    }
}

/// `getPrettyName(title, baseName)`: the entity name split at capitals, without the parts of `base_name`, then the title.
pub fn pretty_name(entity_name: &str, title: Option<&str>, base_name: Option<&str>) -> String {
    let split = |text: &str| -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();
        for ch in text.chars() {
            if ch.is_uppercase() || parts.is_empty() {
                parts.push(String::new());
            }
            if let Some(last) = parts.last_mut() {
                last.push(ch);
            }
        }
        parts
    };
    let base: BTreeSet<String> = base_name
        .map(|base| split(base).into_iter().collect())
        .unwrap_or_default();
    let mut pretty: Vec<String> = split(entity_name)
        .into_iter()
        .filter(|part| !base.contains(part))
        .collect();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        let title_text = split(title).join(" ");
        if pretty.is_empty() {
            pretty.push(title_text);
        } else {
            pretty.push(format!("({title_text})"));
        }
    }
    if pretty.is_empty() {
        return moqui_lib::string::camel_case_to_pretty(entity_name);
    }
    pretty.join(" ")
}

/// Java's `RelationshipInfo.dependent` (`hasReverse`): the related entity has a `one` or `one-nofk` relationship back
/// to `entity` with the same title, whatever this relationship's type, so a one-to-one is a dependent too.
pub(super) fn is_dependent(model: &EntityModel, entity: &EntityDefinition, relationship: &Relationship) -> bool {
    let Some(related) = model.entity_full(&relationship.related) else {
        return false;
    };
    let title = relationship.title.as_deref().filter(|title| !title.is_empty());
    related.relationships.iter().any(|back| {
        matches!(back.relationship_type.as_str(), "one" | "one-nofk")
            && back.title.as_deref().filter(|title| !title.is_empty()) == title
            && (back.related == entity.full_name || back.related_entity_name == entity.entity_name)
    })
}

fn relationship_info(model: &EntityModel, entity: &EntityDefinition, relationship: &Relationship) -> Value {
    let related_short = model.entity_full(&relationship.related).map_or_else(
        || relationship.related_entity_name.clone(),
        |related| related.entity_name.clone(),
    );
    let key_map: IndexMap<String, Value> = relationship
        .key_pairs
        .iter()
        .map(|(field, related)| (field.clone(), Value::string(related)))
        .collect();
    let mut row = IndexMap::new();
    row.insert("type".to_string(), text(&relationship.relationship_type));
    row.insert(
        "isTypeOne".to_string(),
        Value::Bool(relationship.relationship_type.starts_with("one")),
    );
    row.insert(
        "title".to_string(),
        relationship.title.as_deref().map_or(Value::Null, text),
    );
    row.insert("relatedEntityName".to_string(), text(&relationship.related));
    row.insert("relationshipName".to_string(), text(&relationship.name));
    row.insert(
        "shortAlias".to_string(),
        relationship.short_alias.as_deref().map_or(Value::Null, text),
    );
    row.insert(
        "prettyName".to_string(),
        Value::string(pretty_name(
            &related_short,
            relationship.title.as_deref(),
            Some(&entity.entity_name),
        )),
    );
    row.insert("keyMap".to_string(), Value::Map(key_map));
    row.insert(
        "dependent".to_string(),
        Value::Bool(is_dependent(model, entity, relationship)),
    );
    row.insert("mutable".to_string(), Value::Bool(relationship.mutable));
    row.insert("isAutoReverse".to_string(), Value::Bool(relationship.is_auto_reverse));
    Value::Map(row)
}

/// `EntityDependents`: the text of the dependent tree, as `toString` writes it, and every descendant's name.
fn dependents(model: &EntityModel, entity: &EntityDefinition) -> (String, Vec<String>) {
    let mut text_out = String::new();
    let mut visited = BTreeSet::new();
    let mut all = BTreeSet::new();
    let mut ancestors = vec![entity.full_name.clone()];
    build_dependents(model, entity, 0, &mut text_out, &mut visited, &mut ancestors);
    // Java's `getAllDescendants`: every dependent reached, at any depth (the text stops at level 4).
    let mut pending = vec![entity.full_name.clone()];
    let mut walked = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !walked.insert(name.clone()) {
            continue;
        }
        let Some(current) = model.entity_full(&name) else {
            continue;
        };
        for relationship in current
            .relationships
            .iter()
            .filter(|rel| is_dependent(model, current, rel))
        {
            all.insert(relationship.related.clone());
            pending.push(relationship.related.clone());
        }
    }
    (text_out, all.into_iter().collect())
}

fn build_dependents(
    model: &EntityModel,
    entity: &EntityDefinition,
    level: usize,
    out: &mut String,
    visited: &mut BTreeSet<String>,
    ancestors: &mut Vec<String>,
) {
    let indent = "- ".repeat(level + 1);
    for relationship in entity
        .relationships
        .iter()
        .filter(|rel| is_dependent(model, entity, rel))
    {
        let pairs: Vec<String> = relationship
            .key_pairs
            .iter()
            .map(|(field, related)| format!("{field}:{related}"))
            .collect();
        out.push_str(&format!("{indent}{} [{}]\n", relationship.name, pairs.join(", ")));
        let Some(related) = model.entity_full(&relationship.related) else {
            continue;
        };
        if ancestors.contains(&related.full_name) {
            continue;
        }
        if level < 4 && !visited.contains(&related.full_name) {
            ancestors.push(related.full_name.clone());
            build_dependents(model, related, level + 1, out, visited, ancestors);
            ancestors.pop();
            visited.insert(related.full_name.clone());
        } else if visited.contains(&related.full_name) {
            out.push_str(&format!("{indent}- Dependants already displayed\n"));
        } else {
            out.push_str(&format!("{indent}- Reached level limit\n"));
        }
    }
}

/// One entity as the tools screens read an `EntityDefinition`: the names, the flags of `entityInfo`, the fields, the
/// relationships, the master definitions, the dependents, and the entity element's attributes. `None` when the entity
/// is not defined. A DB view entity or `DataDocument.<id>` is one too.
pub fn definition_info(facade: &EntityFacade, name: &str) -> Option<IndexMap<String, Value>> {
    let found = facade.with_entity(name, |entity, _, model| {
        let mut map = IndexMap::new();
        map.insert("fullEntityName".to_string(), text(&entity.full_name));
        map.insert("entityName".to_string(), text(&entity.entity_name));
        map.insert("packageName".to_string(), text(&entity.package_name));
        map.insert("isViewEntity".to_string(), Value::Bool(entity.is_view));
        map.insert("tableName".to_string(), text(&entity.table_name));
        map.insert(
            "groupName".to_string(),
            entity.group.as_deref().map_or(Value::Null, text),
        );
        map.insert(
            "shortAlias".to_string(),
            entity.short_alias.as_deref().map_or(Value::Null, text),
        );
        let needs_audit = needs_audit_log(entity);
        map.insert(
            "needsEncrypt".to_string(),
            Value::Bool(entity.fields.iter().any(|field| field.encrypt)),
        );
        map.insert("needsAuditLog".to_string(), Value::Bool(needs_audit));
        map.insert("createOnly".to_string(), Value::Bool(entity.create_only));
        map.insert("cache".to_string(), text(&entity.cache));
        map.insert(
            "pkFieldNames".to_string(),
            Value::List(entity.pk_field_names().into_iter().map(Value::string).collect()),
        );
        map.insert(
            "allFieldNames".to_string(),
            Value::List(entity.field_names().into_iter().map(Value::string).collect()),
        );
        let fields = entity
            .fields
            .iter()
            .map(|field| {
                let mut row = IndexMap::new();
                row.insert("name".to_string(), text(&field.name));
                row.insert("type".to_string(), text(&field.field_type));
                row.insert("columnName".to_string(), text(&field.column_name));
                row.insert("javaType".to_string(), text(&field.java_type));
                row.insert("isPk".to_string(), Value::Bool(field.is_pk));
                row.insert(
                    "enableAuditLog".to_string(),
                    text(field.enable_audit_log.as_deref().unwrap_or("false")),
                );
                row.insert("encrypt".to_string(), Value::Bool(field.encrypt));
                row.insert("enableLocalization".to_string(), Value::Bool(field.enable_localization));
                row.insert(
                    "defaultStr".to_string(),
                    field.default_value.as_deref().map_or(Value::Null, text),
                );
                row.insert("createOnly".to_string(), Value::Bool(field.create_only));
                row.insert("notNull".to_string(), Value::Bool(field.not_null));
                Value::Map(row)
            })
            .collect();
        map.insert("fields".to_string(), Value::List(fields));
        map.insert(
            "relationships".to_string(),
            Value::List(
                entity
                    .relationships
                    .iter()
                    .map(|relationship| relationship_info(model, entity, relationship))
                    .collect(),
            ),
        );
        map.insert(
            "masterNames".to_string(),
            Value::List(
                entity
                    .masters
                    .iter()
                    .map(|master| Value::string(&master.name))
                    .collect(),
            ),
        );
        let (tree, descendants) = dependents(model, entity);
        map.insert("dependentsText".to_string(), Value::string(tree));
        map.insert(
            "allDescendants".to_string(),
            Value::List(descendants.into_iter().map(Value::string).collect()),
        );
        let mut attributes = IndexMap::new();
        attributes.insert("entity-name".to_string(), text(&entity.entity_name));
        attributes.insert("package".to_string(), text(&entity.package_name));
        if let Some(group) = &entity.group {
            attributes.insert("group-name".to_string(), text(group));
        }
        if let Some(audit) = &entity.enable_audit_log {
            attributes.insert("enable-audit-log".to_string(), text(audit));
        }
        attributes.insert("cache".to_string(), text(&entity.cache));
        attributes.insert("use".to_string(), text(&entity.entity_use));
        if entity.has_dependents {
            attributes.insert("has-dependents".to_string(), text("true"));
        }
        map.insert("entityAttributes".to_string(), Value::Map(attributes));
        Ok(map)
    });
    found.ok()
}

/// The entity group names, sorted: `getDatasourceGroupNames`.
pub fn datasource_group_names(facade: &EntityFacade) -> Vec<String> {
    let mut names: Vec<String> = facade.inner.groups.keys().cloned().collect();
    names.sort();
    names
}

/// What one statement of the SQL runner gave: the columns and rows of a query, or the count of an update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub records: Vec<Vec<Value>>,
    pub limit_reached: bool,
    /// For a statement that is not a query.
    pub rows_affected: Option<u64>,
}

fn first_word(sql: &str) -> String {
    sql.trim_start()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A statement whose first word reads rows: `SELECT`, `WITH`, `SHOW`, `VALUES`, `EXPLAIN`, `TABLE`.
fn is_query(sql: &str) -> bool {
    matches!(
        first_word(sql).as_str(),
        "select" | "with" | "show" | "values" | "explain" | "table" | "describe" | "desc"
    )
}

/// A query a cursor can read, Postgres `DECLARE ... CURSOR FOR` taking only these. `SHOW`, `EXPLAIN` and `DESCRIBE`
/// give a few rows and are read whole.
fn is_streamable(sql: &str) -> bool {
    matches!(first_word(sql).as_str(), "select" | "with" | "values" | "table")
}

/// How long the transaction a query is read in may run: the screen begins none, so Java's statement has no timeout.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// Run one statement on the group's connection, as the SQL runner does with JDBC: in the caller's transaction when
/// there is one, otherwise on its own connection. A query reads at most `limit` rows when `limit` is above 0, through
/// a cursor in the caller's transaction or one begun for it, and stops at the row past the limit. As JDBC, the
/// database tells a query from an update (an `INSERT ... RETURNING` gives rows); a driver that cannot tell leaves it
/// to the first word of the statement. A query with no rows has no column names, since the driver gives names by row.
///
/// # Errors
///
/// Returns an error for an unknown group or when the statement fails.
pub fn run_sql(facade: &EntityFacade, group: &str, sql: &str, limit: usize) -> Result<SqlResult, Error> {
    let runtime = facade
        .inner
        .groups
        .get(group)
        .ok_or_else(|| Error::new(format!("no entity group named {group}")))?;
    let sql = sql.trim().trim_end_matches(';');
    // As JDBC's `execute`, the database says whether the statement gives rows; the first word when it cannot.
    let gives_rows = runtime
        .transactions
        .with_connection(|conn| conn.returns_rows(sql))?
        .unwrap_or_else(|| is_query(sql));
    if !gives_rows {
        let count = runtime.transactions.with_connection(|conn| conn.execute(sql, &[]))?;
        return Ok(SqlResult {
            rows_affected: Some(count),
            ..SqlResult::default()
        });
    }
    let mut result = SqlResult::default();
    let mut add = |row: crate::driver::DbRow| {
        if limit > 0 && result.records.len() >= limit {
            result.limit_reached = true;
            return false;
        }
        if result.columns.is_empty() {
            result.columns = row.columns.iter().map(|(name, _)| name.clone()).collect();
        }
        result
            .records
            .push(row.columns.into_iter().map(|(_, value)| value).collect());
        true
    };
    let tx = &runtime.transactions;
    if !is_streamable(sql) {
        for row in tx.with_connection(|conn| conn.query_once(sql, &[]))? {
            if !add(row) {
                break;
            }
        }
        return Ok(result);
    }
    tx.run(TransactionAttribute::UseOrBegin, Some(QUERY_TIMEOUT), || {
        tx.with_connection(|conn| {
            // A find's cursor already open on the caller's connection keeps it; this query is read whole.
            if conn.stream_open() {
                for row in conn.query_once(sql, &[])? {
                    if !add(row) {
                        break;
                    }
                }
                return Ok(());
            }
            conn.open_stream(sql, &[])?;
            let read = (|| {
                while let Some(row) = conn.next_row()? {
                    if !add(row) {
                        break;
                    }
                }
                Ok(())
            })();
            let closed = conn.close_stream();
            read.and(closed)
        })
    })?;
    Ok(result)
}

/// Java's `EntityInfo.needsAuditLog`: a field (with the entity's attribute as its default) says `true` or `update`.
fn needs_audit_log(entity: &EntityDefinition) -> bool {
    entity.fields.iter().any(|field| {
        field
            .enable_audit_log
            .as_deref()
            .is_some_and(|flag| matches!(flag, "true" | "update"))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn needs_audit_log_is_a_field_with_true_or_update() {
        use moqui_db::model::{EntityDefinition, FieldDefinition};
        let entity = |entity_flag: Option<&str>, field_flag: Option<&str>| EntityDefinition {
            enable_audit_log: entity_flag.map(str::to_string),
            fields: vec![FieldDefinition {
                name: "f".to_string(),
                enable_audit_log: field_flag.or(entity_flag).map(str::to_string),
                ..FieldDefinition::default()
            }],
            ..EntityDefinition::default()
        };
        assert!(super::needs_audit_log(&entity(None, Some("true"))));
        assert!(super::needs_audit_log(&entity(Some("update"), None)));
        assert!(!super::needs_audit_log(&entity(Some("false"), None)));
        assert!(!super::needs_audit_log(&entity(Some("yes"), None)));
        assert!(!super::needs_audit_log(&entity(Some("true"), Some("false"))));
    }

    #[test]
    fn names_group_by_the_leading_parts() {
        assert_eq!(super::name_prefix("moqui.basic.Enumeration", 2), "moqui.basic");
        assert_eq!(super::name_prefix("moqui.basic.Enumeration", 1), "moqui");
        // A miss starts the next search over, as Java does.
        assert_eq!(super::name_prefix("moqui.Thing", 3), "moqui");
        assert_eq!(super::name_prefix("moqui.Thing", 2), "moqui.Thing");
    }

    #[test]
    fn pretty_names_drop_the_base_parts_and_add_the_title() {
        assert_eq!(super::pretty_name("StatusItem", None, None), "Status Item");
        assert_eq!(
            super::pretty_name("StatusItem", Some("PrintJob"), Some("PrintJob")),
            "Status Item (Print Job)"
        );
        assert_eq!(super::pretty_name("UserGroupMember", None, Some("UserGroup")), "Member");
    }

    #[test]
    fn filters_fold_case_as_the_java_pattern_does() {
        assert!(super::name_matches("moqui.basic.Enumeration", "enum"));
        assert!(super::name_matches("moqui.basic.Enumeration", "basic\\.E"));
        assert!(!super::name_matches("moqui.basic.Enumeration", "status"));
        assert!(super::name_matches("anything", ""));
    }
}
