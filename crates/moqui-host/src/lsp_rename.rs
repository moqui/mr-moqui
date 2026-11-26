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

//! `textDocument/rename` of `moqui lsp` (plans/design-dev-aide.md 13.6): an entity field, a service, a form field or a
//! parameter, through the memory index's references, as edits across files.
//!
//! The target is the artifact the attribute value (or script string) under the cursor names, or defines: an attribute
//! that names nothing (a `from`) is no target. A rename edits each definition of the artifact and of each artifact that
//! takes its name from it ([`MemoryIndex::name_family`], by what each row was made from), each reference to one of
//! them, and the other places that write the name: a view alias's `field`, the `exclude`s that leave it out, a
//! form's `field-ref`s, a relationship's or view member's `key-map` that names it in its `field-name` or `related`,
//! and in data files a record's attribute of the field, or a value that is the service's name.
//!
//! It is refused, with what stops it:
//! - when one of them is read by another name (a prefixed `alias-all`, a REST path parameter, a form field posting to a
//!   parameter outside the rename);
//! - when a use depends on the name itself, where an edit would change what it means: an `econdition` or `field-map`
//!   with no `from` or `value` (it reads the context value of the field's name), a `key-map` with no `related` (the
//!   related field has the same name), a relationship with no `key-map` joining by the field as a primary key;
//! - when a reference of the name does not resolve, or its text is not the name as an attribute value or string literal;
//! - when a service's noun change would change the entity it uses: an `entity-auto` service, an `auto-parameters` with
//!   no `entity-name` (its own or an implemented service's), a noun that names an entity;
//! - by the safety scan, which decides: the name as a whole token in any file of the components the rename may reach
//!   (the target's own, those that depend on it, those that refer to its entity, service or form), an expression's
//!   or a script's use included, anywhere but an edit of the rename or a place the index proves names another
//!   artifact ([`safety_scan`]): each place is listed;
//! - when an edit would land in an upstream component while the upstream lock applies. The model-only boot knows the
//!   split from the conf (`dev-aide@site-component`, `dev-aide@allow-upstream-edits`), not from the database's
//!   settings: with no site component in the conf, a rename that would edit more than one component is refused, as it
//!   cannot tell which is upstream.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::index::extract::{is_expanded, parse_parameter_id, ToKind, ORIGIN_DERIVED};
use moqui_devkit::index::resolve::{ExcludeOf, NameDependents};
use moqui_devkit::index::{self, ArtifactRow, MemoryIndex};

use super::edit::{edit_of, line_offset, Edit};

/// What a rename is of: the artifact, and its name as the files write it (a service's full name, else the short one).
#[derive(Clone, Debug)]
pub(crate) struct RenameTarget {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) name: String,
}

/// The rename target of `artifact`, or why it cannot be renamed.
pub(crate) fn rename_target(artifact: &ArtifactRow) -> Result<RenameTarget, String> {
    if !matches!(artifact.kind.as_str(), "field" | "service" | "form-field" | "parameter") {
        return Err(format!(
            "moqui lsp renames an entity field, a service, a form field or a parameter; this is the {} {}",
            artifact.kind, artifact.name
        ));
    }
    if artifact.origin == ORIGIN_DERIVED {
        return Err(format!(
            "{} is derived from its entity: rename the entity or its field",
            artifact.id
        ));
    }
    if is_expanded(&artifact.origin) {
        return Err(format!(
            "{} comes from the {} of {}: rename what it comes from",
            artifact.id,
            artifact.origin,
            artifact.parent.as_deref().unwrap_or_default()
        ));
    }
    let name = match artifact.kind.as_str() {
        "service" => {
            if artifact.summary.starts_with("includes ") {
                return Err(format!(
                    "{} is a service-include: rename the service it includes",
                    artifact.id
                ));
            }
            artifact.name.clone()
        }
        "parameter" => {
            let Some((_, _, name)) = parse_parameter_id(&artifact.id) else {
                return Err(format!("{} is no parameter id", artifact.id));
            };
            if name.contains('.') {
                return Err(format!("{} is a nested parameter: rename it by hand", artifact.id));
            }
            name.to_string()
        }
        "form-field" => artifact.id.rsplit_once('@').map_or("", |(_, name)| name).to_string(),
        _ => artifact.short_name.clone(),
    };
    Ok(RenameTarget {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        name,
    })
}

/// Where the name under the cursor is: its 0-based line and UTF-16 start and end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

/// The attributes a definition writes its name in: a field's, parameter's or form field's `name`, a service's `verb`
/// and `noun`.
const DEFINING: [&str; 3] = ["name", "verb", "noun"];

/// The target of a rename at the 0-based `line` and UTF-16 `character` of the file `path` whose text is `text`: the
/// artifact the attribute value under the cursor names (by the reference rules) or defines, else the one a script's
/// string literal under the cursor names; with the span of that value.
pub(crate) fn target_at(
    index: &MemoryIndex,
    path: &str,
    text: &str,
    line: u32,
    character: u32,
) -> Result<(RenameTarget, Span), String> {
    let offset = offset_of(text, line, character).ok_or("the position is past the text")?;
    let tag = text[..offset]
        .rfind('<')
        .filter(|open| !text[*open..offset].contains('>'))
        .and_then(|open| start_tag_at(text, open));
    if let Some(tag) = tag {
        let Some(attribute) = tag
            .attributes
            .iter()
            .find(|attribute| attribute.value.start <= offset && offset <= attribute.value.end)
        else {
            return Err(format!("the cursor is in the {} tag, not in a name", tag.element));
        };
        let value = &text[attribute.value.clone()];
        let tag_line = line_number(text, tag.start);
        // the element's column, in characters, as the index places its references
        let line_start = text[..tag.start].rfind('\n').map_or(0, |at| at + 1);
        let tag_column = u32::try_from(text[line_start..tag.start].chars().count() + 1).unwrap_or(0);
        let span = span_of(text, attribute.value.clone());
        if moqui_devkit::refs::rule_for(&tag.element, &attribute.name).is_some() {
            let reference = index
                .refs
                .iter()
                .find(|(file, row)| {
                    file == path && row.line == tag_line && row.column == tag_column && row.to_name == value
                })
                .map(|(_, row)| row);
            return match reference {
                Some(row) => match row.to_id.as_deref().and_then(|id| index.describe(id)) {
                    Some(artifact) => Ok((rename_target(&artifact)?, span)),
                    None => Err(format!(
                        "{} {} does not resolve: there is nothing known to rename",
                        row.to_kind.as_str(),
                        row.to_name
                    )),
                },
                None => Err(format!(
                    "the {} of {} is not a reference the index read here",
                    attribute.name, tag.element
                )),
            };
        }
        if DEFINING.contains(&attribute.name.as_str()) {
            let defined = index.artifacts.iter().find(|artifact| {
                artifact.path == path
                    && artifact.line == tag_line
                    && artifact.origin.is_empty()
                    && (artifact.short_name == value
                        || (artifact.kind == "service" && attribute.name != "name")
                        || artifact.id.ends_with(&format!("@{value}"))
                        || artifact.id.ends_with(&format!(".{value}")))
            });
            if let Some(artifact) = defined {
                return Ok((rename_target(artifact)?, span));
            }
        }
        return Err(format!(
            "the {} of {} names no artifact the index knows",
            attribute.name, tag.element
        ));
    }
    // a script's string literal
    let line_start = line_offset(text, line + 1).ok_or("no such line")?;
    let line_text = text[line_start..].split('\n').next().unwrap_or_default();
    let at = offset - line_start;
    for quote in ['"', '\''] {
        let Some(open) = line_text[..at].rfind(quote) else {
            continue;
        };
        let Some(close) = line_text[at..].find(quote).map(|close| at + close) else {
            continue;
        };
        let literal = &line_text[open + 1..close];
        // the literal's column, in characters, where the index knows it (a script file's)
        let column = u32::try_from(line_text[..open + 1].chars().count() + 1).unwrap_or(0);
        let reference = index
            .refs
            .iter()
            .find(|(file, row)| {
                file == path
                    && row.line == line + 1
                    && (row.column == 0 || row.column == column)
                    && row.to_name == literal
            })
            .map(|(_, row)| row);
        if let Some(artifact) = reference
            .and_then(|row| row.to_id.as_deref())
            .and_then(|id| index.describe(id))
        {
            let span = span_of(text, line_start + open + 1..line_start + close);
            return Ok((rename_target(&artifact)?, span));
        }
    }
    Err("nothing the index knows is named here".to_string())
}

