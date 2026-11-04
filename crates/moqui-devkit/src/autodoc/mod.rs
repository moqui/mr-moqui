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

//! Autodoc (plans/archive/Autodoc.md): Markdown docs of the loaded artifacts, like JavaDoc for Moqui, made by templates
//! (`component://framework/template/autodoc/*.fancy`, which a site component's `template/autodoc/` overrides by name).
//! One doc per entity, service, screen, ECA rules file and REST API file, a page per entity package and service and
//! screen directory, an index per tree and a root, with Mermaid diagrams: what uses an artifact to its left and what
//! it uses to its right, an E-R diagram per entity package, a class diagram per service directory and the flowcharts of
//! screen trees, REST APIs and ECA rules.
//!
//! The Dev AIDE shows one doc at a time from a dev server's index ([`source::DbSource`], [`render_doc`]) and exports a
//! tree into a repository; `moqui autodoc` writes a tree from a memory index ([`source::MemorySource`], [`generate`],
//! [`write`]). Both need an entered ExecutionContext: the templates are run as the context's scripts are.

pub mod describe;
pub mod mermaid;
mod model;
pub mod plan;
mod render;
pub mod source;
pub mod write;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde::Serialize;

pub use plan::{DocEntry, DocKind, DocPlan, Page};
pub use source::{DbSource, DocIndex, MemorySource};

/// What to document and how.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The trees to write; empty for all.
    pub kinds: Vec<DocKind>,
    /// The components whose artifacts to write; empty for all.
    pub components: Vec<String>,
    /// Package prefixes to write (`mantle.order`, `mantle/order`, `SimpleScreens/Order`), by whole segments; empty for
    /// all.
    pub packages: Vec<String>,
    /// Put each service's definition in its doc, at most 200 lines.
    pub include_source: bool,
    /// Directories (locations) whose templates take the place of the framework's of the same name, first first.
    pub template_dirs: Vec<String>,
    /// The doc paths a tree has already (its manifest's): listed and linked with the ones written now.
    pub previous: Vec<String>,
}

/// One doc made.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocFile {
    pub path: String,
    pub title: String,
    pub kind: DocKind,
    /// `artifact`, `file`, `package`, `index` or `root`.
    pub page: &'static str,
    /// The index id of an artifact doc; empty for the others.
    pub artifact_id: String,
    pub text: String,
}

/// A doc that was not made, and why.
#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub path: String,
    pub message: String,
}

/// What a run made.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Generated {
    pub files: Vec<DocFile>,
    pub failures: Vec<Failure>,
}

/// Each loaded component's name and its directory, canonical as the index's paths are.
#[must_use]
pub fn component_dirs(factory: &ExecutionContextFactory) -> Vec<(String, String)> {
    factory
        .components()
        .iter()
        .map(|component| {
            let dir = std::fs::canonicalize(&component.location).unwrap_or_else(|_| component.location.clone());
            (component.name.clone(), dir.display().to_string())
        })
        .collect()
}

/// The plan of every doc of `source`'s artifacts.
///
/// # Errors
///
/// Returns an error when the index cannot be read.
pub fn build_plan(factory: &ExecutionContextFactory, source: &dyn DocIndex) -> Result<DocPlan, Error> {
    let kinds: Vec<&str> = DocKind::ALL
        .iter()
        .flat_map(|kind| kind.index_kinds().iter().copied())
        .collect();
    let rows = source.artifacts(&kinds)?;
    Ok(DocPlan::build(&rows, &component_dirs(factory)))
}

/// The last plan made of a factory's index, with the stamp it was made at.
#[derive(Default)]
struct PlanCache(Mutex<Option<(String, Arc<DocPlan>)>>);

/// [`build_plan`], kept on the factory while `source`'s stamp stays the same (the viewer's every doc).
///
/// # Errors
///
/// Returns an error when the index cannot be read.
pub fn cached_plan(factory: &Arc<ExecutionContextFactory>, source: &dyn DocIndex) -> Result<Arc<DocPlan>, Error> {
    let cache = factory.extension(|_| PlanCache::default());
    let stamp = source.stamp();
    if !stamp.is_empty() {
        if let Some((kept, plan)) = lock(&cache.0).as_ref() {
            if *kept == stamp {
                return Ok(Arc::clone(plan));
            }
        }
    }
    let plan = Arc::new(build_plan(factory, source)?);
    *lock(&cache.0) = Some((stamp, Arc::clone(&plan)));
    Ok(plan)
}

