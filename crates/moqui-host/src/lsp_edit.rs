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

//! The edits `moqui lsp` answers (plans/design-dev-aide.md 13.6): `textDocument/rename` of an entity field, a service,
//! a form field or a parameter through the memory index's references, as edits across files, and the
//! `textDocument/codeAction` quick fixes of the check rules that know one (an unknown entity, service, field or
//! in-parameter: the nearest known names).
//!
//! A rename edits each definition of the artifact, of each artifact that takes its name from it (an `auto-parameters`'
//! parameter, an `alias-all`'s alias, an `auto-fields` form field, a parameter a transition or REST method takes from its
//! service, a form field that posts to a parameter by its name, and a declared one of that name that merges with it,
//! [`MemoryIndex::name_family`]), and each reference to one of them. It is refused, with what stops it, when one of them
//! is read by another name (a view alias's `field`, a prefixed `alias-all`, a REST path parameter, a form field posting
//! to a parameter outside the rename), when a reference of the name does not resolve (the index cannot tell whether it
//! is this one), when a reference's text is not the name as an attribute value or string literal, and when an edit
//! would land in an upstream component while the upstream lock applies. The model-only boot knows the split from the
//! conf (`dev-aide@site-component`, `dev-aide@allow-upstream-edits`), not from the database's settings: with no site
//! component in the conf, a rename that would edit more than one component is refused, as it cannot tell which is
//! upstream.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::index::extract::{is_expanded, parse_parameter_id, ToKind, ORIGIN_DERIVED};
use moqui_devkit::index::{self, ArtifactRow, MemoryIndex};