/// The byte offset of the 0-based `line` and UTF-16 `character` of `text`.
fn offset_of(text: &str, line: u32, character: u32) -> Option<usize> {
    let start = line_offset(text, line + 1)?;
    let mut units = 0u32;
    for (at, ch) in text[start..].char_indices() {
        if units >= character || ch == '\n' {
            return Some(start + at);
        }
        units += u32::try_from(ch.len_utf16()).unwrap_or(1);
    }
    Some(text.len())
}

/// The 1-based line of the byte `offset` of `text`.
fn line_number(text: &str, offset: usize) -> u32 {
    u32::try_from(text[..offset].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

/// The span of the bytes `range` of `text`, on one line.
fn span_of(text: &str, range: Range<usize>) -> Span {
    let edit = edit_of(text, range, "");
    Span {
        line: edit.line,
        start: edit.start,
        end: edit.end,
    }
}

// ---- tags ----

/// An attribute of a start tag: its name, and the byte ranges of its name and its value.
#[derive(Clone, Debug)]
pub(crate) struct Attribute {
    name: String,
    name_range: Range<usize>,
    value: Range<usize>,
}

/// A start tag in a text: its element's name, its attributes, where it starts and ends (past its `>`), and whether it
/// closes itself.
#[derive(Clone, Debug)]
pub(crate) struct StartTag {
    element: String,
    attributes: Vec<Attribute>,
    start: usize,
    end: usize,
    closed: bool,
}

impl StartTag {
    fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|attribute| attribute.name == name)
    }

    fn value<'t>(&self, text: &'t str, name: &str) -> Option<&'t str> {
        self.attribute(name).map(|attribute| &text[attribute.value.clone()])
    }
}

/// The start tag that begins at the byte `start` of `text` (its `<`).
fn start_tag_at(text: &str, start: usize) -> Option<StartTag> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut at = start + 1;
    let name_end = text[at..].find(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')? + at;
    let element = text[at..name_end].to_string();
    if element.is_empty() || element.starts_with(['!', '?', '/']) {
        return None;
    }
    at = name_end;
    let mut attributes = Vec::new();
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match bytes.get(at) {
            None => return None,
            Some(b'>') => {
                return Some(StartTag {
                    element,
                    attributes,
                    start,
                    end: at + 1,
                    closed: false,
                })
            }
            Some(b'/') if bytes.get(at + 1) == Some(&b'>') => {
                return Some(StartTag {
                    element,
                    attributes,
                    start,
                    end: at + 2,
                    closed: true,
                })
            }
            _ => {}
        }
        let name_end = text[at..].find(|ch: char| ch == '=' || ch.is_whitespace() || ch == '>' || ch == '/')? + at;
        if name_end == at {
            return None;
        }
        let name_range = at..name_end;
        at = name_end;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if bytes.get(at) != Some(&b'=') {
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let quote = *bytes.get(at)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_start = at + 1;
        let value_end = text[value_start..].find(char::from(quote))? + value_start;
        attributes.push(Attribute {
            name: text[name_range.clone()].to_string(),
            name_range,
            value: value_start..value_end,
        });
        at = value_end + 1;
    }
}

/// The start tag at the 1-based `line` and `column` (in characters) of `text`, or the first one on the line when the
/// column is not at one.
fn start_tag(text: &str, line: u32, column: u32) -> Option<StartTag> {
    let line_start = line_offset(text, line)?;
    let line_text = text[line_start..].split('\n').next().unwrap_or_default();
    let at_column = line_text
        .char_indices()
        .nth(column.saturating_sub(1) as usize)
        .map_or(line_text.len(), |(at, _)| at);
    let from = if line_text[at_column..].starts_with('<') {
        line_start + at_column
    } else {
        line_start + line_text.find('<')?
    };
    start_tag_at(text, from)
}

/// The start tags in the bytes `range` of `text`, in order, past comments, CDATA and processing instructions.
fn tags_in(text: &str, range: Range<usize>) -> Vec<StartTag> {
    let mut tags = Vec::new();
    let mut at = range.start;
    while at < range.end {
        let Some(open) = text[at..range.end].find('<').map(|found| at + found) else {
            break;
        };
        let rest = &text[open..];
        let skip_to = |end: &str| rest.find(end).map_or(text.len(), |found| open + found + end.len());
        if rest.starts_with("<!--") {
            at = skip_to("-->");
        } else if rest.starts_with("<![CDATA[") {
            at = skip_to("]]>");
        } else if let Some(tag) = start_tag_at(text, open) {
            at = tag.end;
            tags.push(tag);
        } else {
            at = open + 1;
        }
    }
    tags
}

