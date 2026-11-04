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

//! The pages above the artifact docs: an entity package (its entities and an E-R diagram), a service directory (its
//! files and a class diagram of them), a screen directory (its screens and their subscreen tree), each tree's index, and
//! the root. They list the docs in the tree only, so a partial export lists what it wrote and what was there before.

use std::collections::{BTreeMap, BTreeSet};

use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::entity::{er_entity, er_links};
use super::Run;
use crate::autodoc::mermaid::{self, ClassBox, ClassLink, ClassLinkKind, ErEntity, FlowEdge, FlowNode, Shape};
use crate::autodoc::plan::{DocEntry, DocKind, Page};
use crate::index::Direction;

/// A list row of a doc.
fn row(run: &Run<'_>, from: &DocEntry, entry: &DocEntry) -> Json {
    json!({
        "title": entry.title,
        "name": entry.name,
        "summary": entry.summary,
        "component": entry.component,
        "href": run.link(from, entry),
    })
}

/// The members of a package that are in the tree.
fn listed_members<'p>(run: &Run<'p>, kind: DocKind, key: &str) -> Vec<&'p DocEntry> {
    run.plan
        .members(kind, key)
        .into_iter()
        .filter(|entry| run.listed(&entry.path))
        .collect()
}

pub(super) fn entity_package(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let mut map = run.header(doc, None);
    let members = listed_members(run, DocKind::Entity, &doc.package);
    let facade = run.factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let definitions: Vec<_> = members
        .iter()
        .filter_map(|entry| facade.with_entity(&entry.name, |entity, _, _| Ok(entity.clone())).ok())
        .collect();
    let names_only = definitions.len() > mermaid::ER_ATTRIBUTE_LIMIT;
    let mut entities: Vec<ErEntity> = definitions.iter().map(|entity| er_entity(entity, names_only)).collect();
    let mut shown: BTreeSet<String> = definitions.iter().map(|entity| entity.full_name.clone()).collect();
    // the entities outside the package that its entities relate to, without their fields
    for definition in &definitions {
        for relationship in &definition.relationships {
            if !relationship.is_auto_reverse && shown.insert(relationship.related.clone()) {
                entities.push(ErEntity {
                    full_name: relationship.related.clone(),
                    short_name: relationship.related.rsplit('.').next().unwrap_or_default().to_string(),
                    stub: true,
                    attributes: Vec::new(),
                });
            }
        }
    }
    let links = er_links(&facade, &definitions, &shown);
    let diagram = if definitions.is_empty() {
        String::new()
    } else {
        mermaid::er_diagram(&entities, &links)
    };
    map.insert(
        "entities".into(),
        Json::Array(members.iter().map(|entry| row(run, doc, entry)).collect()),
    );
    map.insert("namesOnly".into(), json!(names_only));
    map.insert("diagrams".into(), json!({"er": diagram}));
    Ok(Json::Object(map))
}

pub(super) fn service_directory(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let mut map = run.header(doc, None);
    let files = listed_members(run, DocKind::Service, &doc.package);
    let mut classes: Vec<ClassBox> = Vec::new();
    let mut links: Vec<ClassLink> = Vec::new();
    let mut interfaces: BTreeMap<String, String> = BTreeMap::new();
    let mut rows: Vec<Json> = Vec::new();
    for file in &files {
        let services = run.plan.services_of(&file.file);
        classes.push(ClassBox {
            key: file.file.clone(),
            name: file.title.clone(),
            members: services.iter().map(|service| service.title.clone()).collect(),
            interface: false,
        });
        // what the file's services use, all read at once: a directory can have hundreds of services
        let ids: Vec<&str> = services.iter().map(|service| service.id.as_str()).collect();
        for entry in run.source.refs_of_ids(&ids, Direction::Out)? {
            let Some(other) = &entry.other else { continue };
            if other.kind != "service" {
                continue;
            }
            let Some(target) = run.plan.of_id(&other.id) else {
                continue;
            };
            if entry.ref_type == "implements" {
                interfaces
                    .entry(target.name.clone())
                    .or_insert_with(|| target.title.clone());
                links.push(ClassLink {
                    from: file.file.clone(),
                    to: target.name.clone(),
                    kind: ClassLinkKind::Realizes,
                });
            } else if target.package == doc.package && target.file != file.file {
                links.push(ClassLink {
                    from: file.file.clone(),
                    to: target.file.clone(),
                    kind: ClassLinkKind::Depends,
                });
            }
        }
        let mut file_row = row(run, doc, file);
        if let Some(object) = file_row.as_object_mut() {
            object.insert("serviceCount".into(), json!(services.len()));
        }
        rows.push(file_row);
    }
    for (name, title) in interfaces {
        classes.push(ClassBox {
            key: name,
            name: title,
            members: Vec::new(),
            interface: true,
        });
    }
    let diagram = if files.is_empty() {
        String::new()
    } else {
        mermaid::class_diagram(&classes, &links)
    };
    map.insert("files".into(), Json::Array(rows));
    map.insert("diagrams".into(), json!({"classes": diagram}));
    Ok(Json::Object(map))
}

