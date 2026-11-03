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

//! Where each doc goes (plans/Autodoc.md, "Output layout"): the artifact type first, then the type's package structure.
//! Entities by the segments of their package (`entity/mantle/order/OrderHeader.md`), services and screens by their path
//! under their component's `service/` or `screen/` directory (`service/mantle/order/OrderServices/place-Order.md`,
//! `screen/SimpleScreens/Order/OrderDetail.md`, not by where a screen is mounted), ECA rules files by their path in the
//! component (`eca/service/mantle/order/OrderServices.secas.md`) and REST API files by theirs under `service/`
//! (`rest/mantle.md`). Two units at one path (the same screen path in two components) both take their component's name
//! (`OrderDetail.my-site.md`). The plan answers both ways: the doc of an artifact, and what a doc path is.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::index::ArtifactRow;

/// The artifact types of the docs, each a tree of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Entity,
    Service,
    Screen,
    Eca,
    Rest,
}

impl DocKind {
    pub const ALL: [Self; 5] = [Self::Entity, Self::Service, Self::Screen, Self::Eca, Self::Rest];

    /// The tree's directory, and the kind's name in options and the manifest.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Service => "service",
            Self::Screen => "screen",
            Self::Eca => "eca",
            Self::Rest => "rest",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(text.trim()))
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Entity => "Entities",
            Self::Service => "Services",
            Self::Screen => "Screens",
            Self::Eca => "ECA rules",
            Self::Rest => "REST APIs",
        }
    }

    /// The index kinds whose artifacts this tree documents.
    #[must_use]
    pub fn index_kinds(self) -> &'static [&'static str] {
        match self {
            Self::Entity => &["entity", "view-entity"],
            Self::Service => &["service"],
            Self::Screen => &["screen"],
            Self::Eca => &["seca", "eeca", "emeca", "giteca"],
            Self::Rest => &["rest"],
        }
    }
}

/// What a doc is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    /// `README.md`: the trees and their counts.
    Root,
    /// `<kind>/README.md`: a tree's packages, and the docs at its top.
    TypeIndex(DocKind),
    /// An entity package or a service or screen directory: `<kind>/<package path>/README.md`.
    Package(DocKind, String),
    /// An entity, a service or a screen, by its index id.
    Artifact(DocKind, String),
    /// A service file, an ECA rules file or a REST API file, by its file path.
    File(DocKind, String),
}

/// One doc of the plan.
#[derive(Clone, Debug)]
pub struct DocEntry {
    /// The doc's path in the tree, with `/`.
    pub path: String,
    pub page: Page,
    pub kind: DocKind,
    /// The short name a list shows (`OrderHeader`, `place#Order`, `OrderDetail`, `OrderServices`).
    pub title: String,
    /// The full name (`mantle.order.OrderHeader`, the service name, the `component://` location).
    pub name: String,
    pub component: String,
    /// The package key: an entity's package, a directory under `service/` or `screen/` (`mantle/order`), an ECA or REST
    /// file's directory; empty at the top of a tree.
    pub package: String,
    pub summary: String,
    /// The source file and line.
    pub file: String,
    pub line: u32,
    /// The index id of an artifact page; empty for the others.
    pub id: String,
}

/// The docs of a set of artifacts, both ways.
#[derive(Clone, Debug, Default)]
pub struct DocPlan {
    pub entries: Vec<DocEntry>,
    by_path: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
    /// A file's doc (service, ECA and REST files) by its path, and by its `component://` location.
    by_file: HashMap<String, usize>,
    by_location: HashMap<String, usize>,
    /// Each package's members, by kind and key, in name order.
    packages: BTreeMap<(DocKind, String), Vec<usize>>,
}

/// A unit before its path is final: the path without `.md`, which a collision changes.
struct Unit {
    stem: String,
    entry: DocEntry,
    /// For a service file: its services, each with the name of its doc under the file's directory.
    services: Vec<(String, DocEntry)>,
}

