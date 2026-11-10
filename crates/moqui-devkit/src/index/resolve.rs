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
//!
//! A screen path (a `link`'s or a response's `url`, [`ToKind::ScreenPath`]) is walked as Moqui walks a screen URL,
//! from the screen the path is written in: `..` goes up to the screen's parent, `.` stays, and each other segment is a
//! subscreen of the screen reached (a `subscreens-item`, else the default subscreen of the screen's directory:
//! `Foo/Bar.xml` of `Foo.xml`) or, the last one, a transition of it. A path starting with `/` is walked from the webroot
//! screen. A path that cannot be walked stays unresolved.
//!
//! An entity-auto name (`create#moqui.example.Example`) that no file defines a service for gets a derived service
//! ([`Resolver::derive`], origin `derived`), with the in and out parameters the entity-auto runner takes and gives
//! (`moqui_core::service::call::run_entity`), so a call's `field-map`s resolve to its parameters as to a defined
//! service's.

use std::collections::HashMap;

use super::extract::{
    parameter_id, parameter_name, short_of, ArtifactRow, BindingRow, RefRow, ToKind, DERIVED_PATH, ORIGIN_DERIVED,
    PK_MARK,
};

/// A field of an entity, as the entity-auto services take it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoField {
    name: String,
    /// The field's type (`id`, `text-medium`), or what an alias names.
    kind: String,
    pk: bool,
}

/// The ids of the artifacts known, with what resolving needs besides.
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// Each id with the number of files that define it (an entity defined twice, the later overriding).
    ids: HashMap<String, u32>,
    /// An entity's short name to the full names that have it.
    entities: HashMap<String, Vec<String>>,
    /// A screen's path under its component's `screen` directory to its location.
    screens: HashMap<String, String>,
    /// An entity's full name to its fields, in the order they are defined (an `extend-entity`'s after).
    fields: HashMap<String, Vec<AutoField>>,
    /// The derived services known ([`Self::derive`]): each id with its rows (the service, then its parameters).
    derived: HashMap<String, Vec<ArtifactRow>>,
    /// A screen's location to its `subscreens-item`s: each item's name and the location it names.
    items: HashMap<String, Vec<(String, String)>>,
    /// A screen's location to the screens whose `subscreens-item`s name it.
    item_parents: HashMap<String, Vec<String>>,
}

/// The root screen an absolute screen path (`/apps/...`) is walked from.
const WEBROOT_SCREEN: &str = "component://webroot/screen/webroot.xml";

