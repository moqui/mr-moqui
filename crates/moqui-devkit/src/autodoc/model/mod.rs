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

//! The data of each doc's template (plans/Autodoc.md, "Document content"): what every doc has (its header, its way up,
//! what uses it and what it uses, grouped by kind as the Artifact Browser groups them) here, and each type's sections in
//! the modules below. A linked artifact carries its `href`, the relative link to its doc, only when that doc is in the
//! tree; the text is escaped by the templates' helpers (`AutodocLib.js`), not here.

mod entity;
mod files;
mod pages;
mod screen;
mod service;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::describe::Descriptions;
use super::mermaid::{self, UsageGroup};
use super::plan::{relative_link, DocEntry, DocKind, DocPlan, Page};
use super::source::DocIndex;
use super::Options;
use crate::eca::EcaRuleInfo;
use crate::index::{ArtifactRow, Direction, RefEntry};

/// What a run reads once and every doc of it shares.
pub(crate) struct Run<'a> {
    pub factory: &'a Arc<ExecutionContextFactory>,
    pub source: &'a dyn DocIndex,
    pub plan: &'a DocPlan,
    pub options: &'a Options,
    /// The doc paths in the tree, which links and lists take; `None`: every doc of the plan (the viewer).
    pub listed: Option<HashSet<String>>,
    pub descriptions: Descriptions,
    components: HashMap<String, String>,
    eca_rules: Option<Vec<EcaRuleInfo>>,
}

impl<'a> Run<'a> {
    pub fn new(
        factory: &'a Arc<ExecutionContextFactory>,
        source: &'a dyn DocIndex,
        plan: &'a DocPlan,
        options: &'a Options,
        listed: Option<HashSet<String>>,
    ) -> Self {
        Self {
            factory,
            source,
            plan,
            options,
            listed,
            descriptions: Descriptions::default(),
            components: super::component_dirs(factory).into_iter().collect(),
            eca_rules: None,
        }
    }

    /// Whether the doc at `path` is in the tree.
    pub fn listed(&self, path: &str) -> bool {
        self.listed.as_ref().is_none_or(|listed| listed.contains(path))
    }

    /// The link from `from` to `to`, when `to` is in the tree.
    pub fn link(&self, from: &DocEntry, to: &DocEntry) -> Option<String> {
        self.listed(&to.path).then(|| relative_link(&from.path, &to.path))
    }

    /// The link from `from` to the doc of the artifact `id`.
    pub fn link_id(&self, from: &DocEntry, id: &str) -> Option<String> {
        self.plan.of_id(id).and_then(|to| self.link(from, to))
    }

    /// The `component://` location of a file of a component.
    pub fn location(&self, component: &str, file: &str) -> String {
        self.components
            .get(component)
            .and_then(|dir| Path::new(file).strip_prefix(dir).ok())
            .map_or_else(
                || file.to_string(),
                |relative| {
                    format!(
                        "component://{component}/{}",
                        relative.to_string_lossy().replace('\\', "/")
                    )
                },
            )
    }

    /// Every ECA rule of the components, read once a run.
    pub fn eca_rules(&mut self) -> &[EcaRuleInfo] {
        if self.eca_rules.is_none() {
            self.eca_rules = Some(crate::eca::rule_list(self.factory));
        }
        self.eca_rules.as_deref().unwrap_or_default()
    }

    /// What every doc has: its names, where it is defined, its description, and the links up.
    pub fn header(&mut self, doc: &DocEntry, description: Option<String>) -> serde_json::Map<String, Json> {
        let up = self
            .plan
            .parent_of(doc)
            .map(|parent| json!({"title": parent.title, "href": self.link(doc, parent)}));
        let root = self.plan.at_path("README.md").and_then(|root| self.link(doc, root));
        let location = if doc.file.is_empty() {
            String::new()
        } else {
            self.location(&doc.component, &doc.file)
        };
        let mut map = serde_json::Map::new();
        map.insert("title".into(), json!(doc.title));
        map.insert("name".into(), json!(doc.name));
        map.insert("kind".into(), json!(doc.kind.as_str()));
        map.insert("kindTitle".into(), json!(doc.kind.title()));
        map.insert("path".into(), json!(doc.path));
        map.insert("component".into(), json!(doc.component));
        map.insert("packageName".into(), json!(doc.package));
        map.insert("location".into(), json!(location));
        map.insert("line".into(), json!(doc.line));
        map.insert("summary".into(), json!(doc.summary));
        map.insert("description".into(), json!(description.unwrap_or_default()));
        map.insert("up".into(), up.unwrap_or(Json::Null));
        map.insert("rootHref".into(), json!(root));
        map
    }