pub(super) fn screen_directory(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let mut map = run.header(doc, None);
    let screens = listed_members(run, DocKind::Screen, &doc.package);
    let mut nodes: Vec<FlowNode> = Vec::new();
    let mut edges: Vec<FlowEdge> = Vec::new();
    let mut stubs: Vec<FlowNode> = Vec::new();
    for screen in &screens {
        nodes.push(FlowNode {
            key: screen.name.clone(),
            label: screen.title.clone(),
            shape: Shape::Box,
        });
    }
    for screen in &screens {
        let Ok(def) = super::screen::definition(run, &screen.name) else {
            tracing::warn!(
                "Autodoc: the screen {} did not load for its directory's tree",
                screen.name
            );
            continue;
        };
        for item in &def.subscreens.items {
            if !screens.iter().any(|other| other.name == item.location) {
                let title = run
                    .plan
                    .of_id(&format!("screen:{}", item.location))
                    .map_or_else(|| item.name.clone(), |found| found.title.clone());
                stubs.push(FlowNode {
                    key: item.location.clone(),
                    label: title,
                    shape: Shape::Stub,
                });
            }
            edges.push(FlowEdge {
                from: screen.name.clone(),
                to: item.location.clone(),
                label: item.name.clone(),
                dashed: false,
            });
        }
    }
    nodes.extend(stubs);
    let diagram = if edges.is_empty() {
        String::new()
    } else {
        mermaid::flowchart("TD", &nodes, &edges)
    };
    map.insert(
        "screens".into(),
        Json::Array(screens.iter().map(|entry| row(run, doc, entry)).collect()),
    );
    map.insert("diagrams".into(), json!({"tree": diagram}));
    Ok(Json::Object(map))
}

pub(super) fn type_index(run: &mut Run<'_>, doc: &DocEntry, kind: DocKind) -> Result<Json, Error> {
    let mut map = run.header(doc, None);
    let mut packages: Vec<Json> = Vec::new();
    let mut top: Vec<Json> = Vec::new();
    let mut files: Vec<Json> = Vec::new();
    let mut total = 0;
    for (key, _) in run.plan.packages_of(kind) {
        let members = listed_members(run, kind, &key);
        if members.is_empty() {
            continue;
        }
        total += members.len();
        if matches!(kind, DocKind::Eca | DocKind::Rest) {
            files.extend(members.iter().map(|entry| {
                let mut file = row(run, doc, entry);
                if let Some(object) = file.as_object_mut() {
                    object.insert("directory".into(), json!(key));
                }
                file
            }));
        } else if key.is_empty() {
            top.extend(members.iter().map(|entry| row(run, doc, entry)));
        } else {
            let page = run.plan.package_page(kind, &key);
            packages.push(json!({
                "key": key,
                "count": members.len(),
                "href": page.and_then(|found| run.link(doc, found)),
            }));
        }
    }
    map.insert("treeKind".into(), json!(kind.as_str()));
    map.insert("packages".into(), Json::Array(packages));
    map.insert("top".into(), Json::Array(top));
    map.insert("files".into(), Json::Array(files));
    map.insert("count".into(), json!(total));
    Ok(Json::Object(map))
}

pub(super) fn root(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let mut map = run.header(doc, None);
    let mut trees: Vec<Json> = Vec::new();
    for kind in DocKind::ALL {
        let count = run
            .plan
            .entries
            .iter()
            .filter(|entry| entry.kind == kind && matches!(entry.page, Page::Artifact(..) | Page::File(..)))
            .filter(|entry| !(kind == DocKind::Service && matches!(entry.page, Page::File(..))))
            .filter(|entry| run.listed(&entry.path))
            .count();
        let index = run.plan.at_path(&format!("{}/README.md", kind.as_str()));
        trees.push(json!({
            "kind": kind.as_str(),
            "title": kind.title(),
            "count": count,
            "href": if count > 0 { index.and_then(|found| run.link(doc, found)) } else { None },
        }));
    }
    map.insert("trees".into(), Json::Array(trees));
    Ok(Json::Object(map))
}