/// One edit of a file: the 0-based line and UTF-16 characters of the text it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) text: String,
}

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
    let new_full = if is_service {
        new_service_name(&target.name, new_name)?
    } else if is_name(new_name) {
        new_name.to_string()
    } else {
        return Err(format!("{new_name} is no name: a letter, then letters, digits and _"));
    };
    if new_full == target.name {
        return Err("the new name is the name it has".to_string());
    }
    let family = if is_service {
        index::resolve::NameDependents {
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
    // a name the new one would collide with
    for id in &family.ids {
        let new_id = renamed_id(id, &target.name, &new_full);
        if new_id != *id && index.describe(&new_id).is_some() {
            return Err(format!("rename refused: {new_id} is there already"));
        }
    }
    // a reference of the name that does not resolve may be one of them
    let (kind, scopes) = match target.kind.as_str() {
        "field" => (Some(ToKind::Field), family_scopes(&family.ids, "field:")),
        "parameter" => (Some(ToKind::Parameter), family_scopes(&family.ids, "parameter:")),
        "service" => (Some(ToKind::Service), HashSet::new()),
        _ => (None, HashSet::new()),
    };
    if let Some(kind) = kind {
        let unknown = |scope: &str| match kind {
            ToKind::Field => scope.is_empty() || index.resolver.entity(scope).is_none_or(|full| scopes.contains(&full)),
            ToKind::Parameter => {
                scope.is_empty() || !index.resolver.has(&format!("service:{scope}")) || scopes.contains(scope)
            }
            _ => true,
        };
        if let Some((path, row)) = index.refs.iter().find(|(_, row)| {
            !row.resolved && row.to_kind == kind && row.to_name == target.name && unknown(&row.to_scope)
        }) {
            return Err(format!(
                "rename refused: the reference at {path}:{} names {} and does not resolve, so it may be this one",
                row.line, target.name
            ));
        }
    }
    let mut edits: BTreeMap<PathBuf, Vec<Edit>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    let mut text_at =
        |path: &Path| -> Option<String> { texts.entry(path.to_path_buf()).or_insert_with(|| text_of(path)).clone() };
    // each definition written in a file: its name, or a service's verb and noun
    for artifact in index
        .artifacts
        .iter()
        .filter(|artifact| ids.contains(artifact.id.as_str()) && artifact.origin.is_empty())
    {
        let path = PathBuf::from(&artifact.path);
        let Some(text) = text_at(&path) else {
            return Err(format!("rename refused: {} is not read", artifact.path));
        };
        let Some(tag) = start_tag(&text, artifact.line, artifact.column) else {
            return Err(format!(
                "rename refused: no element at {}:{}",
                artifact.path, artifact.line
            ));
        };
        let mut found = Vec::new();
        if is_service {
            let (_, old_verb, old_noun) = service_parts(&target.name).unwrap_or_default();
            let (_, new_verb, new_noun) = service_parts(&new_full).unwrap_or_default();
            found.extend(tag.replace("verb", old_verb, new_verb, &text));
            match (old_noun, new_noun) {
                (Some(old), Some(new)) => found.extend(tag.replace("noun", old, new, &text)),
                (None, None) => {}
                _ => return Err("rename refused: adding or removing a service's noun is done by hand".to_string()),
            }
            if found.len() != usize::from(old_noun.is_some()) + 1 {
                return Err(format!(
                    "rename refused: the service at {}:{} is not written by its verb and noun",
                    artifact.path, artifact.line
                ));
            }
            // the part that stays as it is is not edited
            let unchanged = [old_verb == new_verb, old_noun == new_noun];
            let mut at = 0;
            found.retain(|_| {
                at += 1;
                !unchanged[at - 1]
            });
        } else {
            found.extend(tag.replace("name", &target.name, &new_full, &text));
            if found.is_empty() {
                return Err(format!(
                    "rename refused: the definition at {}:{} does not write the name {}",
                    artifact.path, artifact.line, target.name
                ));
            }
        }
        edits.entry(path).or_default().extend(found);
    }
    // each reference to one of them, by the name's text as an attribute value or a string literal
    for (path, row) in &index.refs {
        if !row.to_id.as_deref().is_some_and(|to| ids.contains(to)) {
            continue;
        }
        let file = PathBuf::from(path);
        let Some(text) = text_at(&file) else {
            return Err(format!("rename refused: {path} is not read"));
        };
        let edit = if matches!(row.origin.as_str(), "js" | "groovy") || row.column == 0 {
            literal_on_line(&text, row.line, &target.name, &new_full)
        } else {
            start_tag(&text, row.line, row.column).and_then(|tag| tag.reference(&target.name, &new_full, &text))
        };
        let Some(edit) = edit else {
            return Err(format!(
                "rename refused: the reference at {path}:{} does not write {} as a value the rename can edit",
                row.line, target.name
            ));
        };
        edits.entry(file).or_default().push(edit);
    }
    for list in edits.values_mut() {
        list.sort_by_key(|edit| (edit.line, edit.start));
        list.dedup();
    }
    check_upstream(factory, edits.keys())?;
    Ok(edits)
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

/// Refuse edits in an upstream component while the upstream lock applies (see the module doc).
fn check_upstream<'a>(
    factory: &ExecutionContextFactory,
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

/// A start tag in a text: its element's name and each attribute with its value's byte range.
pub(crate) struct StartTag {
    element: String,
    attributes: Vec<(String, std::ops::Range<usize>)>,
}

/// The start tag at the 1-based `line` and `column` (in characters) of `text`, or the first one on the line when the
/// column is not at one.
pub(crate) fn start_tag(text: &str, line: u32, column: u32) -> Option<StartTag> {
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
    let bytes = text.as_bytes();
    let mut at = from + 1;
    let name_end = text[at..].find(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')? + at;
    let element = text[at..name_end].to_string();
    at = name_end;
    let mut attributes = Vec::new();
    loop {
        while at < bytes.len() && (bytes[at] as char).is_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] == b'>' || bytes[at] == b'/' {
            break;
        }
        let name_end = text[at..].find(|ch: char| ch == '=' || ch.is_whitespace() || ch == '>')? + at;
        let name = text[at..name_end].to_string();
        at = name_end;
        while at < bytes.len() && (bytes[at] as char).is_whitespace() {
            at += 1;
        }
        if bytes.get(at) != Some(&b'=') {
            continue;
        }
        at += 1;
        while at < bytes.len() && (bytes[at] as char).is_whitespace() {
            at += 1;
        }
        let quote = *bytes.get(at)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_start = at + 1;
        let value_end = text[value_start..].find(quote as char)? + value_start;
        attributes.push((name, value_start..value_end));
        at = value_end + 1;
    }
    Some(StartTag { element, attributes })
}

impl StartTag {
    /// The edit of the attribute `attribute` whose value is `old` to `new`.
    fn replace(&self, attribute: &str, old: &str, new: &str, text: &str) -> Option<Edit> {
        self.attributes
            .iter()
            .find(|(name, range)| name == attribute && text[range.clone()] == *old)
            .map(|(_, range)| edit_of(text, range.clone(), new))
    }

    /// The edit of the reference to `old` in this tag: the attribute whose value is `old` and that names an artifact
    /// by the reference rules (`moqui_devkit::refs`), else the only attribute whose value is `old`.
    fn reference(&self, old: &str, new: &str, text: &str) -> Option<Edit> {
        let matching: Vec<&(String, std::ops::Range<usize>)> = self
            .attributes
            .iter()
            .filter(|(_, range)| text[range.clone()] == *old)
            .collect();
        let chosen = matching
            .iter()
            .find(|(name, _)| moqui_devkit::refs::rule_for(&self.element, name).is_some())
            .or_else(|| (matching.len() == 1).then(|| &matching[0]))?;
        Some(edit_of(text, chosen.1.clone(), new))
    }
}

/// The edit of a string literal `"old"` or `'old'` on the 1-based `line` of `text`.
fn literal_on_line(text: &str, line: u32, old: &str, new: &str) -> Option<Edit> {
    let start = line_offset(text, line)?;
    let line_text = text[start..].split('\n').next().unwrap_or_default();
    for quote in ['"', '\''] {
        let quoted = format!("{quote}{old}{quote}");
        if let Some(at) = line_text.find(&quoted) {
            let from = start + at + 1;
            return Some(edit_of(text, from..from + old.len(), new));
        }
    }
    None
}

/// The byte offset of the 1-based `line` of `text`.
fn line_offset(text: &str, line: u32) -> Option<usize> {
    if line == 0 {
        return None;
    }
    let mut offset = 0;
    for _ in 1..line {
        offset += text[offset..].find('\n')? + 1;
    }
    Some(offset)
}

/// The edit replacing the bytes `range` of `text` with `new`, in 0-based lines and UTF-16 characters.
fn edit_of(text: &str, range: std::ops::Range<usize>, new: &str) -> Edit {
    let before = &text[..range.start];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let units = |part: &str| u32::try_from(part.encode_utf16().count()).unwrap_or(u32::MAX);
    let start = units(&text[line_start..range.start]);
    Edit {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        start,
        end: start + units(&text[range]),
        text: new.to_string(),
    }
}

// ---- quick fixes ----

/// The name a check problem's message says is unknown, with the rule's kind of name: `entity X`, `service X`,
/// `field X of E` (one field), `in-parameter X of S`. None for another rule.
pub(crate) fn unknown_name(rule: &str, message: &str) -> Option<UnknownName> {
    let detail = message.split(": ").next()?;
    let words: Vec<&str> = detail.split(' ').collect();
    Some(match (rule, words.as_slice()) {
        ("reference.entity", ["entity", name]) => UnknownName::Entity((*name).to_string()),
        ("reference.service", ["service", name]) => UnknownName::Service((*name).to_string()),
        ("reference.field", ["field", name, "of", entity]) => UnknownName::Field {
            name: (*name).to_string(),
            entity: (*entity).to_string(),
        },
        ("reference.parameter", ["in-parameter", name, "of", service]) => UnknownName::Parameter {
            name: (*name).to_string(),
            service: (*service).to_string(),
        },
        _ => return None,
    })
}

/// A name a check says is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UnknownName {
    Entity(String),
    Service(String),
    Field { name: String, entity: String },
    Parameter { name: String, service: String },
}

impl UnknownName {
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Entity(name) | Self::Service(name) => name,
            Self::Field { name, .. } | Self::Parameter { name, .. } => name,
        }
    }
}

