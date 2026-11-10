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

//! The merged conf as System › Conf shows it (docs/tools/Component-Deploy.md, "Viewing and editing the conf"): the
//! files merged again with each value's file and line recorded ([`moqui_lib::node::with_merge_sources`]), each element
//! and attribute with its reload class from the [`catalog`](super::catalog), the default conf's comments beside its
//! elements, and the values the files hold that are not running yet. For the editor of one file, the same for each
//! element of that file by its line, and a preview of what saving an edited text would change.

use std::collections::HashMap;
use std::path::Path;

use moqui_lib::env;
use moqui_lib::node::{with_merge_sources, MNode, Source};
use moqui_lib::xml_edit::comments::{element_comments, ElementComments};
use serde::Serialize;

use super::catalog::{Class, Key, Unit, CATALOG, UNKNOWN};
use super::diff::{join, keyed};
use super::{conf_files, preview_conf, restart_reason, ConfChange, ConfReport};
use crate::context::conf::{merge_files, ConfFile, ConfRole};
use crate::context::{Error, ExecutionContextFactory};

/// The text shown for a value that is not shown.
pub const MASKED: &str = "••••••";

/// One conf file of the merge.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub index: usize,
    /// `default`, `component`, `component-test` or `runtime`.
    pub role: &'static str,
    pub component: Option<String>,
    /// A short name: `Default`, the component's name, `Runtime`.
    pub label: String,
    pub path: String,
    /// Whether System › Conf › Edit may save it: not the default conf, and a file the server can write.
    pub editable: bool,
}

/// A file and a line in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct At {
    pub file: usize,
    pub line: u32,
}

/// One attribute of an element of the merged conf.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewAttr {
    pub name: String,
    /// As the file has it; masked for a secret.
    pub value: String,
    /// The value with its `${}` expanded, where it has any; masked when a secret goes into it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<String>,
    /// The file and line it was set in, when not the element's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<At>,
    /// Its class and reason, when not the element's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The change of it the files hold that is not running yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
}

/// A value in the files that is not in the running conf.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    /// `would-apply` (a reload applies it), `pending-restart` or `no-effect`.
    pub outcome: &'static str,
    /// The running value; masked for a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
    pub reason: String,
}

/// One element of the merged conf.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewNode {
    pub name: String,
    /// The values of its key attributes, as the merge keys it: `webroot`, `type,name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The unit it is part of, as the reload names it: `webapp-list/webapp[webroot]`.
    pub path: String,
    /// `live`, `apply` or `restart`, and why for `restart`.
    pub class: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The element's own file and line first, then each file merged into it.
    pub sources: Vec<At>,
    pub attrs: Vec<ViewAttr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The default conf's comments before the element and on its line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<String>,
    /// The default conf's comments after its last child.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trailing: Vec<String>,
    /// A change of the element itself (added, removed, its content) the files hold that is not running yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// For a `default-property`, the value the server has when something other than the conf set it to another value:
    /// the environment, `runtime/.env` or `-D`; masked for a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ViewNode>,
}

/// The merged conf for System › Conf.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfView {
    pub files: Vec<FileInfo>,
    pub root: ViewNode,
    /// The changes the files hold that a reload would apply, and those that wait for a restart.
    pub would_apply: usize,
    pub pending_restart: usize,
}

/// One element of a conf file, for the editor.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineInfo {
    pub line: u32,
    pub name: String,
    pub path: String,
    /// Whether the element is a unit of the reload by itself, not part of its parent's.
    pub unit: bool,
    pub class: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The attributes whose class is not the element's: (name, class, reason).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<(String, &'static str, String)>,
    /// The default conf's comments of the element it merges into.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<String>,
}

/// A problem with an edited conf text.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    /// `error` or `warning`.
    pub severity: &'static str,
    pub message: String,
}

/// What an edited text of one conf file would do.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCheck {
    pub problems: Vec<Problem>,
    pub lines: Vec<LineInfo>,
    /// The preview of a reload with the text saved; none when it does not parse.
    pub report: Option<ConfReport>,
}