impl DocPlan {
    /// The plan of `rows` (index rows of the kinds of [`DocKind::index_kinds`]), with `components` (name and directory)
    /// to place files under their component. A derived row (an entity-auto service no file defines) has no doc.
    #[must_use]
    pub fn build(rows: &[ArtifactRow], components: &[(String, String)]) -> Self {
        let directory: HashMap<&str, &str> = components
            .iter()
            .map(|(name, dir)| (name.as_str(), dir.as_str()))
            .collect();
        let relative = |row: &ArtifactRow| -> Option<String> {
            let dir = directory.get(row.component.as_str())?;
            let path = Path::new(&row.path).strip_prefix(dir).ok()?;
            Some(path.to_string_lossy().replace('\\', "/"))
        };
        let mut units: Vec<Unit> = Vec::new();
        let mut service_files: HashMap<String, usize> = HashMap::new();
        let mut rule_files: HashMap<String, usize> = HashMap::new();
        let mut sorted: Vec<&ArtifactRow> = rows.iter().filter(|row| row.origin.is_empty()).collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        for row in sorted {
            match row.kind.as_str() {
                "entity" | "view-entity" => {
                    let (package, short) = row.name.rsplit_once('.').unwrap_or(("", row.name.as_str()));
                    let dir = package.replace('.', "/");
                    units.push(Unit {
                        stem: join(&["entity", &dir, short]),
                        entry: entry(
                            row,
                            DocKind::Entity,
                            Page::Artifact(DocKind::Entity, row.id.clone()),
                            short,
                            package,
                        ),
                        services: Vec::new(),
                    });
                }
                "screen" => {
                    let Some(rel) = relative(row) else { continue };
                    let Some(under) = rel.strip_prefix("screen/") else {
                        continue;
                    };
                    let under = under.strip_suffix(".xml").unwrap_or(under);
                    let (dir, short) = under.rsplit_once('/').unwrap_or(("", under));
                    units.push(Unit {
                        stem: join(&["screen", under]),
                        entry: entry(
                            row,
                            DocKind::Screen,
                            Page::Artifact(DocKind::Screen, row.id.clone()),
                            short,
                            dir,
                        ),
                        services: Vec::new(),
                    });
                }
                "service" => {
                    let Some(rel) = relative(row) else { continue };
                    let Some(under) = rel.strip_prefix("service/") else {
                        continue;
                    };
                    let under = under.strip_suffix(".xml").unwrap_or(under);
                    let (dir, file_short) = under.rsplit_once('/').unwrap_or(("", under));
                    let at = *service_files.entry(row.path.clone()).or_insert_with(|| {
                        let mut file = entry(
                            row,
                            DocKind::Service,
                            Page::File(DocKind::Service, row.path.clone()),
                            file_short,
                            dir,
                        );
                        file.name = location_of(row, &rel);
                        file.summary = String::new();
                        file.line = 1;
                        file.id = String::new();
                        units.push(Unit {
                            stem: join(&["service", under]),
                            entry: file,
                            services: Vec::new(),
                        });
                        units.len() - 1
                    });
                    let prefix = moqui_core::service::define::service_prefix(Path::new(&row.path));
                    let local = row
                        .name
                        .strip_prefix(&format!("{prefix}."))
                        .unwrap_or(row.short_name.as_str())
                        .to_string();
                    let doc_name = local.replace('#', "-").replace(['/', '\\'], "_");
                    let service = entry(
                        row,
                        DocKind::Service,
                        Page::Artifact(DocKind::Service, row.id.clone()),
                        &local,
                        dir,
                    );
                    units[at].services.push((doc_name, service));
                }
                "seca" | "eeca" | "emeca" | "giteca" => {
                    let Some(rel) = relative(row) else { continue };
                    if rule_files.contains_key(&row.path) {
                        continue;
                    }
                    let under = rel.strip_suffix(".xml").unwrap_or(&rel);
                    let (dir, short) = under.rsplit_once('/').unwrap_or(("", under));
                    let mut file = entry(
                        row,
                        DocKind::Eca,
                        Page::File(DocKind::Eca, row.path.clone()),
                        short,
                        dir,
                    );
                    file.name = location_of(row, &rel);
                    file.summary = String::new();
                    file.line = 1;
                    file.id = String::new();
                    rule_files.insert(row.path.clone(), units.len());
                    units.push(Unit {
                        stem: join(&["eca", under]),
                        entry: file,
                        services: Vec::new(),
                    });
                }
                "rest" if row.parent.is_none() => {
                    let Some(rel) = relative(row) else { continue };
                    if rule_files.contains_key(&row.path) {
                        continue;
                    }
                    let under = rel.strip_prefix("service/").unwrap_or(&rel);
                    let under = under
                        .strip_suffix(".rest.xml")
                        .or_else(|| under.strip_suffix(".xml"))
                        .unwrap_or(under);
                    let (dir, short) = under.rsplit_once('/').unwrap_or(("", under));
                    let mut file = entry(
                        row,
                        DocKind::Rest,
                        Page::File(DocKind::Rest, row.path.clone()),
                        short,
                        dir,
                    );
                    file.name = location_of(row, &rel);
                    file.summary = row.summary.clone();
                    file.id = String::new();
                    rule_files.insert(row.path.clone(), units.len());
                    units.push(Unit {
                        stem: join(&["rest", under]),
                        entry: file,
                        services: Vec::new(),
                    });
                }
                _ => {}
            }
        }
        // two units at one stem: each takes its component's name
        let mut stems: HashMap<String, usize> = HashMap::new();
        for unit in &units {
            *stems.entry(unit.stem.clone()).or_default() += 1;
        }
        let mut plan = Self::default();
        for unit in units {
            let stem = if stems.get(&unit.stem).copied().unwrap_or(0) > 1 {
                format!("{}.{}", unit.stem, unit.entry.component)
            } else {
                unit.stem
            };
            let mut file = unit.entry;
            file.path = format!("{stem}.md");
            plan.add(file);
            for (doc_name, mut service) in unit.services {
                service.path = format!("{stem}/{doc_name}.md");
                plan.add(service);
            }
        }
        plan.add_indexes();
        plan
    }