/// The verbs of the entity-auto services.
const ENTITY_AUTO_VERBS: [&str; 4] = ["create", "update", "store", "delete"];

/// The known names nearest `unknown` (at most three, the nearest first), from `known` names of its kind: a name within
/// an edit distance of a third of its length (at least two). An entity named by its short name is offered short names,
/// and an entity-auto service name the entity-auto names of the nearest entities.
pub(crate) fn nearest(unknown: &UnknownName, index: &MemoryIndex) -> Vec<String> {
    let artifacts = |kinds: &[&str]| -> Vec<&ArtifactRow> {
        index
            .artifacts
            .iter()
            .filter(|artifact| kinds.contains(&artifact.kind.as_str()) && artifact.origin.is_empty())
            .collect()
    };
    let candidates: Vec<String> = match unknown {
        UnknownName::Entity(name) => entity_names(&artifacts(&["entity", "view-entity"]), !name.contains('.')),
        UnknownName::Service(name) => {
            let mut names: Vec<String> = artifacts(&["service"])
                .into_iter()
                .map(|artifact| artifact.name.clone())
                .collect();
            if let Some((verb, noun)) = name
                .split_once('#')
                .filter(|(verb, _)| ENTITY_AUTO_VERBS.contains(verb))
            {
                let entities = entity_names(&artifacts(&["entity"]), !noun.contains('.'));
                names.extend(entities.into_iter().map(|entity| format!("{verb}#{entity}")));
            }
            names
        }
        UnknownName::Field { entity, .. } => {
            let full = index.resolver.entity(entity).unwrap_or_else(|| entity.clone());
            index
                .children(&format!("entity:{full}"))
                .into_iter()
                .filter(|artifact| artifact.kind == "field")
                .map(|artifact| artifact.short_name)
                .collect()
        }
        UnknownName::Parameter { service, .. } => index
            .children(&format!("service:{service}"))
            .into_iter()
            .filter_map(|artifact| {
                let (_, direction, name) = parse_parameter_id(&artifact.id)?;
                (direction == "in" && !name.contains('.')).then(|| name.to_string())
            })
            .collect(),
    };
    rank_nearest(unknown.name(), candidates)
}