/// The byte range of the element whose start tag begins at `start`, to the end of its end tag.
fn element_extent(text: &str, start: usize) -> Option<Range<usize>> {
    let tag = start_tag_at(text, start)?;
    if tag.closed {
        return Some(start..tag.end);
    }
    let open = format!("<{}", tag.element);
    let close = format!("</{}", tag.element);
    let mut depth = 1;
    let mut at = tag.end;
    while let Some(found) = text[at..].find('<').map(|found| at + found) {
        let rest = &text[found..];
        if rest.starts_with("<!--") {
            at = rest.find("-->").map_or(text.len(), |end| found + end + 3);
        } else if rest.starts_with("<![CDATA[") {
            at = rest.find("]]>").map_or(text.len(), |end| found + end + 3);
        } else if rest.starts_with(&close) {
            depth -= 1;
            let end = rest.find('>').map_or(text.len(), |end| found + end + 1);
            if depth == 0 {
                return Some(start..end);
            }
            at = end;
        } else if rest.starts_with(&open) {
            match start_tag_at(text, found) {
                Some(inner) if inner.element == tag.element => {
                    if !inner.closed {
                        depth += 1;
                    }
                    at = inner.end;
                }
                _ => at = found + 1,
            }
        } else {
            at = found + 1;
        }
    }
    None
}

// ---- the rename ----

/// Whether `name` is a name of a field, parameter or form field: a letter or `_`, then letters, digits and `_`.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// A service's name split as its file writes it: the prefix (its file's path), the verb and the noun.
fn service_parts(name: &str) -> Option<(&str, &str, Option<&str>)> {
    let (head, noun) = match name.split_once('#') {
        Some((head, noun)) => (head, Some(noun)),
        None => (name, None),
    };
    let (prefix, verb) = head.rsplit_once('.')?;
    Some((prefix, verb, noun))
}

/// The service's new full name for `new_name` (its verb and noun, or its full name with the same prefix).
fn new_service_name(old: &str, new_name: &str) -> Result<String, String> {
    let Some((prefix, _, _)) = service_parts(old) else {
        return Err(format!("{old} has no file prefix to keep"));
    };
    let full = if new_name.contains('.') && service_parts(new_name).is_some_and(|(new_prefix, ..)| new_prefix != prefix)
    {
        return Err(format!(
            "a service's prefix is its file's path ({prefix}): rename its verb and noun, or move it by hand"
        ));
    } else if new_name.starts_with(&format!("{prefix}.")) {
        new_name.to_string()
    } else {
        format!("{prefix}.{new_name}")
    };
    let Some((_, verb, noun)) = service_parts(&full) else {
        return Err(format!("{new_name} is no service name"));
    };
    if !is_name(verb) || noun.is_some_and(|noun| !is_name(noun)) {
        return Err(format!("{new_name} is no service name (verb#Noun)"));
    }
    Ok(full)
}

/// `id` with its last name `old` made `new`.
fn renamed_id(id: &str, old: &str, new: &str) -> String {
    match id.strip_suffix(old) {
        Some(head) => format!("{head}{new}"),
        None => id.to_string(),
    }
}

/// The edits a rename is making, by file, with the texts read and the places other artifacts of the name hold.
struct Plan<'a> {
    text_of: &'a dyn Fn(&Path) -> Option<String>,
    texts: BTreeMap<PathBuf, Option<String>>,
    edits: BTreeMap<PathBuf, Vec<(Range<usize>, String)>>,
    /// What the safety scan passes over: the name of another artifact, at its definition or a reference to it.
    others: BTreeMap<PathBuf, Vec<Range<usize>>>,
}

impl Plan<'_> {
    fn text(&mut self, path: &Path) -> Result<String, String> {
        self.texts
            .entry(path.to_path_buf())
            .or_insert_with(|| (self.text_of)(path))
            .clone()
            .ok_or_else(|| format!("rename refused: {} is not read", path.display()))
    }

    fn edit(&mut self, path: &Path, range: Range<usize>, new: &str) {
        let list = self.edits.entry(path.to_path_buf()).or_default();
        if !list.iter().any(|(known, _)| *known == range) {
            list.push((range, new.to_string()));
        }
    }

    fn edited(&self, path: &Path, at: &Range<usize>) -> bool {
        let inside = |ranges: Option<&Vec<Range<usize>>>| {
            ranges.is_some_and(|ranges| {
                ranges
                    .iter()
                    .any(|range| range.start <= at.start && at.end <= range.end)
            })
        };
        let edits = self
            .edits
            .get(path)
            .map(|list| list.iter().map(|(range, _)| range.clone()).collect());
        inside(edits.as_ref()) || inside(self.others.get(path))
    }
}

/// The edits of renaming `target` to `new_name`, by file, or why the rename is refused. `text_of` gives a file's text
/// (an open buffer's, else the file's).
pub(crate) fn rename_edits(
    factory: &ExecutionContextFactory,
    index: &MemoryIndex,
    target: &RenameTarget,
    new_name: &str,
    text_of: &dyn Fn(&Path) -> Option<String>,
) -> Result<BTreeMap<PathBuf, Vec<Edit>>, String> {
    let new_name = new_name.trim();
    let is_service = target.kind == "service";
    let old = target.name.as_str();
    let new = if is_service {
        new_service_name(old, new_name)?
    } else if is_name(new_name) {
        new_name.to_string()
    } else {
        return Err(format!("{new_name} is no name: a letter, then letters, digits and _"));
    };
    if new == old {
        return Err("the new name is the name it has".to_string());
    }
    let family = if is_service {
        NameDependents {
            ids: vec![target.id.clone()],
            ..Default::default()
        }
    } else {
        index.name_family(&target.id)
    };
    if !family.blocking.is_empty() {
        return Err(format!("rename refused: {}", family.blocking.join("; ")));
    }
    let ids: HashSet<&str> = family.ids.iter().map(String::as_str).collect();
    for id in &family.ids {
        let new_id = renamed_id(id, old, &new);
        if new_id != *id && index.describe(&new_id).is_some() {
            return Err(format!("rename refused: {new_id} is there already"));
        }
    }
    refuse_unresolved(index, target, &family.ids)?;
    let mut plan = Plan {
        text_of,
        texts: BTreeMap::new(),
        edits: BTreeMap::new(),
        others: BTreeMap::new(),
    };
    definitions(index, target, &new, &ids, &mut plan)?;
    references(index, old, &new, &ids, is_service, &mut plan)?;
    name_edits(index, old, &new, &family, &mut plan)?;
    if !is_service {
        field_refs(index, old, &new, &family.ids, &mut plan)?;
        key_maps(index, old, &new, &family.ids, &mut plan)?;
    }
    data_files(index, target, old, &new, &family.ids, &mut plan)?;
    proven_others(index, old, &ids, &mut plan);
    safety_scan(factory, index, target, old, &mut plan)?;
    let mut edits: BTreeMap<PathBuf, Vec<Edit>> = BTreeMap::new();
    for (path, list) in &plan.edits {
        let text = plan.texts.get(path).cloned().flatten().unwrap_or_default();
        let mut converted: Vec<Edit> = list
            .iter()
            .map(|(range, new)| edit_of(&text, range.clone(), new))
            .collect();
        converted.sort_by_key(|edit| (edit.line, edit.start));
        converted.dedup();
        edits.insert(path.clone(), converted);
    }
    check_upstream(factory, index, edits.keys())?;
    Ok(edits)
}

