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

//! The "Extend in my-site" quick fix of `moqui lsp` (plans/design-dev-aide.md D17, 5.6, P9): an in-place edit of an
//! upstream component, while the upstream lock is on, rewritten as the site component's extension where that is
//! mechanical. The edit is the open text against the file at git's `HEAD`; the mechanical edits are
//!
//! - fields added to an `entity` or `extend-entity` of an entities file: an `extend-entity` in the site component's
//!   entity file, as the Dev AIDE's entity modeler writes it (the first of the site's `entity/*.xml` that names the
//!   entity, else `entity/<Site>Entities.xml`);
//! - fields added to a `form-single` or `form-list` of a screen (a list's with its `form-list-column` of its own, a
//!   form's with its `field-ref` in the `field-layout`): a form merge in the site's `screen-extend` of that screen,
//!   each field with `after=` the field before it, as the screen builder writes it (F16).
//!
//! Anything else in the change (an attribute, a removed element, a field placed first, a ref elsewhere in a layout)
//! makes the whole edit not mechanical, and only the `upstream.edit` diagnostic is given. The site component is the
//! first of the conf's `dev-aide@site-component` on the model-only boot, as the rename's upstream lock reads it; with
//! none there, or `dev-aide@allow-upstream-edits` on, there is nothing upstream and nothing is offered.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::index;
use moqui_devkit::{Problem, Severity};
use moqui_lib::xml_edit::{self, Locator, Op, OutlineNode, Position};

/// The rule of an edit to an upstream component while the lock is on (the dev-aide's `check` gives the same).
pub(crate) const UPSTREAM_RULE: &str = "upstream.edit";

/// A new site entities file, as the entity modeler starts one.
const ENTITIES_TEXT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entities xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/mr-entity-definition.xsd\">\n</entities>\n";
/// A new `screen-extend` file, as the screen builder starts one.
const EXTEND_TEXT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<screen-extend xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/mr-xml-screen.xsd\">\n</screen-extend>\n";

/// A file of an upstream component while the lock is on, with the site component its extensions go in.
pub(crate) struct Upstream {
    pub(crate) component: String,
    /// The default site component, the first of `dev-aide@site-component`.
    pub(crate) site: String,
    site_location: PathBuf,
    /// The file's `component://` location.
    location: String,
}

/// Where `path` is upstream under the lock, or None: the lock off, no site component in the conf (nothing can be told
/// upstream), the file in a site component or in none, or the site component not loaded (nowhere to extend in).
pub(crate) fn upstream_of(factory: &ExecutionContextFactory, path: &Path) -> Option<Upstream> {
    if factory.conf_setting("dev-aide", "allow-upstream-edits") == "true" {
        return None;
    }
    let sites: Vec<String> = factory
        .conf_setting("dev-aide", "site-component")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let site = sites.first()?.clone();
    let located = index::locate(factory, path)?;
    if sites.contains(&located.component) {
        return None;
    }
    let site_location = factory
        .components()
        .iter()
        .find(|component| component.name == site)
        .map(|component| component.location.clone())?;
    Some(Upstream {
        component: located.component,
        site,
        site_location,
        location: located.location,
    })
}

/// The text of `path` at git's `HEAD`, or None where it is in no repository or not committed.
pub(crate) fn committed_text(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off", "show", &format!("HEAD:./{name}")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Whether two texts are the same but for line endings, trailing white space and blank lines.
pub(crate) fn same_text(a: &str, b: &str) -> bool {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    lines(a) == lines(b)
}

/// An element the edit added, its 1-based lines and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Added {
    pub(crate) line: u32,
    pub(crate) end_line: u32,
    pub(crate) column: u32,
    /// `field note of up.Thing`, `field note of form ThingList`.
    pub(crate) what: String,
}

/// What an extension's fields go into in the site's file.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Container {
    /// The `extend-entity` of an entity (its short name and package).
    Entity { short: String, package: String },
    /// The form merge of a form (`form-single` or `form-list`) by its name.
    Form { kind: String, name: String },
}

/// The mechanical part of an upstream edit: what it added, and the fields (as their XML) each container of the site's
/// file gets.
pub(crate) struct Diff {
    pub(crate) added: Vec<Added>,
    /// By the site file's path, each container with its fields in order.
    inserts: BTreeMap<PathBuf, Vec<(Container, Vec<String>)>>,
}