/// The conf files of the running server, with their roles and whether each can be saved.
///
/// # Errors
///
/// When the runtime conf named is missing.
pub fn files(factory: &ExecutionContextFactory) -> Result<Vec<ConfFile>, Error> {
    conf_files(factory, &factory.components())
}

/// [`FileInfo`] for each of `files`.
#[must_use]
pub fn file_infos(files: &[ConfFile]) -> Vec<FileInfo> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let (role, component, label) = match &file.role {
                ConfRole::Default => ("default", None, "Default".to_string()),
                ConfRole::Component(name) => ("component", Some(name.clone()), name.clone()),
                ConfRole::ComponentTest(name) => ("component-test", Some(name.clone()), format!("{name} (test)")),
                ConfRole::Runtime => ("runtime", None, "Runtime".to_string()),
            };
            FileInfo {
                index,
                role,
                component,
                label,
                path: file.location(),
                editable: file.role != ConfRole::Default && writable(&file.path),
            }
        })
        .collect()
}

fn writable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| !meta.permissions().readonly())
        && path
            .parent()
            .is_some_and(|dir| std::fs::metadata(dir).is_ok_and(|meta| !meta.permissions().readonly()))
}

/// The merged conf of the files, with where each value came from, its class, and what is not running yet.
///
/// # Errors
///
/// When a conf file does not parse, or the running conf does not.
pub fn view(factory: &ExecutionContextFactory) -> Result<ConfView, Error> {
    let files = files(factory)?;
    let merged = with_merge_sources(|| merge_files(&files, None))?;
    let report = preview_conf(factory, &merged)?;
    let set_by_conf = factory.conf_properties().clone();
    Ok(build_view(&files, &merged, Some(&report), &set_by_conf))
}

/// [`view`] of a merged tree: `report` is the preview against the running conf, `set_by_conf` the properties the conf
/// set at boot or since, by name.
#[must_use]
pub fn build_view(
    files: &[ConfFile],
    merged: &MNode,
    report: Option<&ConfReport>,
    set_by_conf: &HashMap<String, String>,
) -> ConfView {
    let walk = Walk::new(files, report, set_by_conf);
    let root = walk.node(merged, None, "", UNKNOWN, None);
    let changes = report.map(|report| report.changes.as_slice()).unwrap_or_default();
    ConfView {
        files: file_infos(files),
        root,
        would_apply: changes
            .iter()
            .filter(|change| change.outcome == super::Outcome::WouldApply)
            .count(),
        pending_restart: changes
            .iter()
            .filter(|change| change.outcome == super::Outcome::PendingRestart)
            .count(),
    }
}

/// What saving `text` as conf file `index` would do: its problems, its elements by line with their classes and the
/// default conf's comments, and the preview of the reload.
///
/// # Errors
///
/// When another conf file does not parse, or the running conf does not.
pub fn check_file(factory: &ExecutionContextFactory, index: usize, text: &str) -> Result<FileCheck, Error> {
    let files = files(factory)?;
    let Some(file) = files.get(index) else {
        return Err(Error::new(format!("no conf file {index}")));
    };
    let problems = text_problems(&file.location(), text);
    if !problems.is_empty() {
        return Ok(FileCheck {
            problems,
            ..FileCheck::default()
        });
    }
    let merged = with_merge_sources(|| merge_files(&files, Some((&file.path, text))))?;
    let report = preview_conf(factory, &merged)?;
    Ok(FileCheck {
        problems,
        lines: file_lines(&files, index, &merged),
        report: Some(report),
    })
}

/// The problems that keep `text` from being a conf file: it does not parse, or its root is not `moqui-conf`.
#[must_use]
pub fn text_problems(location: &str, text: &str) -> Vec<Problem> {
    match MNode::parse_text(location, text) {
        Err(err) => {
            let message = err.to_string();
            let (line, column) = line_column_of(&message);
            vec![Problem {
                line,
                column,
                severity: "error",
                message,
            }]
        }
        Ok(root) if !root.has_name("moqui-conf") => vec![Problem {
            line: root.line() as usize,
            column: root.column() as usize,
            severity: "error",
            message: format!("The root element is {}; a conf file's is moqui-conf", root.name()),
        }],
        Ok(_) => Vec::new(),
    }
}

