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

//! The docs of the artifacts whose unit is their file: an ECA rules file (a section per rule, and a flowchart of what
//! the rules run on, the rules, and what their actions call) and a REST API file (its resource tree and a section per
//! method).

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use serde_json::{json, Value as Json};

use super::{capped, Run, SOURCE_LINES};
use crate::autodoc::mermaid::{self, FlowEdge, FlowNode, Shape};
use crate::autodoc::plan::{DocEntry, DocKind};
use crate::index::Direction;

/// The rules' attributes a rule's section lists, in this order.
const RULE_ATTRIBUTES: &[&str] = &[
    "service",
    "entity",
    "event",
    "when",
    "on-create",
    "on-update",
    "on-delete",
    "on-find-one",
    "on-find-list",
    "run-before",
    "run-on-error",
    "get-entire-entity",
    "get-original-value",
    "set-results",
    "priority",
    "repository",
    "ref",
    "name",
];

/// The rules a flowchart of an ECA file shows at most.
const FLOW_RULES: usize = 40;

pub(super) fn eca_data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let root = MNode::parse_file(&doc.file)?;
    let description = run.descriptions.lead(&doc.file);
    let mut map = run.header(doc, description);
    let mut rows = run.source.artifacts(DocKind::Eca.index_kinds())?;
    rows.retain(|row| row.path == doc.file);
    rows.sort_by_key(|row| row.line);
    let mut rules: Vec<Json> = Vec::new();
    let mut nodes: Vec<FlowNode> = Vec::new();
    let mut edges: Vec<FlowEdge> = Vec::new();
    for row in &rows {
        let Some(node) = find_line(&root, row.line) else {
            continue;
        };
        let attributes: Vec<Json> = RULE_ATTRIBUTES
            .iter()
            .filter_map(|name| node.attribute(name).map(|value| json!({"name": name, "value": value})))
            .collect();
        let target = node
            .attribute("service")
            .map(|service| ("service", service))
            .or_else(|| node.attribute("entity").map(|entity| ("entity", entity)));
        let target_doc = target
            .as_ref()
            .and_then(|(kind, name)| run.plan.of_id(&format!("{kind}:{name}")));
        let uses = run.usage(doc, &[&row.id], Direction::Out)?;
        if rules.len() < FLOW_RULES {
            nodes.push(FlowNode {
                key: row.id.clone(),
                label: row.short_name.clone(),
                shape: Shape::Round,
            });
            if let Some((kind, name)) = &target {
                let key = format!("{kind}:{name}");
                nodes.push(FlowNode {
                    key: key.clone(),
                    label: target_doc.map_or_else(|| name.clone(), |found| found.title.clone()),
                    shape: Shape::Box,
                });
                edges.push(FlowEdge {
                    from: key,
                    to: row.id.clone(),
                    label: node.attribute("when").unwrap_or_default(),
                    dashed: false,
                });
            }
            for group in &uses {
                for item in &group.items {
                    if target.as_ref().is_some_and(|(_, name)| *name == item.name) {
                        continue;
                    }
                    let key = format!("{}:{}", group.label, item.name);
                    nodes.push(FlowNode {
                        key: key.clone(),
                        label: item.title.clone(),
                        shape: Shape::Stub,
                    });
                    edges.push(FlowEdge {
                        from: row.id.clone(),
                        to: key,
                        label: String::new(),
                        dashed: false,
                    });
                }
            }
        }
        let (condition, condition_language) = match node.first_named("condition") {
            Some(condition) => match condition.children().as_slice() {
                [only] if only.has_name("expression") => (only.text().map(|text| text.trim().to_string()), "js"),
                _ => (Some(inner_xml(&condition)), "xml"),
            },
            None => (None, ""),
        };
        let target_name = target.as_ref().map(|(_, name)| name.clone()).unwrap_or_default();
        let actions = node
            .first_named("actions")
            .map(|actions| capped(&inner_xml(&actions), SOURCE_LINES));
        let description = run.description_of(&row.path, row.line);
        rules.push(json!({
            "id": row.id,
            "kind": row.kind,
            "title": row.short_name,
            "line": row.line,
            "attributes": attributes,
            "target": target.as_ref().map(|(_, name)| name.clone()),
            "targetKind": target.as_ref().map(|(kind, _)| *kind),
            "targetHref": target_doc.and_then(|found| run.link(doc, found)),
            "condition": condition,
            "conditionLanguage": condition_language,
            "actions": actions,
            "calls": uses
                .iter()
                .flat_map(|group| group.items.iter())
                .filter(|item| item.name != target_name)
                .map(|item| json!({"title": item.title, "href": item.href}))
                .collect::<Vec<_>>(),
            "description": description.unwrap_or_default(),
        }));
    }
    let flow = if nodes.is_empty() {
        String::new()
    } else {
        mermaid::flowchart("LR", &nodes, &edges)
    };
    map.insert("rules".into(), Json::Array(rules));
    map.insert("diagrams".into(), json!({"flow": flow}));
    Ok(Json::Object(map))
}

