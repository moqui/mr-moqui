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

//! The artifact groups' members whose names are patterns (plans/design-dev-aide.md 7.1, `super::super::rows`): each
//! group's patterns as its marks keep them, compiled once, and each pattern member's reference made one for each
//! artifact it matches. A pattern is matched whole by `moqui_lib`'s Java-compatible matcher, as authorization matches
//! it (`moqui_core::user::authz`); one that does not compile matches nothing.

use moqui_lib::object::CompiledRegex;

use super::Resolver;
use crate::index::extract::{ArtifactRow, RefRow, ToKind, PATTERN_SCOPE};

impl Resolver {
    /// Know the patterns of the artifact group `artifact` (its marks, `pattern <kind> <pattern>`).
    pub(super) fn add_group(&mut self, artifact: &ArtifactRow) {
        let patterns: Vec<(ToKind, String, CompiledRegex)> = crate::index::extract::split_marks(&artifact.marks)
            .into_iter()
            .filter_map(|mark| {
                let (kind, pattern) = mark.strip_prefix("pattern ")?.split_once(' ')?;
                let kind = ToKind::parse(kind)?;
                match CompiledRegex::new(pattern) {
                    Ok(compiled) => Some((kind, pattern.to_string(), compiled)),
                    Err(err) => {
                        tracing::warn!(
                            "Artifact index: the pattern {pattern} of {} does not compile: {err}",
                            artifact.id
                        );
                        None
                    }
                }
            })
            .collect();
        if !patterns.is_empty() {
            self.patterns.insert(artifact.id.clone(), patterns);
        }
    }

    /// `refs` with each artifact group member's reference whose name is a pattern ([`PATTERN_SCOPE`], not yet matched)
    /// made one for each artifact of its kind whose name it matches whole (a REST resource, not a method of one), each
    /// resolved to it; one that matches none, or whose pattern does not compile, stays as it is, unresolved.
    #[must_use]
    pub fn expand_patterns(&self, refs: Vec<RefRow>) -> Vec<RefRow> {
        if !refs
            .iter()
            .any(|row| row.to_scope == PATTERN_SCOPE && row.to_id.is_none())
        {
            return refs;
        }
        let mut out = Vec::with_capacity(refs.len());
        for row in refs {
            if row.to_scope != PATTERN_SCOPE || row.to_id.is_some() {
                out.push(row);
                continue;
            }
            // the group's compiled pattern when the resolver knows the group, else compiled here
            let known = self.patterns.get(&row.from).and_then(|patterns| {
                patterns
                    .iter()
                    .find(|(kind, pattern, _)| *kind == row.to_kind && *pattern == row.to_name)
                    .map(|(_, _, compiled)| compiled.clone())
            });
            let pattern = known.or_else(|| CompiledRegex::new(&row.to_name).ok());
            let mut matched: Vec<&String> = match &pattern {
                Some(pattern) => self
                    .ids
                    .keys()
                    .filter(|id| pattern_name(row.to_kind, id).is_some_and(|name| pattern.is_full_match(name)))
                    .collect(),
                None => Vec::new(),
            };
            if matched.is_empty() {
                out.push(row);
                continue;
            }
            matched.sort();
            for id in matched {
                let mut one = row.clone();
                one.to_id = Some(id.clone());
                one.resolved = true;
                out.push(one);
            }
        }
        out
    }

    /// Whether a pattern of an artifact group matches the artifact `id` ([`Self::expand_patterns`]).
    #[must_use]
    pub fn pattern_matches(&self, id: &str) -> bool {
        self.patterns
            .values()
            .flatten()
            .any(|(kind, _, pattern)| pattern_name(*kind, id).is_some_and(|name| pattern.is_full_match(name)))
    }
}

/// The name an artifact group's pattern of `kind` is matched against in the artifact `id`, when the id is of that kind:
/// a screen's location, a service's or entity's name, a REST resource's path (not a method's).
fn pattern_name(kind: ToKind, id: &str) -> Option<&str> {
    let prefix = match kind {
        ToKind::Screen => "screen:",
        ToKind::Service => "service:",
        ToKind::Entity => "entity:",
        ToKind::Rest => "rest:",
        _ => return None,
    };
    let name = id.strip_prefix(prefix)?;
    (kind != ToKind::Rest || !name.contains(':')).then_some(name)
}
