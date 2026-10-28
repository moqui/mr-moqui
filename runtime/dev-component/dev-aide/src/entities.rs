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

//! `org.moqui.devaide.EntityServices`: the entity modeler's server side (plans/design-dev-aide.md 8). The entity view
//! reads an entity as the server merged it, with the origin of each field (the defining file, each `extend-entity`),
//! where its edits go (the site's `extend-entity` for an upstream entity, D17; the defining file with the upstream
//! toggle on), its incoming relationships, its EECA rules, its table against its fields and its first rows. Saving the
//! view's files relinks the model and runs the additive table check (F5, through the save's reload); what the check
//! never does (drop a column, change its type, copy a renamed field's data, recreate the table) is planned and run
//! here for the full tier only, after the table's name is typed back, and audited. A modeler's seed data file loads
//! here after its save.

use std::collections::BTreeMap;
use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::devtools::index::{self, Direction};
use moqui_core::entity::info;
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::artifacts::shown;
use crate::audit::{self, Write};
use crate::site;
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(list_entities), list_entities)?;
    registry.service_function(moqui_core::service_location!(get_entity_view), get_entity_view)?;
    registry.service_function(moqui_core::service_location!(plan_entity_ddl), plan_entity_ddl)?;
    registry.service_function(moqui_core::service_location!(run_entity_ddl), run_entity_ddl)?;
    registry.service_function(moqui_core::service_location!(load_seed_file), load_seed_file)
}

fn moqui_json(value: &Value) -> Json {
    moqui_lib::json::to_json(value)
}