/// The names of `entities`, short or full.
fn entity_names(entities: &[&ArtifactRow], short: bool) -> Vec<String> {
    entities
        .iter()
        .map(|artifact| {
            if short {
                artifact.short_name.clone()
            } else {
                artifact.name.clone()
            }
        })
        .collect()
}

/// Of `candidates`, the three nearest `name` within the distance [`nearest`] allows, the nearest first.
pub(crate) fn rank_nearest(name: &str, mut candidates: Vec<String>) -> Vec<String> {
    candidates.sort();
    candidates.dedup();
    let limit = (name.chars().count() / 3).max(2);
    let mut ranked: Vec<(usize, String)> = candidates
        .into_iter()
        .filter(|candidate| candidate != name)
        .filter_map(|candidate| {
            let distance = edit_distance(&name.to_lowercase(), &candidate.to_lowercase());
            (distance <= limit).then_some((distance, candidate))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().take(3).map(|(_, candidate)| candidate).collect()
}

/// The Levenshtein distance between `a` and `b`, by characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

/// The edit putting `fix` in place of the value `bad` on the 0-based `line` of `text`, at or after the UTF-16
/// `character`: the first attribute value that is `bad` there.
pub(crate) fn value_edit(text: &str, line: u32, character: u32, bad: &str, fix: &str) -> Option<Edit> {
    let start = line_offset(text, line + 1)?;
    let line_text = text[start..].split('\n').next().unwrap_or_default();
    let mut units = 0u32;
    let mut from = line_text.len();
    for (at, ch) in line_text.char_indices() {
        if units >= character {
            from = at;
            break;
        }
        units += u32::try_from(ch.len_utf16()).unwrap_or(1);
    }
    for quote in ['"', '\''] {
        let quoted = format!("{quote}{bad}{quote}");
        let found = line_text[from..]
            .find(&quoted)
            .map(|at| from + at)
            .or_else(|| line_text.find(&quoted));
        if let Some(at) = found {
            let value = start + at + 1;
            return Some(edit_of(text, value..value + bad.len(), fix));
        }
    }
    None
}
