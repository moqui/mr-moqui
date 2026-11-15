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

//! `get#ArtifactBrowse`: the Artifact Browser's graph for the terminal (plans/design-dev-aide.md 7.3, D14), the answer
//! of `Artifacts/BrowseTui`'s `browse` transition that the `artifact-browser` component draws. An artifact as
//! `get#ArtifactDetail` shows it with the browser's links, and its references both ways as `get#Refs` reads them,
//! grouped into sections by the other side's kind and filtered as the browser filters them ([`sections`], the port of
//! `pure.browserSections` in `devstatic/DevAideBrowser.js`, and [`center_links`], its `centerLinks`).

use moqui_core::context::ExecutionContext;
use moqui_core::native::{Parameters, ServiceInfo};
use moqui_devkit::index::{self, Direction};
use moqui_lib::error::Error as LibError;
use serde_json::{json, Value as Json};

use crate::artifacts::{encode, list, shown, shown_ref};
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

/// The framework's own components, hidden by default unless the center is one of theirs (`pure.frameworkComponents`).
pub(crate) const FRAMEWORK_COMPONENTS: [&str; 3] = ["framework", "webroot", "tools"];

/// What the browser shows of the references: the browser's checkboxes and its kinds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Filter {
    /// Leave out the references whose other side is in a framework component.
    pub(crate) hide_framework: bool,
    /// Only these kinds of the other side; all when empty.
    pub(crate) kinds: Vec<String>,
    /// The references only observed (calls no file names).
    pub(crate) observed: bool,
    /// The references read from the files.
    pub(crate) static_links: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            hide_framework: false,
            kinds: Vec::new(),
            observed: true,
            static_links: true,
        }
    }
}

fn str_of<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or("")
}

/// The first of `values` that is not empty, else `""`.
fn first<'a>(values: &[&'a str]) -> &'a str {
    values.iter().copied().find(|value| !value.is_empty()).unwrap_or("")
}

/// The kind of a reference's other side: the indexed artifact's, else what the reference names. An incoming
/// reference's other side is its `from` (`screen:component://…`), whose kind is the id's prefix.
fn other_kind(entry: &Json, direction: Direction) -> String {
    let other = entry.get("other").unwrap_or(&Json::Null);
    let named = match direction {
        Direction::In => str_of(entry, "from").split_once(':').map_or("", |(kind, _)| kind),
        Direction::Out => str_of(entry, "toKind"),
    };
    let kind = first(&[str_of(other, "kind"), named]);
    if kind.is_empty() { "other" } else { kind }.to_string()
}