/// Refuse when a reference of the name does not resolve and may be one of the family's (its scope is none, unknown, or
/// one of theirs).
fn refuse_unresolved(index: &MemoryIndex, target: &RenameTarget, ids: &[String]) -> Result<(), String> {
    let (kind, scopes) = match target.kind.as_str() {
        "field" => (ToKind::Field, family_scopes(ids, "field:")),
        "parameter" => (ToKind::Parameter, family_scopes(ids, "parameter:")),
        "service" => (ToKind::Service, HashSet::new()),
        _ => return Ok(()),
    };
    let unknown = |scope: &str| match kind {
        ToKind::Field => scope.is_empty() || index.resolver.entity(scope).is_none_or(|full| scopes.contains(&full)),
        ToKind::Parameter => {
            scope.is_empty() || !index.resolver.has(&format!("service:{scope}")) || scopes.contains(scope)
        }
        _ => true,
    };
    match index
        .refs
        .iter()
        .find(|(_, row)| !row.resolved && row.to_kind == kind && row.to_name == target.name && unknown(&row.to_scope))
    {
        Some((path, row)) => Err(format!(
            "rename refused: the reference at {path}:{} names {} and does not resolve, so it may be this one",
            row.line, target.name
        )),
        None => Ok(()),
    }
}

/// The entities of the fields (`field:`) or the services of the parameters (`parameter:service:`) among `ids`.
fn family_scopes(ids: &[String], prefix: &str) -> HashSet<String> {
    ids.iter()
        .filter_map(|id| {
            let rest = id.strip_prefix(prefix)?;
            if prefix == "field:" {
                rest.rsplit_once('.').map(|(entity, _)| entity.to_string())
            } else {
                let (owner, _, _) = parse_parameter_id(id)?;
                owner.strip_prefix("service:").map(str::to_string)
            }
        })
        .collect()
}

/// Each definition written in a file: its name, or a service's verb and noun (a noun change refused where it would
/// change the entity the service uses).
fn definitions(
    index: &MemoryIndex,
    target: &RenameTarget,
    new: &str,
    ids: &HashSet<&str>,
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    for artifact in index
        .artifacts
        .iter()
        .filter(|artifact| ids.contains(artifact.id.as_str()) && artifact.origin.is_empty())
    {
        let path = PathBuf::from(&artifact.path);
        let text = plan.text(&path)?;
        let Some(tag) = start_tag(&text, artifact.line, artifact.column) else {
            return Err(format!(
                "rename refused: no element at {}:{}",
                artifact.path, artifact.line
            ));
        };
        let at = format!("{}:{}", artifact.path, artifact.line);
        if target.kind == "service" {
            let (_, old_verb, old_noun) = service_parts(&target.name).unwrap_or_default();
            let (_, new_verb, new_noun) = service_parts(new).unwrap_or_default();
            if old_noun != new_noun {
                refuse_noun_change(index, artifact, &tag, &text, old_noun, new_noun)?;
            }
            for (attribute, old, new) in [("verb", Some(old_verb), Some(new_verb)), ("noun", old_noun, new_noun)] {
                match (old, new, tag.attribute(attribute)) {
                    (Some(old), Some(new), Some(found)) if text[found.value.clone()] == *old => {
                        if old != new {
                            plan.edit(&path, found.value.clone(), new);
                        }
                    }
                    (None, None, None) => {}
                    (Some(_), None, _) | (None, Some(_), _) => {
                        return Err("rename refused: adding or removing a service's noun is done by hand".to_string())
                    }
                    _ => {
                        return Err(format!(
                            "rename refused: the service at {at} is not written by its verb and noun"
                        ))
                    }
                }
            }
        } else {
            let Some(found) = tag
                .attribute("name")
                .filter(|found| text[found.value.clone()] == *target.name)
            else {
                return Err(format!(
                    "rename refused: the definition at {at} does not write the name {}",
                    target.name
                ));
            };
            plan.edit(&path, found.value.clone(), new);
        }
    }
    Ok(())
}

/// Refuse a change of the noun of the service `artifact` (its start tag `tag`) where it would change the entity the
/// service uses.
fn refuse_noun_change(
    index: &MemoryIndex,
    artifact: &ArtifactRow,
    tag: &StartTag,
    text: &str,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<(), String> {
    if tag.value(text, "type") == Some("entity-auto") {
        return Err(format!(
            "rename refused: {} is an entity-auto service, whose noun is its entity",
            artifact.name
        ));
    }
    if index.resolver.reads_noun(&artifact.id) {
        return Err(format!(
            "rename refused: {} has an auto-parameters with no entity-name (its own or an implemented service's), which reads its noun",
            artifact.name
        ));
    }
    for noun in [old, new].into_iter().flatten() {
        if let Some(entity) = index.resolver.entity(noun) {
            return Err(format!(
                "rename refused: the noun {noun} names the entity {entity}, which gives the service's parameters their entity"
            ));
        }
    }
    Ok(())
}

/// Each reference to one of them, by the name's text as an attribute value or a string literal; refused where the
/// reference's element reads the context value of the field's name (an `econdition` or `field-map` with no `from` or
/// `value`).
fn references(
    index: &MemoryIndex,
    old: &str,
    new: &str,
    ids: &HashSet<&str>,
    is_service: bool,
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    for (path, row) in &index.refs {
        if !row.to_id.as_deref().is_some_and(|to| ids.contains(to)) {
            continue;
        }
        let file = PathBuf::from(path);
        let text = plan.text(&file)?;
        let at = format!("{path}:{}", row.line);
        if matches!(row.origin.as_str(), "js" | "groovy") || row.column == 0 && row.origin != "xml" {
            let Some(range) = literal_at(&text, row.line, row.column, old) else {
                return Err(format!(
                    "rename refused: the reference at {at} does not write {old} as one string the rename can edit"
                ));
            };
            plan.edit(&file, range, new);
            continue;
        }
        let Some(tag) = start_tag(&text, row.line, row.column) else {
            return Err(format!("rename refused: no element at {at}"));
        };
        let Some(attribute) = reference_attribute(&tag, &text, old) else {
            return Err(format!(
                "rename refused: the reference at {at} does not write {old} as a value the rename can edit"
            ));
        };
        let implied_from = !is_service
            && attribute.name == "field-name"
            && matches!(tag.element.as_str(), "econdition" | "field-map")
            && ["from", "value", "to-field-name"]
                .iter()
                .all(|name| tag.attribute(name).is_none());
        if implied_from {
            return Err(format!(
                "rename refused: the {} at {at} has no from, so it reads the value named {old}: give it a from first",
                tag.element
            ));
        }
        plan.edit(&file, attribute.value.clone(), new);
    }
    Ok(())
}

/// The attribute of `tag` whose value is `old` and that names an artifact by the reference rules
/// (`moqui_devkit::refs`), else the only attribute whose value is `old`.
fn reference_attribute<'t>(tag: &'t StartTag, text: &str, old: &str) -> Option<&'t Attribute> {
    let matching: Vec<&Attribute> = tag
        .attributes
        .iter()
        .filter(|attribute| text[attribute.value.clone()] == *old)
        .collect();
    matching
        .iter()
        .find(|attribute| moqui_devkit::refs::rule_for(&tag.element, &attribute.name).is_some())
        .copied()
        .or_else(|| (matching.len() == 1).then(|| matching[0]))
}