/// The edit of `text` (the file at `path` of `upstream`) against its committed text `head` as extensions, when the
/// whole of it is mechanical (see the module doc).
pub(crate) fn diff(upstream: &Upstream, text: &str, head: &str) -> Option<Diff> {
    let new = xml_edit::outline(text).ok()?;
    let old = xml_edit::outline(head).ok()?;
    if new.name != old.name {
        return None;
    }
    let found = match new.name.as_str() {
        "entities" => entity_changes(&new, &old, text, upstream)?,
        "screen" => screen_changes(&new, &old, text, upstream)?,
        _ => return None,
    };
    if found.added.is_empty() {
        return None;
    }
    // the edit is exactly these: the text without them is the committed text
    let mut removals = found.removals;
    removals.sort();
    removals.dedup();
    removals.reverse();
    let ops: Vec<Op> = removals
        .into_iter()
        .map(|path| Op::Remove {
            at: Locator::Path(path),
        })
        .collect();
    let reverted = xml_edit::apply(text, &ops).ok()?;
    if !same_text(&reverted.text, head) {
        return None;
    }
    Some(Diff {
        added: found.added,
        inserts: found.inserts,
    })
}

/// What one kind of file's changes come to, before the removal check.
#[derive(Default)]
struct Changes {
    added: Vec<Added>,
    removals: Vec<Vec<usize>>,
    inserts: BTreeMap<PathBuf, Vec<(Container, Vec<String>)>>,
}

impl Changes {
    fn insert(&mut self, file: PathBuf, container: Container, fragment: String) {
        let groups = self.inserts.entry(file).or_default();
        match groups.iter_mut().find(|(other, _)| *other == container) {
            Some((_, fragments)) => fragments.push(fragment),
            None => groups.push((container, vec![fragment])),
        }
    }
}

/// An entity element's full name, short name and package.
fn entity_names(node: &OutlineNode) -> Option<(String, String, String)> {
    let name = node.attr("entity-name")?;
    Some(match name.rsplit_once('.') {
        Some((package, short)) => (name.to_string(), short.to_string(), package.to_string()),
        None => {
            let package = node.attr("package").unwrap_or_default();
            let full = if package.is_empty() {
                name.to_string()
            } else {
                format!("{package}.{name}")
            };
            (full, name.to_string(), package.to_string())
        }
    })
}

fn field_names(node: &OutlineNode) -> Vec<&str> {
    node.children
        .iter()
        .filter(|child| child.name == "field")
        .filter_map(|child| child.attr("name"))
        .collect()
}

/// Fields added to the entities and extend-entities of an entities file.
fn entity_changes(new: &OutlineNode, old: &OutlineNode, text: &str, upstream: &Upstream) -> Option<Changes> {
    let kinds = ["entity", "extend-entity"];
    let keyed = |root: &OutlineNode| -> Option<Vec<(String, String)>> {
        let mut keys = Vec::new();
        for node in root.children.iter().filter(|node| kinds.contains(&node.name.as_str())) {
            let key = (node.name.clone(), entity_names(node)?.0);
            if keys.contains(&key) {
                return None;
            }
            keys.push(key);
        }
        Some(keys)
    };
    // the elements one to one by kind and name, so a renamed or a new entity is no field added
    if keyed(new)? != keyed(old)? {
        return None;
    }
    let mut changes = Changes::default();
    for node in new.children.iter().filter(|node| kinds.contains(&node.name.as_str())) {
        let (full, short, package) = entity_names(node)?;
        let before = old
            .children
            .iter()
            .find(|other| other.name == node.name && entity_names(other).is_some_and(|names| names.0 == full))?;
        let had = field_names(before);
        for field in node.children.iter().filter(|child| child.name == "field") {
            let name = field.attr("name")?;
            if had.contains(&name) {
                continue;
            }
            changes
                .added
                .push(added_of(text, field, format!("field {name} of {full}")));
            changes.removals.push(field.path.clone());
            let file = entity_file(upstream, &short);
            changes.insert(
                file,
                Container::Entity {
                    short: short.clone(),
                    package: package.clone(),
                },
                element_text(text, field)?,
            );
        }
    }
    Some(changes)
}

