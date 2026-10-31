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

//! Which artifact a reference names (plans/design-dev-aide.md 7.1). The artifacts' ids are kept in memory, a few MB for
//! the corpus, so resolving a reference never asks the database: an entity by its full or short name, a field through
//! its entity, a service or the entity of an entity-auto name, a screen by its location or its path under a `screen`
//! directory, a transition or anchor of a screen.

use std::collections::HashMap;

use super::extract::{ArtifactRow, RefRow, ToKind};

/// The ids of the artifacts known, with what resolving needs besides.
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// Each id with the number of files that define it (an entity defined twice, the later overriding).
    ids: HashMap<String, u32>,
    /// An entity's short name to the full names that have it.
    entities: HashMap<String, Vec<String>>,
    /// A screen's path under its component's `screen` directory to its location.
    screens: HashMap<String, String>,
}

impl Resolver {
    /// Know `artifact`.
    pub fn add(&mut self, artifact: &ArtifactRow) {
        *self.ids.entry(artifact.id.clone()).or_insert(0) += 1;
        match artifact.kind.as_str() {
            "entity" | "view-entity" => {
                let full = self.entities.entry(artifact.short_name.clone()).or_default();
                if !full.contains(&artifact.name) {
                    full.push(artifact.name.clone());
                }
            }
            "screen" => {
                if let Some(tail) = screen_tail(&artifact.name) {
                    self.screens.insert(tail.to_string(), artifact.name.clone());
                }
            }
            _ => {}
        }
    }

    /// Know an artifact by its id, kind, name and short name alone, as the rows give them.
    pub fn add_parts(&mut self, id: &str, kind: &str, name: &str, short_name: &str) {
        self.add(&ArtifactRow {
            id: id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            short_name: short_name.to_string(),
            component: String::new(),
            path: String::new(),
            line: 0,
            column: 0,
            summary: String::new(),
            parent: None,
        });
    }

    /// Forget `artifact`, as its file was read again or removed.
    pub fn remove(&mut self, artifact: &ArtifactRow) {
        let gone = match self.ids.get_mut(&artifact.id) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                self.ids.remove(&artifact.id);
                true
            }
            None => false,
        };
        if !gone {
            return;
        }
        match artifact.kind.as_str() {
            "entity" | "view-entity" => {
                if let Some(full) = self.entities.get_mut(&artifact.short_name) {
                    full.retain(|name| *name != artifact.name);
                    if full.is_empty() {
                        self.entities.remove(&artifact.short_name);
                    }
                }
            }
            "screen" => {
                if let Some(tail) = screen_tail(&artifact.name) {
                    self.screens.remove(tail);
                }
            }
            _ => {}
        }
    }

    /// How many files define `id`.
    #[must_use]
    pub fn count(&self, id: &str) -> u32 {
        self.ids.get(id).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The full name of an entity named in full or short.
    #[must_use]
    pub fn entity(&self, name: &str) -> Option<String> {
        if self.has(&format!("entity:{name}")) {
            return Some(name.to_string());
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        self.entities.get(short).and_then(|full| full.first()).cloned()
    }

    /// The location of a screen named by its location, or by a location whose component differs but whose path under
    /// `screen/` is a screen's.
    #[must_use]
    pub fn screen(&self, location: &str) -> Option<String> {
        if self.has(&format!("screen:{location}")) {
            return Some(location.to_string());
        }
        // a screen-extend's path names the base by its path under any component's screen directory too
        let tail = screen_tail(location).or_else(|| location.strip_prefix("component://"))?;
        self.screens.get(tail).cloned()
    }

    /// Set `reference`'s target and whether it resolves. A location that is no indexed artifact resolves when
    /// `exists` says something is there.
    pub fn resolve(&self, reference: &mut RefRow, exists: &dyn Fn(&str) -> bool) {
        let target = match reference.to_kind {
            ToKind::Entity => self.entity(&reference.to_name).map(|full| format!("entity:{full}")),
            ToKind::Field => self
                .entity(&reference.to_scope)
                .map(|full| format!("field:{full}.{}", reference.to_name))
                .filter(|id| self.has(id)),
            ToKind::Service => {
                let id = format!("service:{}", reference.to_name);
                if self.has(&id) {
                    Some(id)
                } else {
                    entity_auto(&reference.to_name)
                        .and_then(|noun| self.entity(noun))
                        .map(|full| format!("entity:{full}"))
                }
            }
            ToKind::Screen => self
                .screen(&reference.to_name)
                .map(|location| format!("screen:{location}")),
            ToKind::Location => {
                let found = ["screen", "script", "data", "skill"]
                    .iter()
                    .map(|kind| format!("{kind}:{}", reference.to_name))
                    .find(|id| self.has(id));
                if found.is_none() && exists(&reference.to_name) {
                    reference.to_id = None;
                    reference.resolved = true;
                    return;
                }
                found
            }
            ToKind::Transition => {
                let id = format!("transition:{}#{}", reference.to_scope, reference.to_name);
                self.has(&id).then_some(id)
            }
            ToKind::Anchor => self.screen(&reference.to_scope).and_then(|base| {
                ["form", "anchor", "transition"]
                    .iter()
                    .map(|kind| format!("{kind}:{base}#{}", reference.to_name))
                    .find(|id| self.has(id))
                    .or_else(|| {
                        // Extension anchors identify a named XML element, independent of request dispatch.
                        // Keep them navigable after transitions acquire method-qualified IDs.
                        let prefix = format!("transition:{base}#{}:", reference.to_name);
                        let any = format!("{prefix}any");
                        if self.has(&any) {
                            Some(any)
                        } else {
                            self.ids.keys().filter(|id| id.starts_with(&prefix)).min().cloned()
                        }
                    })
            }),
            ToKind::Parameter => {
                let id =
                    super::extract::parameter_id(&format!("service:{}", reference.to_scope), "in", &reference.to_name);
                self.has(&id).then_some(id)
            }
        };
        reference.resolved = target.is_some();
        reference.to_id = target;
    }

    /// The names a reference to one of `ids` would carry, for finding the unresolved references an added artifact may
    /// resolve.
    #[must_use]
    pub fn names_of(artifacts: &[ArtifactRow]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for artifact in artifacts {
            match artifact.kind.as_str() {
                "entity" | "view-entity" => {
                    names.push(artifact.name.clone());
                    names.push(artifact.short_name.clone());
                }
                "field" => names.push(artifact.short_name.clone()),
                "parameter" => {
                    names.push(artifact.name.clone());
                    names.push(artifact.short_name.clone());
                }
                "service" => names.push(artifact.name.clone()),
                "screen" => names.push(artifact.name.clone()),
                "transition" => {
                    names.push(artifact.name.clone());
                    if let Some((_, target)) = artifact.id.rsplit_once('#') {
                        names.push(target.to_string());
                    }
                }
                "form" | "anchor" => names.push(artifact.name.clone()),
                "script" | "data" | "skill" => names.push(artifact.name.clone()),
                _ => {}
            }
        }
        names.sort();
        names.dedup();
        names
    }
}

/// The entity noun of an entity-auto service name (`create#moqui.basic.Enumeration`).
fn entity_auto(name: &str) -> Option<&str> {
    let (verb, noun) = name.split_once('#')?;
    matches!(verb, "create" | "update" | "store" | "delete").then_some(noun)
}

/// A screen location's path under its component's `screen` directory.
fn screen_tail(location: &str) -> Option<&str> {
    location
        .strip_prefix("component://")
        .and_then(|rest| rest.split_once("/screen/"))
        .map(|(_, tail)| tail)
}