/// The inner range of the string literal `"old"` or `'old'` on the 1-based `line` of `text` that starts at the 1-based
/// character `column` (the name's), or, with no column, the only one on the line.
fn literal_at(text: &str, line: u32, column: u32, old: &str) -> Option<Range<usize>> {
    let start = line_offset(text, line)?;
    let line_text = text[start..].split('\n').next().unwrap_or_default();
    let mut found: Vec<usize> = Vec::new();
    for quote in ['"', '\''] {
        let quoted = format!("{quote}{old}{quote}");
        let mut from = 0;
        while let Some(at) = line_text[from..].find(&quoted).map(|at| from + at) {
            found.push(at + 1);
            from = at + quoted.len();
        }
    }
    let chosen = if column > 0 {
        found
            .into_iter()
            .find(|at| u32::try_from(line_text[..*at].chars().count() + 1).ok() == Some(column))?
    } else if found.len() == 1 {
        found[0]
    } else {
        return None;
    };
    Some(start + chosen..start + chosen + old.len())
}

/// The family's other writings of the name ([`NameDependents::edits`]): a view alias's `field`, and the `exclude`s that
/// name it under the auto element that reads it.
fn name_edits(
    index: &MemoryIndex,
    old: &str,
    new: &str,
    family: &NameDependents,
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    for name_edit in &family.edits {
        let Some(artifact) = index.describe(&name_edit.artifact) else {
            return Err(format!("rename refused: {} is not in the index", name_edit.artifact));
        };
        let path = PathBuf::from(&artifact.path);
        let text = plan.text(&path)?;
        let at = format!("{}:{}", artifact.path, artifact.line);
        let tag =
            start_tag(&text, artifact.line, artifact.column).ok_or(format!("rename refused: no element at {at}"))?;
        let range = match &name_edit.exclude_of {
            None => tag
                .attribute(&name_edit.attribute)
                .filter(|found| text[found.value.clone()] == *old)
                .map(|found| found.value.clone()),
            Some(exclude_of) => exclude_range(&text, &tag, exclude_of, &name_edit.attribute, old),
        };
        let Some(range) = range else {
            return Err(format!(
                "rename refused: the {} naming {old} in {} at {at} is not found",
                name_edit.attribute, name_edit.artifact
            ));
        };
        plan.edit(&path, range, new);
    }
    Ok(())
}

/// The value range of the `exclude` of `attribute` `old` in the element of `owner`, under the auto element
/// `exclude_of` names.
fn exclude_range(
    text: &str,
    owner: &StartTag,
    exclude_of: &ExcludeOf,
    attribute: &str,
    old: &str,
) -> Option<Range<usize>> {
    let extent = element_extent(text, owner.start)?;
    let mut current: Option<StartTag> = None;
    for tag in tags_in(text, owner.end..extent.end) {
        if matches!(
            tag.element.as_str(),
            "auto-parameters" | "auto-fields-entity" | "auto-fields-service" | "alias-all"
        ) {
            let excludes_inside = !tag.closed;
            current = excludes_inside.then_some(tag);
            continue;
        }
        if tag.element != "exclude" {
            continue;
        }
        let Some(auto) = &current else {
            continue;
        };
        let keyed = auto.element == exclude_of.element
            && auto.value(text, &exclude_of.key_attribute) == exclude_of.key.as_deref();
        if keyed {
            if let Some(found) = tag
                .attribute(attribute)
                .filter(|found| text[found.value.clone()] == *old)
            {
                return Some(found.value.clone());
            }
        }
    }
    None
}

/// Each `field-ref` of a form of the family's form fields that names it (in a `field-layout` or a
/// `form-list-column`).
fn field_refs(index: &MemoryIndex, old: &str, new: &str, ids: &[String], plan: &mut Plan<'_>) -> Result<(), String> {
    for id in ids {
        let Some((form, _)) = id.strip_prefix("form-field:").and_then(|field| field.rsplit_once('@')) else {
            continue;
        };
        let Some(artifact) = index.describe(&format!("form:{form}")) else {
            continue;
        };
        let path = PathBuf::from(&artifact.path);
        let text = plan.text(&path)?;
        let Some(tag) = start_tag(&text, artifact.line, artifact.column) else {
            continue;
        };
        let Some(extent) = element_extent(&text, tag.start) else {
            continue;
        };
        for field_ref in tags_in(&text, tag.end..extent.end) {
            if field_ref.element != "field-ref" {
                continue;
            }
            if let Some(found) = field_ref
                .attribute("name")
                .filter(|found| text[found.value.clone()] == *old)
            {
                plan.edit(&path, found.value.clone(), new);
            }
        }
    }
    Ok(())
}

/// The primary key field names of `entity` as the index knows them.
fn pk_names(index: &MemoryIndex, entity: &str) -> Vec<String> {
    index
        .children(&format!("entity:{entity}"))
        .into_iter()
        .filter(|field| field.kind == "field" && field.summary.ends_with(index::extract::PK_MARK))
        .map(|field| field.short_name)
        .collect()
}

/// A `key-map` as written: its `field-name` and `related` with their value ranges.
struct WrittenMap {
    field: Option<(String, Range<usize>)>,
    related: Option<(String, Range<usize>)>,
}

impl WrittenMap {
    fn of(tag: &StartTag, text: &str) -> Self {
        let value = |name: &str| {
            tag.attribute(name)
                .map(|found| (text[found.value.clone()].to_string(), found.value.clone()))
        };
        Self {
            field: value("field-name"),
            related: value("related").filter(|(related, _)| !related.is_empty()),
        }
    }

    fn field_name(&self) -> &str {
        self.field.as_ref().map_or("", |(name, _)| name.as_str())
    }
}