/// The site's entities file for an extension of the entity `short`: the first of its `entity/*.xml` that names it,
/// else `entity/<Site>Entities.xml`, else the first, else that one made (the entity modeler's `site_file`).
fn entity_file(upstream: &Upstream, short: &str) -> PathBuf {
    let dir = upstream.site_location.join("entity");
    let default = format!("{}Entities.xml", class_of(&upstream.site));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.ends_with(".xml"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let needle = format!("entity-name=\"{short}\"");
    let chosen = names
        .iter()
        .find(|name| std::fs::read_to_string(dir.join(name.as_str())).is_ok_and(|text| text.contains(&needle)))
        .or_else(|| names.iter().find(|name| **name == default))
        .or_else(|| names.first())
        .cloned()
        .unwrap_or(default);
    dir.join(chosen)
}

/// `my-site` as `MySite`, as the Site template names its files.
fn class_of(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

const FORM_KINDS: [&str; 2] = ["form-single", "form-list"];

/// Each form under `node`, depth first.
fn forms_of<'a>(node: &'a OutlineNode, into: &mut Vec<&'a OutlineNode>) {
    for child in &node.children {
        if FORM_KINDS.contains(&child.name.as_str()) {
            into.push(child);
        }
        forms_of(child, into);
    }
}

/// Each `field-ref` under `node` naming `name`, with its parent.
fn refs_named<'a>(node: &'a OutlineNode, name: &str, into: &mut Vec<(&'a OutlineNode, usize)>) {
    for (at, child) in node.children.iter().enumerate() {
        if child.name == "field-ref" && child.attr("name") == Some(name) {
            into.push((node, at));
        }
        refs_named(child, name, into);
    }
}

/// Fields added to the forms of a screen, with their layout refs and list columns.
fn screen_changes(new: &OutlineNode, old: &OutlineNode, text: &str, upstream: &Upstream) -> Option<Changes> {
    let keyed = |root: &OutlineNode| -> Option<Vec<(String, String)>> {
        let mut forms = Vec::new();
        forms_of(root, &mut forms);
        let mut keys = Vec::new();
        for form in forms {
            let key = (form.name.clone(), form.attr("name")?.to_string());
            if keys.contains(&key) {
                return None;
            }
            keys.push(key);
        }
        Some(keys)
    };
    if keyed(new)? != keyed(old)? {
        return None;
    }
    let after_scheme = upstream
        .location
        .strip_prefix("component://")
        .unwrap_or(&upstream.location);
    let file = upstream.site_location.join("screen-extend").join(after_scheme);
    let mut new_forms = Vec::new();
    forms_of(new, &mut new_forms);
    let mut old_forms = Vec::new();
    forms_of(old, &mut old_forms);
    let mut changes = Changes::default();
    for form in new_forms {
        let form_name = form.attr("name")?;
        let before = old_forms
            .iter()
            .find(|other| other.name == form.name && other.attr("name") == Some(form_name))?;
        let had = field_names(before);
        let added_names: Vec<&str> = field_names(form)
            .into_iter()
            .filter(|name| !had.contains(name))
            .collect();
        if added_names.is_empty() {
            continue;
        }
        let layout = form.children.iter().find(|child| child.name == "field-layout");
        let had_layout = before.children.iter().any(|child| child.name == "field-layout");
        let had_columns = before.children.iter().any(|child| child.name == "form-list-column");
        let mut previous: Option<&str> = None;
        for field in form.children.iter().filter(|child| child.name == "field") {
            let name = field.attr("name")?;
            let after = previous.replace(name);
            if !added_names.contains(&name) {
                continue;
            }
            // first in the form: a merge's field cannot be placed there
            let after = after?;
            // its ref in the layout, right after the ref of the field before it
            let mut refs = Vec::new();
            if let Some(layout) = layout {
                refs_named(layout, name, &mut refs);
            }
            match refs.as_slice() {
                [] if had_layout => return None,
                [] => {}
                [(parent, at)] => {
                    let before_ref = at.checked_sub(1).and_then(|at| parent.children.get(at))?;
                    if before_ref.name != "field-ref" || before_ref.attr("name") != Some(after) {
                        return None;
                    }
                    changes.removals.push(parent.children[*at].path.clone());
                }
                _ => return None,
            }
            // its column, its own, right after the column of the field before it
            let columns: Vec<(usize, &OutlineNode)> = form
                .children
                .iter()
                .enumerate()
                .filter(|(_, child)| child.name == "form-list-column")
                .filter(|(_, child)| {
                    let mut found = Vec::new();
                    refs_named(child, name, &mut found);
                    !found.is_empty()
                })
                .collect();
            match columns.as_slice() {
                [] if had_columns => return None,
                [] => {}
                [(at, column)] => {
                    let alone = column.children.len() == 1
                        && column.children[0].name == "field-ref"
                        && column.children[0].attr("name") == Some(name);
                    let before_column = at.checked_sub(1).and_then(|at| form.children.get(at))?;
                    let mut anchor = Vec::new();
                    refs_named(before_column, after, &mut anchor);
                    if !alone || before_column.name != "form-list-column" || anchor.is_empty() {
                        return None;
                    }
                    changes.removals.push(column.path.clone());
                }
                _ => return None,
            }
            let what = if form.name == "form-list" { "column" } else { "field" };
            changes
                .added
                .push(added_of(text, field, format!("{what} {name} of form {form_name}")));
            changes.removals.push(field.path.clone());
            changes.insert(
                file.clone(),
                Container::Form {
                    kind: form.name.clone(),
                    name: form_name.to_string(),
                },
                with_after(&element_text(text, field)?, after)?,
            );
        }
    }
    Some(changes)
}

/// The byte offset of the UTF-16 offset `units` of `text`.
fn byte_of(text: &str, units: usize) -> usize {
    let mut count = 0;
    for (at, ch) in text.char_indices() {
        if count >= units {
            return at;
        }
        count += ch.len_utf16();
    }
    text.len()
}

fn added_of(text: &str, node: &OutlineNode, what: String) -> Added {
    let end = byte_of(text, node.to16);
    let end_line = text[..end].matches('\n').count() + 1;
    Added {
        line: u32::try_from(node.line).unwrap_or(u32::MAX),
        end_line: u32::try_from(end_line).unwrap_or(u32::MAX),
        column: u32::try_from(node.column).unwrap_or(u32::MAX),
        what,
    }
}

/// An element's text as written, its own line's indentation taken off its later lines, to insert elsewhere.
fn element_text(text: &str, node: &OutlineNode) -> Option<String> {
    let (from, to) = (byte_of(text, node.from16), byte_of(text, node.to16));
    let source = text.get(from..to)?;
    let line_start = text[..from].rfind('\n').map_or(0, |at| at + 1);
    let indent = &text[line_start..from];
    let indent = if indent.chars().all(|ch| ch == ' ' || ch == '\t') {
        indent
    } else {
        ""
    };
    let lines: Vec<&str> = source
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .enumerate()
        .map(|(at, line)| {
            if at == 0 {
                line
            } else {
                line.strip_prefix(indent).unwrap_or(line)
            }
        })
        .collect();
    Some(lines.join("\n"))
}

/// A field's XML with `after` set to `after`.
fn with_after(field: &str, after: &str) -> Option<String> {
    let applied = xml_edit::apply(
        field,
        &[Op::SetAttribute {
            at: Locator::Path(Vec::new()),
            name: "after".into(),
            value: after.into(),
        }],
    )
    .ok()?;
    Some(applied.text)
}

/// An attribute value written for XML.
fn attr_escaped(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// A file of the site component the extension writes: its text before (None: a new file) and after.
#[derive(Debug)]
pub(crate) struct SiteEdit {
    pub(crate) path: PathBuf,
    pub(crate) before: Option<String>,
    pub(crate) after: String,
}

/// The site component's files with `diff`'s extensions in them: each container found in the file (`extend-entity` of
/// the entity, the form merge of the form) or made at its end, the fields after its own. `text_of` reads a file as the
/// editor has it.
pub(crate) fn site_edits(diff: &Diff, text_of: &dyn Fn(&Path) -> Option<String>) -> Option<Vec<SiteEdit>> {
    let mut edits = Vec::new();
    for (path, groups) in &diff.inserts {
        let before = text_of(path);
        let is_screen = groups
            .iter()
            .any(|(container, _)| matches!(container, Container::Form { .. }));
        let mut text = before
            .clone()
            .unwrap_or_else(|| (if is_screen { EXTEND_TEXT } else { ENTITIES_TEXT }).to_string());
        for (container, fragments) in groups {
            let root = xml_edit::outline(&text).ok()?;
            let found = root.children.iter().find(|node| match container {
                // by its full name, or its short name with no package (as the entity modeler finds it)
                Container::Entity { short, package } => {
                    let unqualified = node.attr("package").is_none()
                        && node.attr("entity-name").is_some_and(|name| !name.contains('.'));
                    node.name == "extend-entity"
                        && entity_names(node).is_some_and(|(_, other_short, other_package)| {
                            other_short == *short && (other_package == *package || unqualified)
                        })
                }
                Container::Form { kind, name } => node.name == *kind && node.attr("name") == Some(name.as_str()),
            });
            let mut ops = Vec::new();
            let at = match found {
                Some(node) => node.path.clone(),
                None => {
                    let wrapper = match container {
                        Container::Entity { short, package } => format!(
                            "<extend-entity entity-name=\"{}\" package=\"{}\"/>",
                            attr_escaped(short),
                            attr_escaped(package)
                        ),
                        Container::Form { kind, name } => format!("<{kind} name=\"{}\"/>", attr_escaped(name)),
                    };
                    ops.push(Op::Insert {
                        at: Locator::Path(Vec::new()),
                        position: Position::Last,
                        xml: wrapper,
                    });
                    vec![root.children.len()]
                }
            };
            for fragment in fragments {
                ops.push(Op::Insert {
                    at: Locator::Path(at.clone()),
                    position: Position::Last,
                    xml: fragment.clone(),
                });
            }
            text = xml_edit::apply(&text, &ops).ok()?.text;
        }
        edits.push(SiteEdit {
            path: path.clone(),
            before,
            after: text,
        });
    }
    Some(edits)
}

/// The `upstream.edit` problems of `text`, the file at `path`, against its committed text: one at each element a
/// mechanical edit added, naming the quick fix, else one at the top of the file. None where the file is not upstream
/// under the lock, not committed, or unchanged.
pub(crate) fn problems(factory: &ExecutionContextFactory, path: &Path, text: &str) -> Vec<Problem> {
    let Some(upstream) = upstream_of(factory, path) else {
        return Vec::new();
    };
    let Some(head) = committed_text(path) else {
        return Vec::new();
    };
    if same_text(text, &head) {
        return Vec::new();
    }
    let problem = |line: u32, column: u32, message: String| Problem {
        path: path.display().to_string(),
        line,
        column,
        severity: Severity::Warning,
        rule: UPSTREAM_RULE.into(),
        message,
        skill: Some(format!("{}#Extending", moqui_devkit::check::SKILL_FILE)),
    };
    match diff(&upstream, text, &head) {
        Some(found) => found
            .added
            .iter()
            .map(|added| {
                problem(
                    added.line,
                    added.column,
                    format!(
                        "{} is upstream: the {} belongs in {} as an extension; the quick fix Extend in {} moves it",
                        upstream.component, added.what, upstream.site, upstream.site
                    ),
                )
            })
            .collect(),
        None => vec![problem(
            1,
            1,
            format!(
                "{} is upstream: an edit here is a fork every upgrade has to merge; an extension in {} is not",
                upstream.component, upstream.site
            ),
        )],
    }
}

/// The text of `text` with one edit making it `new`: the bytes between their common start and end, on character
/// boundaries.
pub(crate) fn changed_span(text: &str, new: &str) -> (std::ops::Range<usize>, String) {
    let mut prefix = text.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !text.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let most = (text.len() - prefix).min(new.len() - prefix);
    let mut suffix = text
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(most)
        .take_while(|(a, b)| a == b)
        .count();
    while !text.is_char_boundary(text.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (prefix..text.len() - suffix, new[prefix..new.len() - suffix].to_string())
}

/// The committed text as the file writes its line ends.
pub(crate) fn reverted(text: &str, head: &str) -> String {
    if text.contains("\r\n") && !head.contains("\r\n") {
        head.replace('\n', "\r\n")
    } else {
        head.to_string()
    }
}