/// `list#Entities`: the entities and view entities of the index, by name, with their component, file and whether
/// the site extends them; `filter` matches the name, `component` narrows.
fn list_entities(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let filter = text(parameters, "filter").to_lowercase();
    let components: Vec<String> = text(parameters, "component")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let kinds = match text(parameters, "kind").as_str() {
        "entity" => vec!["entity".to_string()],
        "view-entity" => vec!["view-entity".to_string()],
        _ => vec!["entity".to_string(), "view-entity".to_string()],
    };
    let rows = index::of_kinds(&context.factory, &kinds, &components, 20_000)?;
    let mut extended: BTreeMap<String, usize> = BTreeMap::new();
    for extension in index::extensions(&context.factory, &[])? {
        if extension.target.starts_with("entity:") {
            *extended.entry(extension.target.clone()).or_insert(0) += 1;
        }
    }
    let entities: Vec<Json> = rows
        .iter()
        .filter(|row| filter.is_empty() || row.name.to_lowercase().contains(&filter))
        .map(|row| {
            let mut out = shown(&context, row);
            out["extensions"] = json!(extended.get(&row.id).copied().unwrap_or(0));
            out["package"] = json!(row.name.rsplit_once('.').map_or("", |(package, _)| package));
            out["editUrl"] = json!(format!("/qapps/dev/Entities/Edit?artifact={}", row.id));
            out
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("entities".into(), json_value(&entities));
    Ok(out)
}

/// The entity's full name from `artifactId` (`entity:moqui.example.Example`) or `entityName`.
fn entity_name_in(parameters: &Parameters) -> String {
    let id = text(parameters, "artifactId");
    match id.split_once(':') {
        Some(("entity" | "view-entity", name)) => name.to_string(),
        _ if !id.is_empty() => id,
        _ => text(parameters, "entityName"),
    }
}

/// `get#EntityView`: everything the entity view shows of one entity.
fn get_entity_view(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let facade = &context.facade;
    let asked = entity_name_in(parameters);
    let detail = info::entity_detail(facade, &asked).ok_or_else(|| LibError::new(format!("no entity {asked}")))?;
    let detail_json = moqui_json(&Value::Map(detail));
    let full_name = detail_json["fullEntityName"].as_str().unwrap_or(&asked).to_string();
    let short_name = detail_json["entityName"].as_str().unwrap_or_default().to_string();
    let is_view = detail_json["isViewEntity"].as_bool().unwrap_or(false);
    let id = format!("entity:{full_name}");
    let mut out = Parameters::new();
    let artifact = index::describe(&context.factory, &id)?;
    let defining = artifact.as_ref().map(|row| shown(&context, row));
    // each field where it is defined: the entity's file, or an extend-entity of another component
    let fields: Vec<Json> = index::children(&context.factory, &id)?
        .iter()
        .filter(|row| row.kind == "field" || row.kind == "alias")
        .map(|row| {
            let mut field = site::located(&context, &row.path);
            field["name"] = json!(row.short_name);
            field["line"] = json!(row.line);
            field["fromExtension"] =
                json!(row.component != artifact.as_ref().map_or("", |artifact| &artifact.component));
            field["artifactId"] = json!(row.id);
            field
        })
        .collect();
    let users = index::refs(&context.factory, &id, Direction::In, &[])?;
    let extensions: Vec<Json> = users
        .iter()
        .filter(|entry| {
            entry.ref_type == "extend" && entry.other.as_ref().is_some_and(|other| other.kind == "extend-entity")
        })
        .map(|entry| {
            let mut extension = site::located(&context, &entry.path);
            extension["line"] = json!(entry.line);
            extension
        })
        .collect();
    let eecas: Vec<Json> = users
        .iter()
        .filter(|entry| entry.other.as_ref().is_some_and(|other| other.kind == "eeca"))
        .map(|entry| {
            let mut rule = entry.other.as_ref().map_or(Json::Null, |other| shown(&context, other));
            rule["refType"] = json!(entry.ref_type);
            rule
        })
        .collect();
    let used_by = users
        .iter()
        .filter(|entry| !matches!(entry.ref_type.as_str(), "extend" | "anchor"))
        .count();
    // where the view's edits go
    let defining_site = defining
        .as_ref()
        .is_some_and(|row| row["site"].as_bool().unwrap_or(false));
    let in_place = defining_site || context.upstream_edits();
    let site_name = context.settings.default_site().map(str::to_string);
    let class = site_name.as_deref().map(site::class_of).unwrap_or_default();
    let target = if in_place {
        defining.as_ref().map(|row| {
            json!({ "rootId": row["rootId"], "path": row["relativePath"], "exists": true, "inPlace": true,
                "component": row["component"] })
        })
    } else {
        site::site_file(
            &context,
            "entity",
            &format!("{class}Entities.xml"),
            ".xml",
            Some(&format!("entity-name=\"{short_name}\"")),
        )
        .map(|file| {
            let mut target = file.json();
            target["inPlace"] = json!(false);
            target
        })
    };
    let seed = site::site_file(
        &context,
        "data",
        &format!("{class}SeedData.xml"),
        ".xml",
        Some("type=\"seed\""),
    )
    .map(|file| file.json());
    // the table against the fields
    let mut table = json!({ "exists": false, "columns": [], "missingColumns": [], "extraColumns": [] });
    if !is_view {
        match facade.table_columns(&full_name) {
            Ok(columns) => {
                let wanted: Vec<String> = detail_json["fields"]
                    .as_array()
                    .map(|fields| {
                        fields
                            .iter()
                            .filter_map(|field| field["columnName"].as_str().map(str::to_ascii_lowercase))
                            .collect()
                    })
                    .unwrap_or_default();
                let have: Vec<String> = columns.iter().map(|(name, _)| name.to_ascii_lowercase()).collect();
                let missing: Vec<&String> = wanted.iter().filter(|name| !have.contains(name)).collect();
                let extra: Vec<&String> = have.iter().filter(|name| !wanted.contains(name)).collect();
                table = json!({
                    "exists": !columns.is_empty(),
                    "columns": columns.iter().map(|(name, sql_type)| json!({ "name": name, "sqlType": sql_type })).collect::<Vec<_>>(),
                    "missingColumns": missing,
                    "extraColumns": extra,
                });
            }
            Err(err) => table["error"] = json!(err.to_string()),
        }
    }
    // the data: its count and first rows, read as the user
    let mut data = json!({});
    if table["exists"].as_bool().unwrap_or(false) || is_view {
        match facade.find(&full_name).and_then(|find| find.count()) {
            Ok(count) => data["count"] = json!(count),
            Err(err) => data["error"] = json!(err.to_string()),
        }
        if let Ok(rows) = facade.find(&full_name).and_then(|find| find.limit(10).list()) {
            data["rows"] = Json::Array(
                rows.iter()
                    .map(|row| {
                        Json::Object(
                            row.fields()
                                .iter()
                                .map(|(name, value)| (name.to_string(), moqui_json(value)))
                                .collect(),
                        )
                    })
                    .collect(),
            );
        }
    }
    let group = detail_json["groupName"].as_str().map(str::to_string);
    let types: Vec<Json> = info::field_types(facade, group.as_deref())
        .into_iter()
        .map(|row| moqui_json(&Value::Map(row)))
        .collect();
    let view = json!({
        "entity": detail_json,
        "artifact": defining,
        "fields": fields,
        "extensions": extensions,
        "eecas": eecas,
        "usedByCount": used_by,
        "target": target,
        "seedFile": seed,
        "site": site_name,
        "inPlace": in_place,
        "upstream": !defining_site,
        "table": table,
        "data": data,
        "fieldTypes": types,
        "tier": context.tier.name(),
        "upstreamEdits": context.upstream_edits(),
    });
    out.insert("view".into(), json_value(&view));
    Ok(out)
}

/// `plan#EntityDdl`: the SQL of a change the table check never makes, to read before running it. Full tier.
fn plan_entity_ddl(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let name = entity_name_in(parameters);
    let statements = context.facade.plan_ddl(
        &name,
        &text(parameters, "action"),
        &text(parameters, "fieldName"),
        &text(parameters, "columnName"),
    )?;
    let table = context
        .facade
        .with_entity_table(&name)
        .ok_or_else(|| LibError::new(format!("no entity {name}")))?;
    let mut out = Parameters::new();
    out.insert("statements".into(), json_value(&statements));
    out.insert("tableName".into(), Value::string(table));
    Ok(out)
}

/// `run#EntityDdl`: run what [`plan_entity_ddl`] planned, when `confirmTable` is the table's name typed back; each
/// statement audited. Full tier: the extend tier never sees a destructive change.
fn run_entity_ddl(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let name = entity_name_in(parameters);
    let action = text(parameters, "action");
    let table = context
        .facade
        .with_entity_table(&name)
        .ok_or_else(|| LibError::new(format!("no entity {name}")))?;
    if !text(parameters, "confirmTable").eq_ignore_ascii_case(&table) {
        return Err(LibError::new(format!("type the table's name, {table}, to run it")));
    }
    let statements = context.facade.plan_ddl(
        &name,
        &action,
        &text(parameters, "fieldName"),
        &text(parameters, "columnName"),
    )?;
    let group = context.facade.group_of(&name).unwrap_or_default();
    let mut ran = Vec::new();
    for sql in &statements {
        let result = info::run_sql(&context.facade, &group, sql, 0);
        audit::record(
            &context.facade,
            &Write {
                action: "DAA_DDL",
                root: "",
                path: &name,
                detail: Some(json!({ "action": action, "sql": sql, "ok": result.is_ok(),
                    "error": result.as_ref().err().map(ToString::to_string) })),
                ..Write::default()
            },
        );
        match result {
            Ok(_) => ran.push(sql.clone()),
            Err(err) => {
                return Err(LibError::new(format!(
                    "{sql}: {err} (ran before it: {})",
                    if ran.is_empty() {
                        "nothing".to_string()
                    } else {
                        ran.join("; ")
                    }
                )))
            }
        }
    }
    context.facade.forget_tables();
    let mut out = Parameters::new();
    out.insert("ran".into(), json_value(&ran));
    Ok(out)
}

/// `load#SeedFile`: load one data file of a site component into the database, as the modeler does after saving the
/// seed rows of a Picklist or Status field. Extend tier, site files only (or any file with the upstream toggle on).
fn load_seed_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    context.may_write(root, &resolved.relative)?;
    if !resolved.relative.ends_with(".xml") {
        return Err(LibError::new("a seed data file is an XML file"));
    }
    let location = resolved.path.display().to_string();
    let loaded = moqui_core::entity::load_feed::load_entity_data(
        &context.facade,
        &location,
        &moqui_core::entity::load_feed::LoadOptions::default(),
    );
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_LOAD",
            root: &root.id,
            path: &resolved.relative,
            detail: Some(json!({ "records": loaded.as_ref().map(|result| result.records).ok(),
                "error": loaded.as_ref().err().map(ToString::to_string) })),
            ..Write::default()
        },
    );
    let loaded = loaded?;
    let mut out = Parameters::new();
    out.insert(
        "records".into(),
        Value::I64(i64::try_from(loaded.records).unwrap_or(i64::MAX)),
    );
    out.insert(
        "file".into(),
        Value::string(
            Path::new(&resolved.relative)
                .file_name()
                .map_or(String::new(), |name| name.to_string_lossy().into_owned()),
        ),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::Value as Json;

    /// The entity view's operations (test/vectors/entity-view-vectors.json): each vector's outline is the one this layer
    /// reads, and its operations, which test/client/pages/DevAideEntity.page.js makes from that outline with the view's
    /// builders, give the expected text, comments and order kept. With MOQUI_REGENERATE_VECTORS set the outlines and
    /// texts are written instead, for review.
    #[test]
    fn the_entity_view_vectors_apply() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/vectors/entity-view-vectors.json");
        let mut file: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let regenerate = std::env::var_os("MOQUI_REGENERATE_VECTORS").is_some();
        for vector in file["vectors"].as_array_mut().unwrap() {
            let name = vector["name"].as_str().unwrap_or_default().to_string();
            let input = vector["input"].as_str().unwrap().to_string();
            let outline = serde_json::to_value(moqui_lib::xml_edit::outline(&input).unwrap()).unwrap();
            if regenerate {
                vector["outline"] = outline;
            } else {
                assert_eq!(vector["outline"], outline, "{name}: the outline");
            }
            let Some(ops) = vector.get("ops").cloned() else {
                assert!(regenerate, "{name}: no ops");
                continue;
            };
            let ops: Vec<moqui_lib::xml_edit::Op> = serde_json::from_value(ops).unwrap();
            let applied = moqui_lib::xml_edit::apply(&input, &ops).unwrap_or_else(|err| panic!("{name}: {err}"));
            if regenerate {
                vector["expected"] = Json::String(applied.text);
            } else {
                assert_eq!(vector["expected"].as_str(), Some(applied.text.as_str()), "{name}");
            }
        }
        if regenerate {
            std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap() + "\n").unwrap();
        }
    }
}
