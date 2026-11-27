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

//! The text edits `moqui lsp` answers (plans/archive/design-dev-aide.md 13.6): the `textDocument/codeAction` quick fixes of
//! the check rules that know one (an unknown entity, service, field or in-parameter: the nearest known names), and the
//! positions and edits a rename (`lsp_rename.rs`) is made of.

use moqui_devkit::index::extract::parse_parameter_id;
use moqui_devkit::index::{ArtifactRow, MemoryIndex};

/// One edit of a file: the 0-based line and UTF-16 characters of the text it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) text: String,
}

/// The byte offset of the 1-based `line` of `text`.
pub(crate) fn line_offset(text: &str, line: u32) -> Option<usize> {
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
pub(crate) fn edit_of(text: &str, range: std::ops::Range<usize>, new: &str) -> Edit {
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