    /// What uses the artifacts `ids` (`In`) or what they use (`Out`), each other side as the doc it is shown in, grouped
    /// by kind; references to `doc` itself left out.
    pub fn usage(&mut self, doc: &DocEntry, ids: &[&str], direction: Direction) -> Result<Vec<Group>, Error> {
        let mut entries: Vec<RefEntry> = Vec::new();
        for id in ids {
            entries.extend(self.source.refs(id, direction)?);
        }
        let mut items: BTreeMap<(usize, String), Item> = BTreeMap::new();
        for entry in &entries {
            let Some(item) = self.item_of(doc, entry, direction) else {
                continue;
            };
            let key = (label_order(&item.label), item.key.clone());
            let kept = items.entry(key).or_insert_with(|| Item {
                via: BTreeSet::new(),
                calls: 0,
                observed: true,
                ..item.clone()
            });
            kept.via.extend(item.via);
            kept.calls += entry.calls;
            kept.observed &= entry.observed_only;
        }
        let mut groups: Vec<Group> = Vec::new();
        for ((_, _), item) in items {
            match groups.last_mut() {
                Some(group) if group.label == item.label => group.items.push(item),
                _ => groups.push(Group {
                    label: item.label.clone(),
                    items: vec![item],
                }),
            }
        }
        for group in &mut groups {
            group
                .items
                .sort_by(|a, b| a.title.cmp(&b.title).then(a.key.cmp(&b.key)));
        }
        Ok(groups)
    }

    /// The item a reference shows: the doc of the artifact on its other side, else the artifact (a data file's record as
    /// its file), else the name it gives.
    fn item_of(&self, doc: &DocEntry, entry: &RefEntry, direction: Direction) -> Option<Item> {
        let via_type = entry.ref_type.clone();
        let Some(other) = &entry.other else {
            let (name, kind) = match direction {
                Direction::In => (
                    entry.from.clone(),
                    entry.from.split(':').next().unwrap_or_default().to_string(),
                ),
                Direction::Out => (entry.to_name.clone(), entry.to_kind.clone()),
            };
            return Some(Item {
                key: format!("{kind}:{name}"),
                title: name.clone(),
                name,
                label: kind_label(&kind).to_string(),
                href: None,
                via: BTreeSet::from([via_type]),
                calls: 0,
                observed: false,
                resolved: false,
            });
        };
        if let Some(owner) = self.plan.of_artifact(other) {
            if owner.path == doc.path {
                return None;
            }
            let part = other.id != owner.id && !matches!(owner.page, Page::File(..)) && other.kind != via_type;
            let via = BTreeSet::from([if part {
                format!("{via_type} in {} {}", other.kind, other.short_name)
            } else {
                via_type
            }]);
            return Some(Item {
                key: owner.path.clone(),
                title: owner.title.clone(),
                name: owner.name.clone(),
                label: doc_label(owner).to_string(),
                href: self.link(doc, owner),
                via,
                calls: 0,
                observed: false,
                resolved: true,
            });
        }
        if other.kind == "record" {
            let file = other.parent.clone().unwrap_or_else(|| other.path.clone());
            let title = file.rsplit('/').next().unwrap_or(file.as_str()).to_string();
            return Some(Item {
                key: file.clone(),
                title,
                name: file.strip_prefix("data:").unwrap_or(&file).to_string(),
                label: kind_label("data").to_string(),
                href: None,
                via: BTreeSet::from([via_type]),
                calls: 0,
                observed: false,
                resolved: true,
            });
        }
        Some(Item {
            key: other.id.clone(),
            title: other.short_name.clone(),
            name: other.name.clone(),
            label: kind_label(&other.kind).to_string(),
            href: None,
            via: BTreeSet::from([via_type]),
            calls: 0,
            observed: false,
            resolved: true,
        })
    }

    /// The `usedBy`, `uses` and `diagrams.usage` of an artifact doc: what uses `ids` and what they use.
    pub fn usage_data(&mut self, doc: &DocEntry, ids: &[&str]) -> Result<(Json, Json, String), Error> {
        let used_by = self.usage(doc, ids, Direction::In)?;
        let uses = self.usage(doc, ids, Direction::Out)?;
        let diagram = mermaid::usage_flowchart(
            &doc.title,
            &used_by.iter().map(Group::diagram).collect::<Vec<_>>(),
            &uses.iter().map(Group::diagram).collect::<Vec<_>>(),
        );
        Ok((groups_json(&used_by), groups_json(&uses), diagram))
    }

    /// The description of the artifact `row` from its file.
    pub fn description_of(&mut self, row_path: &str, line: u32) -> Option<String> {
        self.descriptions.at(row_path, line)
    }