/// The line and column a parse error names at its end (`... (file.xml:3:7)`), else the first line.
fn line_column_of(message: &str) -> (usize, usize) {
    let place = message.rsplit_once('(').map(|(_, place)| place.trim_end_matches(')'));
    let mut numbers = place
        .into_iter()
        .flat_map(|place| place.rsplitn(3, ':'))
        .map(str::parse::<usize>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(column)), Some(Ok(line))) => (line, column),
        _ => (1, 1),
    }
}

/// Each element of conf file `index` that went into `merged`, by its line in the file; a line with several elements has
/// one for each, outermost first.
#[must_use]
pub fn file_lines(files: &[ConfFile], index: usize, merged: &MNode) -> Vec<LineInfo> {
    let none = HashMap::new();
    let walk = Walk::new(files, None, &none);
    let mut out = Vec::new();
    walk.lines(merged, None, "", UNKNOWN, index, &mut out);
    // by line, the elements of one line outermost first; an element merged into two nodes (an override of a keyed
    // child and its parent) once
    out.sort_by_key(|line| line.line);
    out.dedup_by(|later, earlier| {
        later.line == earlier.line && later.path == earlier.path && later.name == earlier.name
    });
    out
}

/// The walk of the merged tree beside the catalog, as the reload's diff walks it, so a node's path is the reload's.
struct Walk<'a> {
    file_index: HashMap<String, usize>,
    default_location: Option<String>,
    comments: HashMap<u32, ElementComments>,
    changes: &'a [ConfChange],
    set_by_conf: &'a HashMap<String, String>,
}

impl<'a> Walk<'a> {
    fn new(files: &[ConfFile], report: Option<&'a ConfReport>, set_by_conf: &'a HashMap<String, String>) -> Self {
        let default = files.iter().find(|file| file.role == ConfRole::Default);
        let comments = default
            .and_then(|file| std::fs::read_to_string(&file.path).ok())
            .and_then(|text| element_comments(&text).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|comments| (u32::try_from(comments.line).unwrap_or(u32::MAX), comments))
            .collect();
        Self {
            file_index: files
                .iter()
                .enumerate()
                .map(|(at, file)| (file.location(), at))
                .collect(),
            default_location: default.map(ConfFile::location),
            comments,
            changes: report.map(|report| report.changes.as_slice()).unwrap_or_default(),
            set_by_conf,
        }
    }

    fn at(&self, source: &Source) -> Option<At> {
        self.file_index.get(source.location.as_ref()).map(|&file| At {
            file,
            line: source.line,
        })
    }

    /// The default conf's comments of `node`: those of its own element when it is the default conf's.
    fn comments_of(&self, node: &MNode) -> Option<&ElementComments> {
        let source = node.source()?;
        if self.default_location.as_deref() != Some(source.location.as_ref()) {
            return None;
        }
        self.comments.get(&source.line)
    }

    /// Each child of `node` with the unit it is, if any, and its path: the catalog's units by name, keyed as the diff
    /// keys them; a child no unit names is part of `node`'s unit (at the top, a unit of its own name).
    fn children(&self, node: &MNode, units: &'a [Unit], path: &str) -> Vec<(MNode, Option<&'a Unit>, String)> {
        let mut keys: HashMap<&str, Vec<(String, MNode)>> = HashMap::new();
        for unit in units {
            if let Key::By(attrs) = unit.key {
                keys.insert(unit.name, keyed(Some(node), unit.name, attrs));
            }
        }
        node.children()
            .into_iter()
            .map(|child| {
                let unit = units.iter().find(|unit| child.has_name(unit.name));
                let child_path = match unit {
                    Some(unit) => match unit.key {
                        Key::By(_) => {
                            let key = keys
                                .get(unit.name)
                                .and_then(|found| found.iter().find(|(_, held)| held.same_node(&child)))
                                .map(|(key, _)| key.clone())
                                .unwrap_or_default();
                            format!("{}[{key}]", join(path, unit.name))
                        }
                        Key::Single | Key::Sequence => join(path, unit.name),
                    },
                    None if path.is_empty() => child.name(),
                    None => path.to_string(),
                };
                (child, unit, child_path)
            })
            .collect()
    }