/// The field pairs (own field, related field) of a relationship of `relationship_type` with the key-maps `maps` to
/// the entity `related`, as moqui-db's `expand_key_map` expands them: with no key-map a `one` relationship joins the
/// related primary key to own fields of the same names; a key-map with no `related` joins the field of its own name,
/// or the related single primary key for a `one` (one key-map, or a name the related entity has no field of).
fn expand_pairs(
    index: &MemoryIndex,
    relationship_type: &str,
    maps: &[WrittenMap],
    related: &str,
) -> Vec<(String, String)> {
    let pks = pk_names(index, related);
    let one = relationship_type.starts_with("one");
    let one_pk = match pks.as_slice() {
        [pk] if one => Some(pk.clone()),
        _ => None,
    };
    if maps.is_empty() {
        return if one {
            pks.into_iter().map(|pk| (pk.clone(), pk)).collect()
        } else {
            Vec::new()
        };
    }
    let has_field = |name: &str| index.describe(&format!("field:{related}.{name}")).is_some();
    maps.iter()
        .map(|map| {
            let field = map.field_name().to_string();
            let related_field = match &map.related {
                Some((related, _)) => {
                    let mut related = related.clone();
                    if maps.len() > 1 {
                        if let Some(pk) = one_pk.as_ref().filter(|_| !has_field(&related)) {
                            related = pk.clone();
                        }
                    }
                    related
                }
                None if maps.len() == 1 => one_pk.clone().unwrap_or_else(|| field.clone()),
                None => one_pk
                    .clone()
                    .filter(|_| !has_field(&field))
                    .unwrap_or_else(|| field.clone()),
            };
            (field, related_field)
        })
        .collect()
}

/// The relationships and view members that join by an entity field of the family. A relationship's pairs are those
/// moqui-db expands ([`expand_pairs`]); where the renamed field takes part, the key-map's attribute that writes it is
/// edited, and the rename is refused where its side is implied: by no key-map (the primary key), or by a key-map with
/// no `related` (its own name, or the related single primary key). An auto-reverse relationship moqui-db adds mirrors
/// the relationship it reverses, which is checked on its side; a view's `member-relationship` joins by the
/// relationship it names, checked so too.
fn key_maps(index: &MemoryIndex, old: &str, new: &str, ids: &[String], plan: &mut Plan<'_>) -> Result<(), String> {
    let entities: HashSet<String> = ids
        .iter()
        .filter_map(|id| {
            id.strip_prefix("field:")?
                .rsplit_once('.')
                .map(|(entity, _)| entity.to_string())
        })
        .collect();
    if entities.is_empty() {
        return Ok(());
    }
    for artifact in &index.artifacts {
        if artifact.kind == "view-entity" {
            view_key_maps(index, artifact, &entities, old, new, plan)?;
            continue;
        }
        if artifact.kind != "relationship" {
            continue;
        }
        let Some(own) = artifact
            .parent
            .as_deref()
            .and_then(|parent| parent.strip_prefix("entity:"))
        else {
            continue;
        };
        let head = artifact.summary.split(", short-alias ").next().unwrap_or_default();
        let (relationship_type, related) = head.split_once(' ').unwrap_or((head, ""));
        let related = index.resolver.entity(related).unwrap_or_else(|| related.to_string());
        let (own_renamed, related_renamed) = (entities.contains(own), entities.contains(&related));
        if !own_renamed && !related_renamed {
            continue;
        }
        let path = PathBuf::from(&artifact.path);
        let at = format!("{}:{}", artifact.path, artifact.line);
        let text = plan.text(&path)?;
        let Some(tag) = start_tag(&text, artifact.line, artifact.column) else {
            continue;
        };
        let extent = element_extent(&text, tag.start).unwrap_or(tag.start..tag.end);
        let maps: Vec<WrittenMap> = tags_in(&text, tag.end..extent.end)
            .iter()
            .filter(|map| map.element == "key-map")
            .map(|map| WrittenMap::of(map, &text))
            .collect();
        for (place, (field, related_field)) in expand_pairs(index, relationship_type, &maps, &related)
            .into_iter()
            .enumerate()
        {
            let written = maps.get(place);
            if own_renamed && field == old {
                match written.and_then(|map| map.field.as_ref()) {
                    Some((_, range))
                        if related_field != old
                            || related_renamed
                            || written.is_some_and(|map| map.related.is_some()) =>
                    {
                        plan.edit(&path, range.clone(), new);
                    }
                    Some(_) => {
                        return Err(format!(
                            "rename refused: the key-map at {at} has no related, so the related field is named {old} too: write its related first"
                        ))
                    }
                    None => {
                        return Err(format!(
                            "rename refused: the relationship at {at} has no key-map, so it joins by the primary key {old}: write its key-map first"
                        ))
                    }
                }
            } else if !own_renamed
                && written
                    .and_then(|map| map.field.as_ref())
                    .is_some_and(|(name, _)| name == old)
            {
                // the field of another entity of the name, known
                if let Some((_, range)) = written.and_then(|map| map.field.as_ref()) {
                    plan.others.entry(path.clone()).or_default().push(range.clone());
                }
            }
            if related_renamed && related_field == old {
                match written.and_then(|map| map.related.as_ref()) {
                    Some((_, range)) => plan.edit(&path, range.clone(), new),
                    // implied by its own field's name, which the rename edits too
                    None if own_renamed && field == old => {}
                    None if written.is_some() => {
                        return Err(format!(
                            "rename refused: the key-map at {at} has no related, so it joins to the field {old} by its own field's name or the primary key: write its related first"
                        ))
                    }
                    None => {
                        return Err(format!(
                            "rename refused: the relationship at {at} has no key-map, so it joins to the primary key {old}: write its key-map first"
                        ))
                    }
                }
            } else if !related_renamed {
                if let Some((_, range)) = written
                    .and_then(|map| map.related.as_ref())
                    .filter(|(name, _)| name == old)
                {
                    plan.others.entry(path.clone()).or_default().push(range.clone());
                }
            }
        }
    }
    Ok(())
}