impl Resolver {
    /// Know `artifact`.
    pub fn add(&mut self, artifact: &ArtifactRow) {
        *self.ids.entry(artifact.id.clone()).or_insert(0) += 1;
        if artifact.origin == ORIGIN_DERIVED {
            let service = match artifact.kind.as_str() {
                "service" => Some(artifact.id.clone()),
                _ => artifact
                    .id
                    .strip_prefix("parameter:")
                    .and_then(|rest| rest.rsplit_once('@'))
                    .map(|(owner, _)| owner.to_string()),
            };
            if let Some(service) = service {
                let rows = self.derived.entry(service).or_default();
                if !rows.iter().any(|row| row.id == artifact.id) {
                    rows.push(artifact.clone());
                }
            }
            return;
        }
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
            "subscreen" => {
                if let Some((parent, location)) = item_of(artifact) {
                    self.items
                        .entry(parent.to_string())
                        .or_default()
                        .push((artifact.name.clone(), location.to_string()));
                    self.item_parents
                        .entry(location.to_string())
                        .or_default()
                        .push(parent.to_string());
                }
            }
            "field" => {
                if let Some(entity) = artifact.parent.as_deref().and_then(|id| id.strip_prefix("entity:")) {
                    let (kind, pk) = match artifact.summary.strip_suffix(PK_MARK) {
                        Some(kind) => (kind, true),
                        None => (artifact.summary.as_str(), false),
                    };
                    let fields = self.fields.entry(entity.to_string()).or_default();
                    if !fields.iter().any(|field| field.name == artifact.short_name) {
                        fields.push(AutoField {
                            name: artifact.short_name.clone(),
                            kind: kind.to_string(),
                            pk,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Know an artifact by its id, kind, name and short name alone, as the rows give them.
    #[cfg(test)]
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
            origin: String::new(),
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
        if artifact.origin == ORIGIN_DERIVED {
            // forgotten as derived even while a file defines the id too (the file's then answers)
            if artifact.kind == "service" {
                self.derived.remove(&artifact.id);
            }
            return;
        }
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
            "subscreen" => {
                if let Some((parent, location)) = item_of(artifact) {
                    if let Some(items) = self.items.get_mut(parent) {
                        if let Some(at) = items.iter().position(|(name, _)| *name == artifact.name) {
                            items.remove(at);
                        }
                        if items.is_empty() {
                            self.items.remove(parent);
                        }
                    }
                    if let Some(parents) = self.item_parents.get_mut(location) {
                        if let Some(at) = parents.iter().position(|known| known == parent) {
                            parents.remove(at);
                        }
                        if parents.is_empty() {
                            self.item_parents.remove(location);
                        }
                    }
                }
            }
            "field" => {
                if let Some(entity) = artifact.parent.as_deref().and_then(|id| id.strip_prefix("entity:")) {
                    if let Some(fields) = self.fields.get_mut(entity) {
                        fields.retain(|field| field.name != artifact.short_name);
                        if fields.is_empty() {
                            self.fields.remove(entity);
                        }
                    }
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
        if reference.to_kind == ToKind::TransitionAnchor {
            // the base screen's transition it replaces, else a transition it adds: resolved while the base is there
            let base = self.screen(&reference.to_scope);
            reference.to_id = base
                .as_deref()
                .and_then(|base| self.base_transition(base, &reference.to_name));
            reference.resolved = base.is_some();
            return;
        }
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
                    // the derived service of an entity-auto name, else its entity
                    self.entity_auto_service(&reference.to_name)
                        .filter(|id| self.has(id))
                        .or_else(|| {
                            entity_auto(&reference.to_name)
                                .and_then(|(_, noun)| self.entity(noun))
                                .map(|full| format!("entity:{full}"))
                        })
                }
            }
            // a default subscreen of a directory names its parent screen in the scope: an edge while both are there
            ToKind::Screen => self
                .screen(&reference.to_name)
                .filter(|_| reference.to_scope.is_empty() || self.screen(&reference.to_scope).is_some())
                .map(|location| format!("screen:{location}")),
            ToKind::ScreenPath => self.screen_path(&reference.to_scope, &reference.to_name),
            ToKind::Location => {
                // a widget-template-include names `<file>#<template>`
                let found = ["screen", "script", "data", "skill", "widget-template"]
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
                    .or_else(|| self.base_transition(&base, &reference.to_name))
            }),
            ToKind::TransitionAnchor => None,
            ToKind::Parameter => self.service_parameter(&reference.to_scope, "in", &reference.to_name),
        };
        reference.resolved = target.is_some();
        reference.to_id = target;
    }

    /// The parameter `name` of the service `service` in `direction`: a defined service's, or the derived service's
    /// of an entity-auto name, when known.
    fn service_parameter(&self, service: &str, direction: &str, name: &str) -> Option<String> {
        let id = parameter_id(&format!("service:{service}"), direction, name);
        if self.has(&id) {
            return Some(id);
        }
        let owner = self.entity_auto_service(service)?;
        let id = parameter_id(&owner, direction, name);
        self.has(&id).then_some(id)
    }

    /// `binding`'s target as the index knows it: the derived service's parameter of an entity-auto name named by the
    /// entity's short name (`create#Example`). Left as it is otherwise.
    pub fn resolve_binding(&self, binding: &mut BindingRow) {
        if self.has(&binding.to_id) {
            return;
        }
        let Some((owner, direction, name)) = super::extract::parse_parameter_id(&binding.to_id) else {
            return;
        };
        if let Some(found) = owner
            .strip_prefix("service:")
            .and_then(|service| self.service_parameter(service, direction, name))
        {
            binding.to_id = found;
        }
    }

    /// The id of the derived service of an entity-auto name, by the entity's full name (`service:create#a.B`), when
    /// the entity is known.
    fn entity_auto_service(&self, name: &str) -> Option<String> {
        let (verb, noun) = entity_auto(name)?;
        let full = self.entity(noun)?;
        Some(format!("service:{verb}#{full}"))
    }

    /// The derived services the service and parameter references of `refs` need: for each entity-auto name
    /// (`create`, `update`, `store`, `delete` and an entity) of a known entity that no file defines a service for and
    /// none is derived for yet, the service and its parameters (origin `derived`, in the pseudo file
    /// [`DERIVED_PATH`]). Not added: the caller adds them, as it adds the files' artifacts.
    #[must_use]
    pub fn derive<'a>(&self, refs: impl IntoIterator<Item = &'a RefRow>) -> Vec<ArtifactRow> {
        let mut made: Vec<String> = Vec::new();
        let mut rows = Vec::new();
        for reference in refs {
            let name = match reference.to_kind {
                ToKind::Service => &reference.to_name,
                ToKind::Parameter => &reference.to_scope,
                _ => continue,
            };
            if self.has(&format!("service:{name}")) {
                continue;
            }
            let Some(id) = self.entity_auto_service(name) else {
                continue;
            };
            if self.has(&id) || made.contains(&id) {
                continue;
            }
            rows.extend(self.derived_service(&id));
            made.push(id);
        }
        rows
    }

    /// The derived services known, by id.
    #[must_use]
    pub fn derived_ids(&self) -> Vec<String> {
        self.derived.keys().cloned().collect()
    }

    /// The rows of the derived service `id`, as known.
    #[must_use]
    pub fn derived_rows(&self, id: &str) -> Vec<ArtifactRow> {
        self.derived.get(id).cloned().unwrap_or_default()
    }

    /// The service `id` (`service:<verb>#<entity full name>`) and its parameters as the entity's fields give them now,
    /// by the rules of the entity-auto runner (`moqui_core::service::call::run_entity`, Java's
    /// `EntityAutoServiceRunner`), leaving out `lastUpdatedStamp`:
    /// - create: each primary key field in and out; a single key is optional (sequenced when left out), and of several
    ///   all but the last are required (the last gets a secondary sequenced id); a `fromDate` key is optional (now);
    ///   the other fields in, optional.
    /// - store: as create (a store with its key left out creates).
    /// - update: the key required in and out, the other fields in, optional.
    /// - delete: the key required in, nothing else.
    /// - update and store of an entity with `statusId`: `oldStatusId` and `statusChanged` out.
    ///
    /// Empty when `id` is no entity-auto name.
    #[must_use]
    pub fn derived_service(&self, id: &str) -> Vec<ArtifactRow> {
        let Some((verb, full)) = id.strip_prefix("service:").and_then(|name| name.split_once('#')) else {
            return Vec::new();
        };
        if entity_auto(&format!("{verb}#{full}")).is_none() {
            return Vec::new();
        }
        let name = format!("{verb}#{full}");
        let row =
            |id: String, kind: &str, name: &str, short_name: String, summary: String, parent: String| ArtifactRow {
                id,
                kind: kind.to_string(),
                name: name.to_string(),
                short_name,
                component: String::new(),
                path: DERIVED_PATH.to_string(),
                line: 0,
                column: 0,
                summary,
                parent: Some(parent),
                origin: ORIGIN_DERIVED.to_string(),
            };
        let mut rows = vec![row(
            id.to_string(),
            "service",
            &name,
            short_of("service", &name),
            format!("entity-auto {verb} of {full}"),
            format!("entity:{full}"),
        )];
        let mut parameter = |direction: &str, field: &str, kind: &str, required: bool| {
            let parameter_name = parameter_name(id, direction, field);
            rows.push(row(
                parameter_id(id, direction, field),
                "parameter",
                &parameter_name,
                short_of("parameter", &parameter_name),
                format!("{direction} {kind}, required={required}"),
                id.to_string(),
            ));
        };
        let fields: &[AutoField] = self.fields.get(full).map_or(&[], Vec::as_slice);
        let keys: Vec<&AutoField> = fields.iter().filter(|field| field.pk).collect();
        for (at, key) in keys.iter().enumerate() {
            let required = match verb {
                "update" | "delete" => true,
                // create and store: a key left out is sequenced (the last of several by a secondary id)
                _ => at + 1 < keys.len() && key.name != "fromDate",
            };
            parameter("in", &key.name, &key.kind, required);
        }
        if verb != "delete" {
            for field in fields
                .iter()
                .filter(|field| !field.pk && field.name != "lastUpdatedStamp")
            {
                parameter("in", &field.name, &field.kind, false);
            }
            for key in &keys {
                parameter("out", &key.name, &key.kind, false);
            }
            if matches!(verb, "update" | "store") && fields.iter().any(|field| field.name == "statusId") {
                parameter("out", "oldStatusId", "id", false);
                parameter("out", "statusChanged", "Boolean", false);
            }
        }
        rows
    }

    /// The screen or transition the screen path `path` names, written in the screen at `from` (see the module doc):
    /// `screen:<location>` or `transition:<location>#<name>:<method>`. The query and fragment are not part of it.
    #[must_use]
    pub fn screen_path(&self, from: &str, path: &str) -> Option<String> {
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let (mut at, rest) = match path.strip_prefix('/') {
            Some(rest) => (self.screen(WEBROOT_SCREEN)?, rest),
            None => (self.screen(from)?, path),
        };
        let segments: Vec<&str> = rest.split('/').filter(|segment| !segment.is_empty()).collect();
        for (index, segment) in segments.iter().enumerate() {
            match *segment {
                "." => {}
                ".." => at = self.parent_screen(&at)?,
                name => match self.subscreen(&at, name) {
                    Some(child) => at = child,
                    None if index + 1 == segments.len() => return self.base_transition(&at, name),
                    None => return None,
                },
            }
        }
        Some(format!("screen:{at}"))
    }

    /// The location of the subscreen `name` of the screen at `screen`: its `subscreens-item` of that name, else the
    /// default subscreen of its directory (`Foo/<name>.xml` of `Foo.xml`).
    #[must_use]
    pub fn subscreen(&self, screen: &str, name: &str) -> Option<String> {
        let item = self
            .items
            .get(screen)
            .and_then(|items| items.iter().find(|(item, _)| item == name))
            .and_then(|(_, location)| self.screen(location));
        item.or_else(|| {
            let location = format!("{}/{name}.xml", screen.strip_suffix(".xml")?);
            self.has(&format!("screen:{location}")).then_some(location)
        })
    }

    /// The parent of the screen at `screen`, which a path's `..` goes up to: the screen of its directory (`Foo.xml` of
    /// `Foo/Bar.xml`), else the first (by location) of the screens whose `subscreens-item` names it.
    #[must_use]
    pub fn parent_screen(&self, screen: &str) -> Option<String> {
        directory_parent(screen)
            .filter(|parent| self.has(&format!("screen:{parent}")))
            .or_else(|| {
                self.item_parents
                    .get(screen)
                    .and_then(|parents| parents.iter().min())
                    .cloned()
            })
    }

    /// The transition `name` of the screen at `base`, by its plain id or its method-qualified one.
    fn base_transition(&self, base: &str, name: &str) -> Option<String> {
        let plain = format!("transition:{base}#{name}");
        if self.has(&plain) {
            return Some(plain);
        }
        // Extension anchors identify a named XML element, independent of request dispatch.
        // Keep them navigable after transitions acquire method-qualified IDs.
        let prefix = format!("{plain}:");
        let any = format!("{prefix}any");
        if self.has(&any) {
            Some(any)
        } else {
            self.ids.keys().filter(|id| id.starts_with(&prefix)).min().cloned()
        }
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
                    // a call of its entity-auto services, which resolves to the derived service or the entity
                    names.extend(Self::entity_auto_names(artifact));
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
                        // the id ends with the method (`#save:post`); a reference names the transition only
                        names.push(target.rsplit_once(':').map_or(target, |(name, _)| name).to_string());
                    }
                }
                "form" | "anchor" => names.push(artifact.name.clone()),
                "script" | "data" | "skill" => names.push(artifact.name.clone()),
                // a widget-template-include's location: `<file>#<template>`
                "widget-template" => {
                    if let Some(location) = artifact.id.strip_prefix("widget-template:") {
                        names.push(location.to_string());
                    }
                }
                _ => {}
            }
        }
        names.sort();
        names.dedup();
        names
    }
}