    /// The artifact rows a doc's parts come from (fields, relationships, forms, transitions), by id.
    pub fn children_by_id(&self, id: &str) -> Result<HashMap<String, ArtifactRow>, Error> {
        Ok(self
            .source
            .children(id)?
            .into_iter()
            .map(|row| (row.id.clone(), row))
            .collect())
    }

    /// The data of the doc `doc` for its template.
    pub fn data(&mut self, doc: &DocEntry) -> Result<Json, Error> {
        match &doc.page {
            Page::Artifact(DocKind::Entity, _) => entity::data(self, doc),
            Page::Artifact(DocKind::Service, _) => service::data(self, doc),
            Page::Artifact(DocKind::Screen, _) => screen::data(self, doc),
            Page::File(DocKind::Service, _) => service::file_data(self, doc),
            Page::File(DocKind::Eca, _) => files::eca_data(self, doc),
            Page::File(DocKind::Rest, _) => files::rest_data(self, doc),
            Page::Package(DocKind::Entity, _) => pages::entity_package(self, doc),
            Page::Package(DocKind::Service, _) => pages::service_directory(self, doc),
            Page::Package(DocKind::Screen, _) => pages::screen_directory(self, doc),
            Page::TypeIndex(kind) => pages::type_index(self, doc, *kind),
            Page::Root => pages::root(self, doc),
            other => Err(Error::new(format!("no doc of {other:?}"))),
        }
    }
}

/// One item of a usage group: an artifact on the other side of a reference.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    key: String,
    pub title: String,
    pub name: String,
    pub label: String,
    pub href: Option<String>,
    /// How it refers: the reference types and the parts (`transition placeOrder`).
    pub via: BTreeSet<String>,
    pub calls: i64,
    pub observed: bool,
    pub resolved: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub label: String,
    pub items: Vec<Item>,
}

impl Group {
    fn diagram(&self) -> UsageGroup {
        UsageGroup {
            label: self.label.clone(),
            names: self.items.iter().map(|item| item.title.clone()).collect(),
            observed_only: self.items.iter().all(|item| item.observed),
        }
    }
}

fn groups_json(groups: &[Group]) -> Json {
    Json::Array(
        groups
            .iter()
            .map(|group| {
                json!({
                    "label": group.label,
                    "count": group.items.len(),
                    "items": group.items.iter().map(|item| json!({
                        "title": item.title,
                        "name": item.name,
                        "href": item.href,
                        "via": item.via.iter().filter(|via| !via.is_empty()).cloned().collect::<Vec<_>>().join(", "),
                        "calls": item.calls,
                        "observed": item.observed,
                        "resolved": item.resolved,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// The group of a doc.
fn doc_label(doc: &DocEntry) -> &'static str {
    match doc.kind {
        DocKind::Entity => "Entities",
        DocKind::Service => "Services",
        DocKind::Screen => "Screens",
        DocKind::Eca => "ECA rules",
        DocKind::Rest => "REST APIs",
    }
}

/// The group of an artifact kind with no doc of its own.
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "entity" | "view-entity" | "field" | "relationship" => "Entities",
        "service" | "parameter" => "Services",
        "screen" | "transition" | "form" | "form-field" | "subscreen" | "widget-template" | "location" => "Screens",
        "screen-extend" | "anchor" | "ext" => "Screen extensions",
        "extend-entity" => "Entity extensions",
        "seca" | "eeca" | "emeca" | "giteca" => "ECA rules",
        "rest" => "REST APIs",
        "job" => "Service jobs",
        "document" => "Data documents",
        "email" => "Email templates",
        "data" | "record" => "Data files",
        "script" => "Scripts",
        "skill" => "Skills",
        "dbform" => "Forms in the database",
        _ => "Other",
    }
}

const LABEL_ORDER: &[&str] = &[
    "Screens",
    "Screen extensions",
    "Services",
    "Entities",
    "Entity extensions",
    "ECA rules",
    "REST APIs",
    "Service jobs",
    "Data documents",
    "Email templates",
    "Data files",
    "Scripts",
    "Skills",
    "Forms in the database",
    "Other",
];

fn label_order(label: &str) -> usize {
    LABEL_ORDER
        .iter()
        .position(|known| *known == label)
        .unwrap_or(LABEL_ORDER.len())
}

/// A text cut to at most `lines` lines, with a last line saying how many were left out.
pub(crate) fn capped(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    if all.len() <= lines {
        return text.trim_end().to_string();
    }
    format!("{}\n<!-- {} more lines -->", all[..lines].join("\n"), all.len() - lines)
}

/// The lines of source a doc shows at most.
pub(crate) const SOURCE_LINES: usize = 200;