    fn add(&mut self, entry: DocEntry) {
        let at = self.entries.len();
        if self.by_path.contains_key(&entry.path) {
            // a service whose name repeats in its file (a later definition replaces the first): one doc
            return;
        }
        self.by_path.insert(entry.path.clone(), at);
        match &entry.page {
            Page::Artifact(_, id) => {
                self.by_id.entry(id.clone()).or_insert(at);
            }
            Page::File(_, file) => {
                self.by_file.insert(file.clone(), at);
                self.by_location.insert(entry.name.clone(), at);
            }
            _ => {}
        }
        let packaged = match &entry.page {
            Page::Artifact(DocKind::Service, _) => None,
            Page::Artifact(kind, _) | Page::File(kind, _) => Some(*kind),
            _ => None,
        };
        if let Some(kind) = packaged {
            self.packages.entry((kind, entry.package.clone())).or_default().push(at);
        }
        self.entries.push(entry);
    }

    /// The root, each tree's index, and a page per package that has docs of its own (entity packages, service and
    /// screen directories; the docs at a tree's top are on its index).
    fn add_indexes(&mut self) {
        for list in self.packages.values_mut() {
            let entries = &self.entries;
            list.sort_by(|a, b| {
                entries[*a]
                    .title
                    .cmp(&entries[*b].title)
                    .then(entries[*a].path.cmp(&entries[*b].path))
            });
        }
        let mut pages = Vec::new();
        for (kind, key) in self.packages.keys() {
            if key.is_empty() || matches!(kind, DocKind::Eca | DocKind::Rest) {
                continue;
            }
            let dir = if *kind == DocKind::Entity {
                key.replace('.', "/")
            } else {
                key.clone()
            };
            pages.push(index_entry(
                format!("{}/{dir}/README.md", kind.as_str()),
                Page::Package(*kind, key.clone()),
                *kind,
                key,
            ));
        }
        for kind in DocKind::ALL {
            pages.push(index_entry(
                format!("{}/README.md", kind.as_str()),
                Page::TypeIndex(kind),
                kind,
                kind.title(),
            ));
        }
        pages.push(index_entry(
            "README.md".to_string(),
            Page::Root,
            DocKind::Entity,
            "Artifacts",
        ));
        for page in pages {
            let at = self.entries.len();
            self.by_path.insert(page.path.clone(), at);
            self.entries.push(page);
        }
    }