/// The references of an artifact (`get#Refs`'s `refs`, each as `shown_ref` makes it) as sections by the other side's
/// kind, sorted by kind: `{kind, count, rows}`, each row `{id, kind, shortName, name, component, how, calls,
/// observedOnly, resolved, url}` and one row per other side, its ways of referring joined and its calls added. Rows of
/// one short name in a section (orderId of three entities) are told apart by the last two parts of their full name.
pub(crate) fn sections(refs: &[Json], direction: Direction, filter: &Filter) -> Vec<Json> {
    let mut sections: std::collections::BTreeMap<String, Vec<Json>> = std::collections::BTreeMap::new();
    for entry in refs {
        let other = entry.get("other").unwrap_or(&Json::Null);
        let kind = other_kind(entry, direction);
        if !filter.kinds.is_empty() && !filter.kinds.contains(&kind) {
            continue;
        }
        if filter.hide_framework && FRAMEWORK_COMPONENTS.contains(&str_of(other, "component")) {
            continue;
        }
        let observed_only = entry.get("observedOnly").and_then(Json::as_bool).unwrap_or(false);
        if if observed_only {
            !filter.observed
        } else {
            !filter.static_links
        } {
            continue;
        }
        let named = match direction {
            Direction::In => str_of(entry, "from").to_string(),
            Direction::Out => first(&[str_of(entry, "toId"), str_of(entry, "toName")]).to_string(),
        };
        let fallback = match direction {
            Direction::Out if str_of(entry, "toId").is_empty() => format!("{kind}:{}", str_of(entry, "toName")),
            _ => named.clone(),
        };
        let id = first(&[str_of(other, "id"), fallback.as_str()]).to_string();
        let how = str_of(entry, "refType");
        let calls = entry.get("calls").and_then(Json::as_i64).unwrap_or(0);
        let rows = sections.entry(kind.clone()).or_default();
        if let Some(have) = rows.iter_mut().find(|row| row["id"] == json!(id)) {
            have["calls"] = json!(have["calls"].as_i64().unwrap_or(0) + calls);
            let ways = have["how"].as_str().unwrap_or("").to_string();
            if !how.is_empty() && !ways.split(", ").any(|way| way == how) {
                have["how"] = json!(if ways.is_empty() {
                    how.to_string()
                } else {
                    format!("{ways}, {how}")
                });
            }
            continue;
        }
        let to_name = match direction {
            Direction::In => named.as_str(),
            Direction::Out => str_of(entry, "toName"),
        };
        rows.push(json!({
            "id": id,
            "kind": kind,
            "shortName": first(&[str_of(other, "shortName"), to_name, id.as_str()]),
            "name": first(&[str_of(other, "name"), to_name, id.as_str()]),
            "component": str_of(other, "component"),
            "how": how,
            "calls": calls,
            "observedOnly": observed_only,
            "resolved": !str_of(other, "id").is_empty(),
            "url": first(&[str_of(entry, "url"), str_of(other, "url")]),
        }));
    }
    sections
        .into_iter()
        .map(|(kind, mut rows)| {
            let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
            for row in &rows {
                *counts.entry(str_of(row, "shortName").to_string()).or_default() += 1;
            }
            for row in &mut rows {
                let short = str_of(row, "shortName");
                if counts.get(short).copied().unwrap_or(0) > 1 {
                    if let Some(owner) = owner_name(str_of(row, "name"), str_of(row, "id"), short) {
                        row["shortName"] = json!(owner);
                    }
                }
            }
            json!({"kind": kind, "count": rows.len(), "rows": rows})
        })
        .collect()
}

/// A row's name told apart from others of its short name: the last two parts of its full name, or, when the index
/// names it by its short name alone (a form or transition, `ExampleList`), of its id's location
/// (`form:component://…/Search.xml#ExampleList` is `Search.xml#ExampleList`, a transition's `:any` left off). None
/// when neither says more than the short name.
pub(crate) fn owner_name(name: &str, id: &str, short: &str) -> Option<String> {
    let full = if name != short {
        name
    } else {
        let location = id.split_once(':').map_or("", |(_, rest)| rest);
        match location.split_once('#') {
            Some((file, anchor)) => {
                let anchor = anchor.split(':').next().unwrap_or(anchor);
                return Some(format!("{}#{anchor}", last_parts(file, 2)));
            }
            None if !location.is_empty() && location != short => location,
            None => return None,
        }
    };
    Some(last_parts(full, 2))
}

fn last_parts(text: &str, count: usize) -> String {
    let parts: Vec<&str> = text.split(['.', ':', '/']).collect();
    parts[parts.len().saturating_sub(count)..].join(".")
}