    fn node(
        &self,
        node: &MNode,
        unit: Option<&'a Unit>,
        path: &str,
        inherited: Class,
        key: Option<String>,
    ) -> ViewNode {
        let class = unit.map_or(inherited, |unit| unit.class);
        let own = node.source();
        let mut sources: Vec<At> = own.as_ref().and_then(|source| self.at(source)).into_iter().collect();
        for source in node.merge_sources() {
            if let Some(at) = self.at(&source) {
                if !sources.contains(&at) {
                    sources.push(at);
                }
            }
        }
        let is_property = node.has_name("default-property") && path.starts_with("default-property[");
        let property_name = is_property.then(|| node.attribute("name").unwrap_or_default());
        let property_secret = property_name.as_deref().is_some_and(|name| {
            crate::context::factory::property_is_secret(name, node.attribute("is-secret").as_deref())
        });
        let own_change = unit.is_some().then(|| self.change(path, None)).flatten();
        let attrs = node
            .attributes()
            .into_iter()
            .map(|(name, value)| {
                let attr_class = unit.map_or(class, |unit| unit.attr_class(&name));
                let secret = (property_secret && name == "value") || secret_attribute(&name);
                let source = node
                    .attribute_source(&name)
                    .and_then(|source| self.at(&source))
                    .filter(|at| sources.first() != Some(at));
                let expanded = value
                    .contains("${")
                    .then(|| env::expand(&value).unwrap_or_default())
                    .map(|expanded| {
                        if secret || refers_to_secret(&value) {
                            MASKED.to_string()
                        } else {
                            expanded
                        }
                    });
                let pending = unit.and_then(|_| self.change(path, Some(&name))).map(|change| Pending {
                    outcome: outcome_name(change),
                    running: change
                        .old
                        .clone()
                        .map(|old| if secret { MASKED.to_string() } else { old }),
                    reason: change.reason.clone(),
                });
                ViewAttr {
                    // a `${}` reference names the property, which is not secret; its expanded value is masked above
                    value: if secret && !value.is_empty() && !value.contains("${") {
                        MASKED.to_string()
                    } else {
                        value
                    },
                    expanded,
                    source,
                    class: (attr_class != class).then_some(attr_class.name()),
                    reason: (attr_class != class)
                        .then(|| restart_reason(attr_class))
                        .filter(|text| !text.is_empty()),
                    pending,
                    name,
                }
            })
            .collect();
        let environment = property_name.and_then(|name| {
            let current = env::get_prop_or_env(&name);
            let by_conf = self.set_by_conf.get(&name).is_some_and(|value| *value == current);
            // only a value other than the conf's own: the environment overriding it
            let conf_value = env::expand(&node.attribute("value").unwrap_or_default()).unwrap_or_default();
            (!current.is_empty() && !by_conf && current != conf_value)
                .then(|| if property_secret { MASKED.to_string() } else { current })
        });
        let comments = self.comments_of(node);
        let child_units = unit.map_or(&[][..], |unit| unit.children);
        let children = if path.is_empty() {
            self.children(node, CATALOG, "")
        } else {
            self.children(node, child_units, path)
        };
        let children = children
            .into_iter()
            .map(|(child, child_unit, child_path)| {
                let child_class = match (child_unit, path.is_empty()) {
                    (Some(_), _) => class,
                    (None, true) => UNKNOWN,
                    (None, false) => class,
                };
                let child_key = child_unit.and_then(|unit| match unit.key {
                    Key::By(_) => child_path
                        .rsplit_once('[')
                        .map(|(_, key)| key.trim_end_matches(']').to_string()),
                    _ => None,
                });
                self.node(&child, child_unit, &child_path, child_class, child_key)
            })
            .collect();
        ViewNode {
            name: node.name(),
            key,
            path: path.to_string(),
            class: class.name(),
            reason: Some(restart_reason(class)).filter(|text| !text.is_empty()),
            sources,
            attrs,
            text: node
                .text()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
            comments: comments
                .map(|comments| comments.before.iter().chain(&comments.after).cloned().collect())
                .unwrap_or_default(),
            trailing: comments.map(|comments| comments.trailing.clone()).unwrap_or_default(),
            pending: own_change.map(|change| Pending {
                outcome: outcome_name(change),
                running: None,
                reason: change.reason.clone(),
            }),
            environment,
            children,
        }
    }