/// [`key_maps`] of the `member-entity` key-maps of the view entity `view`: a key-map's `field-name` is a field of the
/// member it joins from (a `member-entity`'s or a `member-relationship`'s entity), its `related` one of its own
/// entity, by default the field-name, or its single primary key when it has no field of that name and one key-map, as
/// moqui-db joins them. Refused where the member it joins from is not known and the key-map names the field.
fn view_key_maps(
    index: &MemoryIndex,
    view: &ArtifactRow,
    entities: &HashSet<String>,
    old: &str,
    new: &str,
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    let path = PathBuf::from(&view.path);
    let text = plan.text(&path)?;
    let Some(tag) = start_tag(&text, view.line, view.column) else {
        return Ok(());
    };
    let Some(extent) = element_extent(&text, tag.start) else {
        return Ok(());
    };
    let members: HashMap<String, Option<String>> = index.resolver.view_member_entities(&view.id).into_iter().collect();
    let entity_of = |name: &str| index.resolver.entity(name).unwrap_or_else(|| name.to_string());
    let tags = tags_in(&text, tag.end..extent.end);
    let mut at_member = 0;
    while at_member < tags.len() {
        let member = &tags[at_member];
        at_member += 1;
        if member.element != "member-entity" {
            continue;
        }
        let own = entity_of(member.value(&text, "entity-name").unwrap_or_default());
        let from_alias = member.value(&text, "join-from-alias");
        let from = from_alias.map(|alias| members.get(alias).cloned().flatten());
        let member_extent = element_extent(&text, member.start).unwrap_or(member.start..member.end);
        let maps: Vec<WrittenMap> = tags
            .iter()
            .filter(|map| map.element == "key-map" && member_extent.contains(&map.start))
            .map(|map| WrittenMap::of(map, &text))
            .collect();
        let pks = pk_names(index, &own);
        let own_has = |name: &str| index.describe(&format!("field:{own}.{name}")).is_some();
        for map in &maps {
            let at = format!(
                "{}:{}",
                view.path,
                line_number(&text, map.field.as_ref().map_or(member.start, |(_, range)| range.start))
            );
            let field = map.field_name();
            let related = match &map.related {
                Some((related, _)) => related.clone(),
                None if pks.len() == 1 && maps.len() == 1 && !own_has(field) => pks[0].clone(),
                None => field.to_string(),
            };
            if field == old {
                match &from {
                    Some(None) => {
                        return Err(format!(
                            "rename refused: the key-map at {at} names {old} of a member the index cannot follow: check it first"
                        ))
                    }
                    Some(Some(from)) if entities.contains(from) => match (&map.field, &map.related) {
                        (Some((_, range)), Some(_)) => plan.edit(&path, range.clone(), new),
                        (Some((_, range)), None) if related != old || entities.contains(&own) => {
                            plan.edit(&path, range.clone(), new);
                        }
                        _ => {
                            return Err(format!(
                                "rename refused: the key-map at {at} has no related, so the member's field is named {old} too: write its related first"
                            ))
                        }
                    },
                    _ => {
                        if let Some((_, range)) = &map.field {
                            plan.others.entry(path.clone()).or_default().push(range.clone());
                        }
                    }
                }
            }
            if related == old {
                if entities.contains(&own) {
                    match &map.related {
                        Some((_, range)) => plan.edit(&path, range.clone(), new),
                        None => {
                            let from_renamed = from.clone().flatten().is_some_and(|from| entities.contains(&from));
                            if !(from_renamed && field == old) {
                                return Err(format!(
                                    "rename refused: the key-map at {at} has no related, so it joins to the field {old} by its field-name or the primary key: write its related first"
                                ));
                            }
                        }
                    }
                } else if let Some((_, range)) = &map.related {
                    plan.others.entry(path.clone()).or_default().push(range.clone());
                }
            }
        }
    }
    Ok(())
}

/// In data files: a record's attribute of a renamed entity field, and for a service an attribute whose value is its
/// name (a `ServiceJob`'s `serviceName`, an `ArtifactGroupMember`'s `artifactName`).
fn data_files(
    index: &MemoryIndex,
    target: &RenameTarget,
    old: &str,
    new: &str,
    ids: &[String],
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    if target.kind == "service" {
        let mut paths: Vec<&str> = index
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == "data")
            .map(|artifact| artifact.path.as_str())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        for path in paths {
            let file = PathBuf::from(path);
            let text = plan.text(&file)?;
            for tag in tags_in(&text, 0..text.len()) {
                for attribute in &tag.attributes {
                    if text[attribute.value.clone()] == *old {
                        plan.edit(&file, attribute.value.clone(), new);
                    }
                }
            }
        }
        return Ok(());
    }
    let entities: HashSet<&str> = ids
        .iter()
        .filter_map(|id| id.strip_prefix("field:")?.rsplit_once('.').map(|(entity, _)| entity))
        .collect();
    for record in index.artifacts.iter().filter(|artifact| artifact.kind == "record") {
        let Some((entity, _)) = record.name.split_once(':') else {
            continue;
        };
        if !entities.contains(entity) {
            continue;
        }
        let path = PathBuf::from(&record.path);
        let text = plan.text(&path)?;
        if let Some(tag) = start_tag(&text, record.line, record.column) {
            if let Some(found) = tag.attribute(old) {
                plan.edit(&path, found.name_range.clone(), new);
            }
        }
    }
    Ok(())
}

/// The places proven to name another artifact, which the safety scan passes over: the defining attribute of another
/// artifact of the name (its `name`), and a reference the index resolved to another artifact (the attribute its
/// reference rule names, or a script string at its column). Nothing is passed over for not being of the family.
fn proven_others(index: &MemoryIndex, old: &str, ids: &HashSet<&str>, plan: &mut Plan<'_>) {
    for artifact in &index.artifacts {
        let named = artifact.short_name == old || artifact.name == old;
        if !named || ids.contains(artifact.id.as_str()) || !artifact.origin.is_empty() {
            continue;
        }
        let path = PathBuf::from(&artifact.path);
        let Ok(text) = plan.text(&path) else {
            continue;
        };
        let defining = start_tag(&text, artifact.line, artifact.column).and_then(|tag| {
            tag.attribute("name")
                .filter(|found| text[found.value.clone()] == *old)
                .cloned()
        });
        if let Some(found) = defining {
            plan.others.entry(path).or_default().push(found.value);
        }
    }
    for (path, row) in &index.refs {
        let other = row.to_id.as_deref().is_some_and(|to| !ids.contains(to));
        if !other || row.to_name != old {
            continue;
        }
        let file = PathBuf::from(path);
        let Ok(text) = plan.text(&file) else {
            continue;
        };
        let range = if matches!(row.origin.as_str(), "js" | "groovy") || row.column == 0 && row.origin != "xml" {
            literal_at(&text, row.line, row.column, old)
        } else {
            start_tag(&text, row.line, row.column)
                .and_then(|tag| reference_attribute(&tag, &text, old).map(|found| found.value.clone()))
        };
        if let Some(range) = range {
            plan.others.entry(file).or_default().push(range);
        }
    }
}

/// The directories a safety scan does not walk, as the index does not.
const SKIPPED_DIRS: [&str; 6] = [".git", "target", "node_modules", "libs", "build", "tmp"];

/// The largest file the safety scan reads.
const MAX_SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// The components a rename's leftovers may be in: the target's own, every component that depends on it (in turn, by
/// `component.xml`), and every component with a reference to the target's owner (the service, entity or form), with
/// their directories when the factory knows them.
fn scan_components(
    factory: &ExecutionContextFactory,
    index: &MemoryIndex,
    target: &RenameTarget,
    plan: &Plan<'_>,
) -> BTreeMap<String, Option<PathBuf>> {
    let component_of = |path: &str| -> String {
        index::locate(factory, Path::new(path))
            .map(|located| located.component)
            .or_else(|| {
                index
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.path == path)
                    .map(|artifact| artifact.component.clone())
            })
            .unwrap_or_default()
    };
    let owner = match target.kind.as_str() {
        "field" => target
            .id
            .strip_prefix("field:")
            .and_then(|field| field.rsplit_once('.'))
            .map(|(entity, _)| format!("entity:{entity}")),
        "parameter" => parse_parameter_id(&target.id).map(|(owner, _, _)| owner.to_string()),
        "form-field" => target
            .id
            .strip_prefix("form-field:")
            .and_then(|field| field.rsplit_once('@'))
            .map(|(form, _)| format!("form:{form}")),
        _ => Some(target.id.clone()),
    };
    let mut names: HashSet<String> = index
        .artifacts
        .iter()
        .filter(|artifact| artifact.id == target.id || Some(&artifact.id) == owner.as_ref())
        .map(|artifact| artifact.component.clone())
        .collect();
    // with the components the edits are in: a component of no artifact row is still scanned by its files
    names.extend(plan.edits.keys().map(|path| component_of(&path.display().to_string())));
    let components = factory.components();
    let mut grew = true;
    while grew {
        grew = false;
        for component in components.iter() {
            if !names.contains(&component.name) && component.depends_on.iter().any(|on| names.contains(on)) {
                names.insert(component.name.clone());
                grew = true;
            }
        }
    }
    if let Some(owner) = &owner {
        for (path, row) in &index.refs {
            if row.to_id.as_deref() == Some(owner.as_str()) {
                names.insert(component_of(path));
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let dir = components
                .iter()
                .find(|component| component.name == name)
                .map(|component| component.location.clone());
            (name, dir)
        })
        .collect()
}