/// The element of `root`'s tree that starts on `line`.
fn find_line(root: &MNode, line: u32) -> Option<MNode> {
    for child in root.child_iter() {
        if child.line() == line {
            return Some(child);
        }
        if child.line() < line {
            if let Some(found) = find_line(&child, line) {
                return Some(found);
            }
        }
    }
    None
}

/// The XML inside an element, its children as written out, one after the other.
fn inner_xml(node: &MNode) -> String {
    node.child_iter()
        .map(|child| child.write_string().trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The depth of a REST tree the flowchart shows, and the resources it shows at most.
const REST_DEPTH: usize = 2;
const REST_NODES: usize = 60;

pub(super) fn rest_data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let root = MNode::parse_file(&doc.file)?;
    let description = root
        .attribute("description")
        .map(|text| crate::autodoc::describe::paragraphs(&text))
        .or_else(|| run.descriptions.lead(&doc.file));
    let mut map = run.header(doc, description);
    let mut resources: Vec<Json> = Vec::new();
    let mut nodes: Vec<FlowNode> = Vec::new();
    let mut edges: Vec<FlowEdge> = Vec::new();
    let base = format!("/{}", root.attribute("name").unwrap_or_default());
    walk_rest(run, doc, &root, &base, 0, None, &mut resources, &mut nodes, &mut edges);
    map.insert(
        "rest".into(),
        json!({
            "name": root.attribute("name"),
            "displayName": root.attribute("displayName"),
            "version": root.attribute("version"),
            "path": format!("/rest/s1{base}"),
            "resources": resources,
            "resourceCount": resources.len(),
        }),
    );
    map.insert(
        "diagrams".into(),
        json!({"tree": mermaid::flowchart("LR", &nodes, &edges)}),
    );
    Ok(Json::Object(map))
}

#[allow(clippy::too_many_arguments)]
fn walk_rest(
    run: &Run<'_>,
    doc: &DocEntry,
    node: &MNode,
    path: &str,
    depth: usize,
    parent: Option<&str>,
    resources: &mut Vec<Json>,
    nodes: &mut Vec<FlowNode>,
    edges: &mut Vec<FlowEdge>,
) {
    let methods: Vec<Json> = node
        .children_named("method")
        .iter()
        .map(|method| {
            let verb = method.attribute("type").unwrap_or_default().to_ascii_uppercase();
            let (target_kind, target, operation) = if let Some(service) = method.first_named("service") {
                ("service", service.attribute("name").unwrap_or_default(), None)
            } else if let Some(entity) = method.first_named("entity") {
                (
                    "entity",
                    entity.attribute("name").unwrap_or_default(),
                    entity.attribute("operation"),
                )
            } else {
                ("", String::new(), None)
            };
            let href = if target_kind == "entity" {
                entity_doc(run, &target).and_then(|found| run.link(doc, found))
            } else if target.is_empty() {
                None
            } else {
                run.link_id(doc, &format!("{target_kind}:{target}"))
            };
            json!({"method": verb, "targetKind": target_kind, "target": target, "operation": operation, "href": href})
        })
        .collect();
    let verbs: Vec<String> = methods
        .iter()
        .filter_map(|method| method.get("method").and_then(Json::as_str).map(str::to_string))
        .collect();
    if depth > 0 || !methods.is_empty() {
        resources.push(json!({
            "path": path,
            "description": node.attribute("description").map(|text| crate::autodoc::describe::paragraphs(&text)),
            "methods": methods,
        }));
    }
    if depth <= REST_DEPTH && nodes.len() < REST_NODES {
        let name = path.rsplit('/').next().unwrap_or(path);
        let text = if verbs.is_empty() {
            name.to_string()
        } else {
            format!("{name} · {}", verbs.join(" "))
        };
        nodes.push(FlowNode {
            key: path.to_string(),
            label: text,
            shape: if depth == 0 { Shape::Subject } else { Shape::Box },
        });
        if let Some(parent) = parent {
            edges.push(FlowEdge {
                from: parent.to_string(),
                to: path.to_string(),
                label: String::new(),
                dashed: false,
            });
        }
    }
    for child in node.child_iter() {
        let segment = match child.name().as_str() {
            "resource" => child.attribute("name").unwrap_or_default(),
            "id" => format!("{{{}}}", child.attribute("name").unwrap_or_default()),
            _ => continue,
        };
        let child_path = format!("{path}/{segment}");
        walk_rest(
            run,
            doc,
            &child,
            &child_path,
            depth + 1,
            Some(path),
            resources,
            nodes,
            edges,
        );
    }
}

/// The doc of an entity named by its full or short name.
fn entity_doc<'p>(run: &Run<'p>, name: &str) -> Option<&'p DocEntry> {
    if let Some(found) = run.plan.of_id(&format!("entity:{name}")) {
        return Some(found);
    }
    let full = run
        .factory
        .entity()?
        .with_entity(name, |entity, _, _| Ok(entity.full_name.clone()))
        .ok()?;
    run.plan.of_id(&format!("entity:{full}"))
}