    /// The change of the report to the unit at `path` and `attr`; for the element itself (`None`), its addition,
    /// removal or content.
    fn change(&self, path: &str, attr: Option<&str>) -> Option<&ConfChange> {
        self.changes
            .iter()
            .find(|change| change.unit == path && change.attribute.as_deref() == attr && change.kind != "property")
    }

    #[allow(clippy::too_many_arguments)]
    fn lines(
        &self,
        node: &MNode,
        unit: Option<&'a Unit>,
        path: &str,
        inherited: Class,
        file: usize,
        out: &mut Vec<LineInfo>,
    ) {
        let class = unit.map_or(inherited, |unit| unit.class);
        let in_file: Vec<u32> = node
            .source()
            .into_iter()
            .chain(node.merge_sources())
            .filter_map(|source| self.at(&source))
            .filter(|at| at.file == file)
            .map(|at| at.line)
            .collect();
        if !in_file.is_empty() && !path.is_empty() {
            let attrs: Vec<(String, &'static str, String)> = unit
                .map(|unit| {
                    unit.attrs
                        .iter()
                        .filter(|(_, attr_class)| *attr_class != class)
                        .map(|(name, attr_class)| (name.to_string(), attr_class.name(), restart_reason(*attr_class)))
                        .collect()
                })
                .unwrap_or_default();
            let comments: Vec<String> = self
                .comments_of(node)
                .map(|comments| comments.before.iter().chain(&comments.after).cloned().collect())
                .unwrap_or_default();
            for line in in_file {
                out.push(LineInfo {
                    line,
                    name: node.name(),
                    path: path.to_string(),
                    unit: unit.is_some(),
                    class: class.name(),
                    reason: Some(restart_reason(class)).filter(|text| !text.is_empty()),
                    attrs: attrs.clone(),
                    comments: comments.clone(),
                });
            }
        }
        let units = if path.is_empty() {
            CATALOG
        } else {
            unit.map_or(&[][..], |unit| unit.children)
        };
        for (child, child_unit, child_path) in self.children(node, units, path) {
            let child_class = if child_unit.is_none() && path.is_empty() {
                UNKNOWN
            } else {
                class
            };
            self.lines(&child, child_unit, &child_path, child_class, file, out);
        }
    }
}

fn outcome_name(change: &ConfChange) -> &'static str {
    match change.outcome {
        super::Outcome::Applied => "applied",
        super::Outcome::WouldApply => "would-apply",
        super::Outcome::PendingRestart => "pending-restart",
        super::Outcome::NoEffect => "no-effect",
        super::Outcome::Failed => "failed",
    }
}

/// An attribute that holds a secret by its name: `password`, `jdbc-password`, `crypt-pass`, `api-key`,
/// `lfs-token-key`.
fn secret_attribute(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let secret_words = [
        "password",
        "pass",
        "secret",
        "api-key",
        "token-key",
        "state-key",
        "private-key",
    ];
    secret_words
        .iter()
        .any(|word| name == *word || name.ends_with(&format!("-{word}")))
        || name.contains("token-key")
}

/// Whether a `${}` in `value` names a secret property.
fn refers_to_secret(value: &str) -> bool {
    value.split("${").skip(1).any(|rest| {
        let body = rest.split('}').next().unwrap_or_default();
        body.split("?:")
            .map(|term| term.trim())
            .filter(|term| !term.starts_with('\'') && !term.is_empty())
            .any(|name| crate::context::factory::property_is_secret(name, None))
    })
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