/// The files of `dir`, walked as the index walks a component (its skipped directories left out), sorted.
fn files_of(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| SKIPPED_DIRS.contains(&name));
            if !skipped {
                files_of(&path, into);
            }
        } else if entry.metadata().is_ok_and(|meta| meta.len() <= MAX_SCAN_BYTES) {
            into.push(path);
        }
    }
}

/// Whether `ch` continues an identifier: a token of the name ends where one does not.
fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// The byte ranges of `old` in `text` as a whole token: no identifier character before or after it (for a service's
/// name, no `.` or `#` before it either, which would make it part of a longer name). In an XML file the markup's own
/// names are left out (an element's name, an attribute's name), but for a data file (`entity-facade-xml`), whose
/// attribute names are the fields of its records.
fn tokens_of(text: &str, old: &str, xml: bool) -> Vec<Range<usize>> {
    let dotted = old.contains(['.', '#']);
    let data = text.contains("<entity-facade-xml") || text.contains("<seed-data");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(old).map(|at| from + at) {
        let end = at + old.len();
        let before = text[..at].chars().next_back();
        let after = text[end..].chars().next();
        let starts = before.is_none_or(|ch| !identifier_char(ch) && !(dotted && matches!(ch, '.' | '#')));
        let ends = after.is_none_or(|ch| !identifier_char(ch) && !(dotted && matches!(ch, '.' | '#')));
        from = end;
        if !starts || !ends {
            continue;
        }
        if xml && !dotted {
            let element_name = matches!(before, Some('<' | '/')) && in_markup(text, at);
            let attribute_name = text[end..].trim_start().starts_with('=') && in_markup(text, at);
            if element_name || (attribute_name && !data) {
                continue;
            }
        }
        found.push(at..end);
    }
    found
}

/// Whether the byte `at` of `text` is inside a tag's markup, outside an attribute value: a `<` before it with no `>`
/// between, and an even number of quotes since that `<`.
fn in_markup(text: &str, at: usize) -> bool {
    let Some(open) = text[..at].rfind('<') else {
        return false;
    };
    let inside = &text[open..at];
    !inside.contains('>')
        && inside.matches('"').count().is_multiple_of(2)
        && inside.matches('\'').count().is_multiple_of(2)
}

/// The decisive gate of a rename: the name as a whole token anywhere in a file of the components it may reach
/// ([`scan_components`]), artifact or not (an expression's `row.old`, `${old}`, a script, a `.qvue`, a data record's
/// attribute, JSON or CSV), refuses it, listing up to 20 places, unless the place is an edit of the rename or proven to
/// name another artifact ([`proven_others`]).
fn safety_scan(
    factory: &ExecutionContextFactory,
    index: &MemoryIndex,
    target: &RenameTarget,
    old: &str,
    plan: &mut Plan<'_>,
) -> Result<(), String> {
    let components = scan_components(factory, index, target, plan);
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in components.values().flatten() {
        files_of(dir, &mut files);
    }
    // the files the index has of those components, the open buffers among them, and the edited ones
    files.extend(
        index
            .artifacts
            .iter()
            .filter(|artifact| components.contains_key(&artifact.component) && artifact.path.starts_with('/'))
            .map(|artifact| PathBuf::from(&artifact.path)),
    );
    files.extend(plan.edits.keys().cloned());
    files.sort();
    files.dedup();
    let mut left: Vec<String> = Vec::new();
    for file in files {
        let Ok(text) = plan.text(&file) else {
            continue;
        };
        let xml = file.extension().is_some_and(|ext| ext == "xml");
        for token in tokens_of(&text, old, xml) {
            if !plan.edited(&file, &token) {
                left.push(format!("{}:{}", file.display(), line_number(&text, token.start)));
            }
        }
    }
    if left.is_empty() {
        return Ok(());
    }
    left.dedup();
    let count = left.len();
    let shown: Vec<&str> = left.iter().take(20).map(String::as_str).collect();
    let more = if count > shown.len() {
        format!(" and {} more", count - shown.len())
    } else {
        String::new()
    };
    Err(format!(
        "rename refused: {old} is written where the rename does not know it is this name (an expression, a script, \
         a use it does not edit): {}{more}; change or check those first",
        shown.join(", ")
    ))
}

/// Refuse edits in an upstream component while the upstream lock applies (see the module doc).
fn check_upstream<'a>(
    factory: &ExecutionContextFactory,
    index: &MemoryIndex,
    paths: impl Iterator<Item = &'a PathBuf>,
) -> Result<(), String> {
    if factory.conf_setting("dev-aide", "allow-upstream-edits") == "true" {
        return Ok(());
    }
    let sites: Vec<String> = factory
        .conf_setting("dev-aide", "site-component")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let mut components: Vec<String> = Vec::new();
    for path in paths {
        let component = index::locate(factory, path)
            .map(|located| located.component)
            .or_else(|| {
                let path = path.display().to_string();
                index
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.path == path)
                    .map(|artifact| artifact.component.clone())
            })
            .unwrap_or_default();
        if !sites.is_empty() && !sites.contains(&component) {
            return Err(format!(
                "rename refused: {} is in the upstream component {}, and upstream components are locked \
                 (dev-aide@site-component is {}; dev-aide@allow-upstream-edits)",
                path.display(),
                if component.is_empty() { "of none" } else { &component },
                sites.join(", ")
            ));
        }
        if !components.contains(&component) {
            components.push(component);
        }
    }
    if sites.is_empty() && components.len() > 1 {
        components.sort();
        return Err(format!(
            "rename refused: it would edit the components {}, and with no dev-aide@site-component in the conf moqui \
             lsp cannot tell which of them are upstream",
            components
                .iter()
                .map(|name| if name.is_empty() { "(none)" } else { name.as_str() })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}