/// The doc at a tree path, or of an artifact id (an entity's field opens its entity's doc), with every doc of the plan
/// linked (the viewer).
///
/// # Errors
///
/// Returns an error when there is no such doc, or it does not build or render.
pub fn render_doc(
    factory: &Arc<ExecutionContextFactory>,
    source: &dyn DocIndex,
    plan: &DocPlan,
    options: &Options,
    key: &str,
) -> Result<DocFile, Error> {
    let doc = find(plan, key).ok_or_else(|| Error::new(format!("no doc of {key}")))?;
    let mut run = model::Run::new(factory, source, plan, options, None);
    make(&mut run, doc)
}

/// The doc a key names: a tree path, an artifact id, or the id of an artifact's part.
#[must_use]
pub fn find<'p>(plan: &'p DocPlan, key: &str) -> Option<&'p DocEntry> {
    let key = key.trim().trim_start_matches('/');
    plan.at_path(key)
        .or_else(|| plan.of_id(key))
        .or_else(|| plan::owner_id(key).and_then(|owner| plan.of_id(&owner)))
}

fn make(run: &mut model::Run<'_>, doc: &DocEntry) -> Result<DocFile, Error> {
    let data = run.data(doc)?;
    let text = render::render(run.factory, run.options, render::template_of(&doc.page), &data)?;
    Ok(DocFile {
        path: doc.path.clone(),
        title: doc.title.clone(),
        kind: doc.kind,
        page: match doc.page {
            Page::Artifact(..) => "artifact",
            Page::File(..) => "file",
            Page::Package(..) => "package",
            Page::TypeIndex(_) => "index",
            Page::Root => "root",
        },
        artifact_id: doc.id.clone(),
        text,
    })
}

/// Whether the options take a unit doc (an artifact's or a file's).
fn selected(options: &Options, doc: &DocEntry) -> bool {
    if !options.kinds.is_empty() && !options.kinds.contains(&doc.kind) {
        return false;
    }
    if !options.components.is_empty() && !options.components.contains(&doc.component) {
        return false;
    }
    if options.packages.is_empty() {
        return true;
    }
    let segments = |text: &str| -> Vec<String> {
        text.split(['.', '/'])
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };
    let package = segments(&doc.package);
    // an ECA file's package is its directory in the component, `service/...` or `entity/...`
    let within = if doc.kind == DocKind::Eca {
        package.get(1..).unwrap_or_default().to_vec()
    } else {
        package.clone()
    };
    options.packages.iter().any(|prefix| {
        let prefix = segments(prefix);
        !prefix.is_empty() && (package.starts_with(&prefix) || within.starts_with(&prefix))
    })
}

/// The docs `options` select, the pages of their packages and trees and the root, each made on its own: one that does
/// not build or render is logged and listed in the failures, and the rest go on.
#[must_use]
pub fn generate(
    factory: &Arc<ExecutionContextFactory>,
    source: &dyn DocIndex,
    plan: &DocPlan,
    options: &Options,
) -> Generated {
    let units: Vec<&DocEntry> = plan
        .entries
        .iter()
        .filter(|doc| matches!(doc.page, Page::Artifact(..) | Page::File(..)) && selected(options, doc))
        .collect();
    let mut pages: Vec<&DocEntry> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for unit in &units {
        let mut up = plan.parent_of(unit);
        while let Some(page) = up {
            if !matches!(page.page, Page::File(..)) && seen.insert(page.path.as_str()) {
                pages.push(page);
            }
            up = plan.parent_of(page);
        }
    }
    if let Some(root) = plan.at_path("README.md") {
        if seen.insert(root.path.as_str()) {
            pages.push(root);
        }
    }
    let mut listed: HashSet<String> = options.previous.iter().cloned().collect();
    listed.extend(units.iter().map(|doc| doc.path.clone()));
    listed.extend(pages.iter().map(|doc| doc.path.clone()));
    let mut run = model::Run::new(factory, source, plan, options, Some(listed));
    let mut generated = Generated::default();
    for doc in units.into_iter().chain(pages) {
        match make(&mut run, doc) {
            Ok(file) => generated.files.push(file),
            Err(err) => {
                tracing::error!("Autodoc: the doc {} was not made: {err}", doc.path);
                generated.failures.push(Failure {
                    path: doc.path.clone(),
                    message: err.to_string(),
                });
            }
        }
    }
    generated
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