impl Resolver {
    /// The entity-auto service names of the entity `artifact`, by its full and short name (`create#moqui.basic.Enumeration`,
    /// `create#Enumeration`, and `update`, `store`, `delete`).
    #[must_use]
    pub fn entity_auto_names(artifact: &ArtifactRow) -> Vec<String> {
        let mut names = Vec::new();
        for verb in ["create", "update", "store", "delete"] {
            for name in [&artifact.name, &artifact.short_name] {
                if !name.is_empty() {
                    names.push(format!("{verb}#{name}"));
                }
            }
        }
        names
    }
}

/// The verb and entity noun of an entity-auto service name (`create#moqui.basic.Enumeration`).
fn entity_auto(name: &str) -> Option<(&str, &str)> {
    let (verb, noun) = name.split_once('#')?;
    (matches!(verb, "create" | "update" | "store" | "delete") && !noun.is_empty()).then_some((verb, noun))
}

/// The parent screen and the named location of a `subscreens-item` artifact of a screen, when it names one.
fn item_of(artifact: &ArtifactRow) -> Option<(&str, &str)> {
    let parent = artifact.parent.as_deref()?.strip_prefix("screen:")?;
    let location = artifact.summary.as_str();
    location.contains("://").then_some((parent, location))
}

/// The screen whose default subscreen the screen at `location` is, by Moqui's directory rule: `Foo.xml` beside the
/// directory `Foo/` that holds it. None for a screen directly in its component's `screen` directory.
#[must_use]
pub fn directory_parent(location: &str) -> Option<String> {
    let (directory, _) = location.rsplit_once('/')?;
    if directory.ends_with("/screen") || !directory.contains("/screen/") {
        return None;
    }
    Some(format!("{directory}.xml"))
}

/// A screen location's path under its component's `screen` directory.
fn screen_tail(location: &str) -> Option<&str> {
    location
        .strip_prefix("component://")
        .and_then(|rest| rest.split_once("/screen/"))
        .map(|(_, tail)| tail)
}