    #[must_use]
    pub fn at_path(&self, path: &str) -> Option<&DocEntry> {
        self.by_path.get(path).map(|at| &self.entries[*at])
    }

    /// The doc of an entity, service or screen id, or of an ECA rule or REST resource (its file's).
    #[must_use]
    pub fn of_id(&self, id: &str) -> Option<&DocEntry> {
        if let Some(at) = self.by_id.get(id) {
            return Some(&self.entries[*at]);
        }
        let (kind, rest) = id.split_once(':')?;
        match kind {
            "seca" | "eeca" | "emeca" | "giteca" => {
                let location = rest.split_once('#').map_or(rest, |(location, _)| location);
                self.by_location.get(location).map(|at| &self.entries[*at])
            }
            _ => None,
        }
    }

    /// The doc of a file: a service, ECA rules or REST API file.
    #[must_use]
    pub fn of_file(&self, path: &str) -> Option<&DocEntry> {
        self.by_file.get(path).map(|at| &self.entries[*at])
    }

    /// The doc an artifact is shown in: its own, or its owner's (a transition's or form's screen, a field's or
    /// relationship's entity, a parameter's owner, an ECA rule's or REST method's file).
    #[must_use]
    pub fn of_artifact(&self, row: &ArtifactRow) -> Option<&DocEntry> {
        if let Some(found) = self.of_id(&row.id) {
            return Some(found);
        }
        if matches!(row.kind.as_str(), "seca" | "eeca" | "emeca" | "giteca" | "rest") {
            if let Some(found) = self.of_file(&row.path) {
                return Some(found);
            }
        }
        owner_id(&row.id).and_then(|owner| self.of_id(&owner))
    }

    /// The members of a package, in title order.
    #[must_use]
    pub fn members(&self, kind: DocKind, key: &str) -> Vec<&DocEntry> {
        self.packages
            .get(&(kind, key.to_string()))
            .map(|list| list.iter().map(|at| &self.entries[*at]).collect())
            .unwrap_or_default()
    }

    /// The packages of a tree, with their member counts, in key order.
    #[must_use]
    pub fn packages_of(&self, kind: DocKind) -> Vec<(String, usize)> {
        self.packages
            .iter()
            .filter(|((of, _), _)| *of == kind)
            .map(|((_, key), list)| (key.clone(), list.len()))
            .collect()
    }

    /// A service file's services, in file order.
    #[must_use]
    pub fn services_of(&self, file: &str) -> Vec<&DocEntry> {
        let mut found: Vec<&DocEntry> = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.page, Page::Artifact(DocKind::Service, _)) && entry.file == file)
            .collect();
        found.sort_by_key(|entry| entry.line);
        found
    }

    /// The package page of a package, when it has one.
    #[must_use]
    pub fn package_page(&self, kind: DocKind, key: &str) -> Option<&DocEntry> {
        self.entries
            .iter()
            .find(|entry| entry.page == Page::Package(kind, key.to_string()))
    }

    /// The page a doc goes up to: a package's, else its tree's index; the root for a tree's index.
    #[must_use]
    pub fn parent_of(&self, doc: &DocEntry) -> Option<&DocEntry> {
        let up = match &doc.page {
            Page::Root => return None,
            Page::TypeIndex(_) => "README.md".to_string(),
            Page::Package(kind, _) => format!("{}/README.md", kind.as_str()),
            Page::Artifact(DocKind::Service, _) => {
                return self.of_file(&doc.file);
            }
            Page::Artifact(kind, _) | Page::File(kind, _) => {
                return self
                    .package_page(*kind, &doc.package)
                    .or_else(|| self.at_path(&format!("{}/README.md", kind.as_str())));
            }
        };
        self.at_path(&up)
    }
}