/// The kinds of the other sides of `refs`, sorted, for the browser's kind filter.
pub(crate) fn kinds(refs: &[(Direction, &Json)]) -> Vec<String> {
    let mut kinds: Vec<String> = refs
        .iter()
        .map(|(direction, entry)| other_kind(entry, *direction))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// The center's links, each with the key the terminal follows it by: Text (the Workspace at its line), Try (a
/// service's writer), Data (an entity's modeler), Preview (a screen's builder), Refs (the lists) and Doc.
pub(crate) fn center_links(artifact: &Json) -> Vec<Json> {
    let id = encode(str_of(artifact, "id"));
    let kind = str_of(artifact, "kind");
    let mut links = Vec::new();
    let mut link = |key: &str, label: &str, url: String| links.push(json!({"key": key, "label": label, "url": url}));
    if !str_of(artifact, "url").is_empty() {
        link("t", "Text", str_of(artifact, "url").to_string());
    }
    if kind == "service" {
        link("y", "Try", format!("/qapps/dev/Services/Edit?artifact={id}"));
    }
    if kind == "entity" || kind == "view-entity" {
        link("d", "Data", format!("/qapps/dev/Entities/Edit?artifact={id}"));
    }
    let root = str_of(artifact, "rootId");
    if kind == "screen" && !root.is_empty() {
        let path = encode(str_of(artifact, "relativePath"));
        link(
            "p",
            "Preview",
            format!("/qapps/dev/Screens/Design?rootId={}&path={path}", encode(root)),
        );
    }
    link("r", "Refs", format!("/qapps/dev/Artifacts/Refs?artifactId={id}"));
    link("D", "Doc", format!("/qapps/dev/Artifacts/Doc?artifactId={id}"));
    links
}

/// The center's lines under its name: the full name, where it is defined, its summary.
pub(crate) fn center_lines(artifact: &Json) -> Vec<String> {
    let path = first(&[str_of(artifact, "relativePath"), str_of(artifact, "path")]);
    let file = path.rsplit('/').next().unwrap_or(path);
    let line = artifact.get("line").and_then(Json::as_u64).unwrap_or(0);
    let site = if artifact.get("site").and_then(Json::as_bool).unwrap_or(false) {
        " · site"
    } else {
        ""
    };
    let mut lines = vec![
        str_of(artifact, "name").to_string(),
        format!("{}{site} · {file}:{line}", str_of(artifact, "component")),
    ];
    let summary = str_of(artifact, "summary");
    if !summary.is_empty() {
        lines.push(summary.to_string());
    }
    lines
}

/// A Y/N parameter, else `default` when it is not given.
fn flag_or(parameters: &Parameters, key: &str, default: bool) -> bool {
    match text(parameters, key).as_str() {
        "Y" | "y" | "true" | "1" => true,
        "N" | "n" | "false" | "0" => false,
        _ => default,
    }
}

/// `get#ArtifactBrowse`: the center, what uses it and what it uses in sections, the kinds there are, and the filter
/// applied. The framework's own artifacts are hidden unless the center is one of them or `hideFramework` says.
fn get_artifact_browse(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut out = Parameters::new();
    let asked = text(parameters, "artifactId");
    let Some(artifact) = index::named(&context.factory, &asked)? else {
        out.insert("center".into(), moqui_lib::object::Value::Null);
        out.insert(
            "message".into(),
            json_value(&format!("No artifact {asked} in the index")),
        );
        out.insert("usedBy".into(), json_value(&Vec::<Json>::new()));
        out.insert("uses".into(), json_value(&Vec::<Json>::new()));
        return Ok(out);
    };
    let users: Vec<Json> = index::refs(&context.factory, &artifact.id, Direction::In, &[])?
        .iter()
        .map(|entry| shown_ref(&context, entry))
        .collect();
    let uses: Vec<Json> = index::refs(&context.factory, &artifact.id, Direction::Out, &[])?
        .iter()
        .map(|entry| shown_ref(&context, entry))
        .collect();
    let framework = FRAMEWORK_COMPONENTS.contains(&artifact.component.as_str());
    let filter = Filter {
        hide_framework: flag_or(parameters, "hideFramework", !framework),
        kinds: list(parameters, "kinds"),
        observed: flag_or(parameters, "observed", true),
        static_links: flag_or(parameters, "staticLinks", true),
    };
    let all: Vec<(Direction, &Json)> = users
        .iter()
        .map(|entry| (Direction::In, entry))
        .chain(uses.iter().map(|entry| (Direction::Out, entry)))
        .collect();
    let mut center = shown(&context, &artifact);
    center["lines"] = json!(center_lines(&center));
    center["links"] = json!(center_links(&center));
    out.insert("center".into(), json_value(&center));
    out.insert("usedBy".into(), json_value(&sections(&users, Direction::In, &filter)));
    out.insert("uses".into(), json_value(&sections(&uses, Direction::Out, &filter)));
    out.insert("kinds".into(), json_value(&kinds(&all)));
    out.insert(
        "filter".into(),
        json_value(&json!({"kinds": filter.kinds, "hideFramework": filter.hide_framework,
            "observed": filter.observed, "staticLinks": filter.static_links})),
    );
    Ok(out)
}

pub(crate) fn register(registry: &mut moqui_core::native::NativeRegistry) -> Result<(), moqui_core::context::Error> {
    registry.service_function(moqui_core::service_location!(get_artifact_browse), get_artifact_browse)
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