fn entry(row: &ArtifactRow, kind: DocKind, page: Page, title: &str, package: &str) -> DocEntry {
    DocEntry {
        path: String::new(),
        page,
        kind,
        title: title.to_string(),
        name: row.name.clone(),
        component: row.component.clone(),
        package: package.to_string(),
        summary: row.summary.clone(),
        file: row.path.clone(),
        line: row.line,
        id: row.id.clone(),
    }
}

fn index_entry(path: String, page: Page, kind: DocKind, title: &str) -> DocEntry {
    let package = match &page {
        Page::Package(_, key) => key.clone(),
        _ => String::new(),
    };
    DocEntry {
        path,
        page,
        kind,
        title: title.to_string(),
        name: title.to_string(),
        component: String::new(),
        package,
        summary: String::new(),
        file: String::new(),
        line: 0,
        id: String::new(),
    }
}

fn location_of(row: &ArtifactRow, relative: &str) -> String {
    format!("component://{}/{relative}", row.component)
}

/// The non-empty parts joined by `/`.
fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// The id of the artifact that owns `id` in the docs: a screen's part's screen, an entity's part's entity, a
/// parameter's owner (and its owner's, for a transition's parameter).
#[must_use]
pub fn owner_id(id: &str) -> Option<String> {
    let (kind, rest) = id.split_once(':')?;
    match kind {
        "transition" | "form" | "form-field" | "subscreen" | "ext" | "anchor" | "widget-template" => {
            let location = rest.split_once('#').map_or(rest, |(location, _)| location);
            Some(format!("screen:{location}"))
        }
        "field" => rest.rsplit_once('.').map(|(entity, _)| format!("entity:{entity}")),
        "relationship" | "index" | "view-member" => rest.split_once('@').map(|(entity, _)| format!("entity:{entity}")),
        "extend-entity" => rest.rsplit_once('#').map(|(_, entity)| format!("entity:{entity}")),
        "parameter" => {
            let (owner, _) = rest.rsplit_once('@')?;
            if owner.starts_with("service:") || owner.starts_with("screen:") || owner.starts_with("entity:") {
                Some(owner.to_string())
            } else {
                owner_id(owner).or_else(|| Some(owner.to_string()))
            }
        }
        _ => None,
    }
}

/// The link from the doc at `from` to the doc at `to`, both tree paths: relative, so a tree reads the same wherever it is
/// put; the characters a Markdown link destination does not take as they are, escaped.
#[must_use]
pub fn relative_link(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let from_dirs = &from_dirs[..from_dirs.len().saturating_sub(1)];
    let to_parts: Vec<&str> = to.split('/').collect();
    let (to_dirs, to_file) = to_parts.split_at(to_parts.len().saturating_sub(1));
    let common = from_dirs.iter().zip(to_dirs.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_dirs.len() {
        parts.push("..".to_string());
    }
    parts.extend(to_dirs[common..].iter().map(|part| (*part).to_string()));
    parts.extend(to_file.iter().map(|part| (*part).to_string()));
    let text = parts.join("/");
    text.chars()
        .map(|ch| match ch {
            ' ' => "%20".to_string(),
            '(' => "%28".to_string(),
            ')' => "%29".to_string(),
            '<' => "%3C".to_string(),
            '>' => "%3E".to_string(),
            '#' => "%23".to_string(),
            other => other.to_string(),
        })
        .collect()
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
