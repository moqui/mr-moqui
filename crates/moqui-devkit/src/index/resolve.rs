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
//! A screen path (a `link`'s or a response's `url`, a form's `transition`, a `default-item`, [`ToKind::ScreenPath`]) is
//! walked as Moqui walks a screen URL (`moqui_core::screen::url`), from the screen the path is written in: `.` and
//! `~name=value` segments are dropped, `..` goes up to the screen's parent, and each other segment is a subscreen of the
//! screen reached (its mount in the conf or a `SubscreensItem` row, its `subscreens-item`, else the directory
//! subscreen: `Foo/Bar.xml` of `Foo.xml`), through a `no-sub-path` item that holds it, or a transition of it, which
//! ends the path (the rest is extra path). `Name.<mode>` is the screen or transition `Name` in a render mode. A path
//! starting with `/` is walked from the webroot screen, and one starting with `//` is searched for under it as Moqui's
//! `findSubscreenPath` does. A path that cannot be walked, or that `..` walks from more than one parent to different
//! artifacts, stays unresolved.
//!
//! An entity-auto name (`create#moqui.example.Example`) that no file defines a service for gets a derived service
//! ([`Resolver::derive`], origin `derived`), with the in and out parameters the entity-auto runner takes and gives
//! (`moqui_core::service::call::run_entity`), so a call's `field-map`s resolve to its parameters as to a defined
//! service's.

use std::collections::{HashMap, HashSet};

use super::extract::{
    parameter_id, parameter_name, short_of, ArtifactRow, BindingRow, RefRow, ToKind, CALLS_MARK, CONF_ITEM,
    DERIVED_PATH, NO_SUB_PATH_MARK, ORIGIN_DERIVED, OWN_IN_MAP_MARK, PK_MARK, REQUEST_MARK, ROW_ITEM,
    SCREEN_PATH_OWNER,
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
    /// A screen's location to its subscreens items: its own (and its `screen-extend`s'), its conf mounts and its
    /// `SubscreensItem` rows.
    items: HashMap<String, Vec<Item>>,
    /// A screen's location to the screens whose items name it, once for each item.
    item_parents: HashMap<String, Vec<String>>,
    /// A screen's location to its directory subscreens' locations (`Foo/Bar.xml` of `Foo.xml`).
    directory: HashMap<String, Vec<String>>,
    /// A transition's id without its method (`transition:<screen>#<name>`) to its ids with one.
    transitions: HashMap<String, Vec<String>>,
    /// A transition's id to the services it passes the request's parameters to ([`request_services`]).
    requests: HashMap<String, Vec<String>>,
}

/// One subscreens item of a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    name: String,
    location: String,
    no_sub_path: bool,
    /// Which wins for a name, as Moqui merges them: 1 a `subscreens-item` of the screen or a `screen-extend`, 2 a conf
    /// mount, 3 a `SubscreensItem` row of a user group, 4 a row of every user. A directory subscreen loses to any of
    /// them.
    rank: u8,
}

/// How deep a screen path's walk or search goes, as Moqui's.
const FIND_DEPTH: usize = 24;

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
                if let Some(parent) = directory_parent(&artifact.name) {
                    let children = self.directory.entry(parent).or_default();
                    let at = children.partition_point(|known| *known <= artifact.name);
                    children.insert(at, artifact.name.clone());
                }
            }
            "subscreen" => {
                if let Some((parent, item)) = item_of(artifact) {
                    self.item_parents
                        .entry(item.location.clone())
                        .or_default()
                        .push(parent.clone());
                    // in name and rank order, for [`Self::children`]
                    let items = self.items.entry(parent).or_default();
                    let at = items.partition_point(|known| (&known.name, known.rank) <= (&item.name, item.rank));
                    items.insert(at, item);
                }
            }
            "transition" => {
                if let Some((plain, _)) = artifact.id.rsplit_once(':') {
                    self.transitions
                        .entry(plain.to_string())
                        .or_default()
                        .push(artifact.id.clone());
                }
                let services = request_services(&artifact.summary);
                if !services.is_empty() {
                    self.requests.insert(artifact.id.clone(), services);
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
                if let Some(parent) = directory_parent(&artifact.name) {
                    remove_one(&mut self.directory, &parent, |known| *known == artifact.name);
                }
            }
            "subscreen" => {
                if let Some((parent, item)) = item_of(artifact) {
                    remove_one(&mut self.items, &parent, |known| *known == item);
                    remove_one(&mut self.item_parents, &item.location, |known| *known == parent);
                }
            }
            "transition" => {
                if let Some((plain, _)) = artifact.id.rsplit_once(':') {
                    remove_one(&mut self.transitions, plain, |known| *known == artifact.id);
                }
                self.requests.remove(&artifact.id);
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
            // a directory subscreen names its parent screen in the scope: an edge while both are there
            ToKind::Screen => self
                .screen(&reference.to_name)
                .filter(|_| reference.to_scope.is_empty() || self.has(&format!("screen:{}", reference.to_scope)))
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
    ///
    /// A form field's binding into a transition by its path (`parameter:screen-path:<screen>#<path>@in.<field>`)
    /// keeps that id in [`BindingRow::to_path`] and names the parameter of the first service the transition reached
    /// passes the request to, or the path's id again while it reaches none; it is resolved again from `to_path`.
    pub fn resolve_binding(&self, binding: &mut BindingRow) {
        if let Some(written) = binding.to_path.clone().or_else(|| {
            binding
                .to_id
                .starts_with(&format!("parameter:{SCREEN_PATH_OWNER}"))
                .then(|| binding.to_id.clone())
        }) {
            binding.to_id = self.path_parameter(&written).unwrap_or_else(|| written.clone());
            binding.to_path = Some(written);
            return;
        }
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

    /// The parameter a form field's binding into a transition by its path names: of the first service the transition
    /// the path reaches passes the request to.
    fn path_parameter(&self, written: &str) -> Option<String> {
        let (owner, direction, name) = super::extract::parse_parameter_id(written)?;
        let (from, path) = owner.strip_prefix(SCREEN_PATH_OWNER)?.split_once('#')?;
        let transition = self.screen_path(from, path)?;
        let service = self.requests.get(&transition)?.first()?;
        Some(
            self.service_parameter(service, direction, name)
                .unwrap_or_else(|| parameter_id(&format!("service:{service}"), direction, name)),
        )
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
        let path = path.trim();
        let path = path.split(['?', '#']).next().unwrap_or_default();
        if let Some(rest) = path.strip_prefix("//") {
            let names = path_segments(rest, false);
            return self.sparse(&self.screen(WEBROOT_SCREEN)?, &names, 0, &mut HashSet::new());
        }
        let (start, segments) = match path.strip_prefix('/') {
            Some(rest) => (self.screen(WEBROOT_SCREEN)?, path_segments(rest, false)),
            None => (self.screen(from)?, path_segments(path, true)),
        };
        let mut found = Vec::new();
        self.walk(&start, &segments, 0, &mut found);
        match found.len() {
            1 => found.pop(),
            _ => None,
        }
    }

    /// Walk `segments` from the screen at `at`, adding each artifact a walk ends at to `found` (more than one only when
    /// a `..` goes up to several parents).
    fn walk(&self, at: &str, segments: &[&str], depth: usize, found: &mut Vec<String>) {
        if depth > FIND_DEPTH || found.len() > 1 {
            return;
        }
        let Some((first, rest)) = segments.split_first() else {
            let id = format!("screen:{at}");
            if !found.contains(&id) {
                found.push(id);
            }
            return;
        };
        if *first == ".." {
            for parent in self.parents(at) {
                self.walk(&parent, rest, depth + 1, found);
            }
            return;
        }
        match self.step(at, first) {
            Some(Step::Screen(child)) => self.walk(&child, rest, depth + 1, found),
            // a transition ends the path: the rest is extra path
            Some(Step::Transition(id)) if !found.contains(&id) => found.push(id),
            Some(Step::Transition(_)) | None => {}
        }
    }

    /// Where the segment `name` goes from the screen at `at`, as Moqui's walk takes it: into a `no-sub-path` item
    /// that holds a child of that name first, then a subscreen, else a transition, else `Name.<mode>`'s screen or
    /// transition `Name`.
    fn step(&self, at: &str, name: &str) -> Option<Step> {
        let before = name
            .split_once('.')
            .map(|(before, _)| before)
            .filter(|before| !before.is_empty());
        let wrapper = self.items.get(at).and_then(|items| {
            items
                .iter()
                .filter(|item| item.no_sub_path)
                .filter_map(|item| self.screen(&item.location))
                .find(|wrapper| {
                    self.has_child(wrapper, name) || before.is_some_and(|before| self.has_child(wrapper, before))
                })
        });
        let at = wrapper.as_deref().unwrap_or(at);
        if let Some(child) = self.subscreen(at, name) {
            return Some(Step::Screen(child));
        }
        if let Some(transition) = self.base_transition(at, name) {
            return Some(Step::Transition(transition));
        }
        let before = before?;
        if let Some(child) = self.subscreen(at, before) {
            return Some(Step::Screen(child));
        }
        self.base_transition(at, before).map(Step::Transition)
    }

    /// Whether the screen at `screen` has a subscreen or a transition named `name`.
    fn has_child(&self, screen: &str, name: &str) -> bool {
        self.subscreen(screen, name).is_some() || self.base_transition(screen, name).is_some()
    }

    /// The artifact a sparse path (`//Commons/Party`, `names` its segments) names, searched for under the screen at
    /// `screen` as Moqui's `findSubscreenPath` searches: the first name a subscreen (its rest walked from it) or a
    /// transition, else a grandchild's subscreen, else the whole path under each child in turn.
    ///
    /// `tried` holds each screen searched with each rest of the names (by its length): a second search of it would
    /// find nothing again, and a cycle of items ends.
    fn sparse(
        &self,
        screen: &str,
        names: &[&str],
        depth: usize,
        tried: &mut HashSet<(String, usize)>,
    ) -> Option<String> {
        let (first, rest) = names.split_first()?;
        if depth >= FIND_DEPTH || !tried.insert((screen.to_string(), names.len())) {
            return None;
        }
        if let Some(child) = self.subscreen(screen, first) {
            if rest.is_empty() {
                return Some(format!("screen:{child}"));
            }
            return self.sparse(&child, rest, depth + 1, tried);
        }
        if let Some(transition) = self.base_transition(screen, first) {
            return Some(transition);
        }
        if let Some(grand) = self.children(screen).find_map(|child| self.subscreen(&child, first)) {
            if rest.is_empty() {
                return Some(format!("screen:{grand}"));
            }
            return self.sparse(&grand, rest, depth + 1, tried);
        }
        self.children(screen)
            .find_map(|child| self.sparse(&child, names, depth + 1, tried))
    }

    /// The locations of the subscreens of the screen at `screen`: its items' (the screens indexed; of several of a
    /// name the one that wins), in name order, then its directory subscreens not named by an item, in location order.
    fn children<'a>(&'a self, screen: &str) -> impl Iterator<Item = String> + 'a {
        let items: &'a [Item] = self.items.get(screen).map_or(&[], Vec::as_slice);
        let winners = items
            .iter()
            .enumerate()
            // the items are in name and rank order: the last of a name wins
            .filter(move |(at, item)| items.get(at + 1).is_none_or(|next| next.name != item.name))
            .filter_map(move |(_, item)| self.screen(&item.location));
        let directory = self
            .directory
            .get(screen)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(move |location| {
                let name = file_stem(location);
                items.binary_search_by(|item| item.name.as_str().cmp(name)).is_err()
            })
            .cloned();
        winners.chain(directory)
    }

    /// The location of the subscreen `name` of the screen at `screen`: the item of that name that wins (a row, else a
    /// conf mount, else a `subscreens-item`), else the directory subscreen (`Foo/<name>.xml` of `Foo.xml`).
    #[must_use]
    pub fn subscreen(&self, screen: &str, name: &str) -> Option<String> {
        let item = self
            .items
            .get(screen)
            .and_then(|items| {
                items
                    .iter()
                    .filter(|item| item.name == name)
                    .reduce(|best, item| if item.rank >= best.rank { item } else { best })
            })
            .and_then(|item| self.screen(&item.location));
        item.or_else(|| {
            let location = format!("{}/{name}.xml", screen.strip_suffix(".xml")?);
            self.has(&format!("screen:{location}")).then_some(location)
        })
    }

    /// The screens a path's `..` may go up to from the screen at `screen`: the screen of its directory (`Foo.xml` of
    /// `Foo/Bar.xml`) and each screen with an item that names it, in location order.
    #[must_use]
    pub fn parents(&self, screen: &str) -> Vec<String> {
        let mut parents: Vec<String> = directory_parent(screen)
            .filter(|parent| self.has(&format!("screen:{parent}")))
            .into_iter()
            .collect();
        parents.extend(self.item_parents.get(screen).into_iter().flatten().cloned());
        parents.sort();
        parents.dedup();
        parents
    }

    /// The transition `name` of the screen at `base`, by its plain id or its method-qualified one.
    fn base_transition(&self, base: &str, name: &str) -> Option<String> {
        let plain = format!("transition:{base}#{name}");
        if self.has(&plain) {
            return Some(plain);
        }
        // Extension anchors identify a named XML element, independent of request dispatch.
        // Keep them navigable after transitions acquire method-qualified IDs.
        let ids = self.transitions.get(&plain)?;
        let any = format!("{plain}:any");
        if ids.contains(&any) {
            Some(any)
        } else {
            ids.iter().min().cloned()
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

/// Where a step of a screen path goes.
enum Step {
    Screen(String),
    Transition(String),
}

/// The segments of a screen path as Moqui cleans them (`cleanupPathNameList`): no empty ones, no `.`, no `~name=value`,
/// and a `..` taking off the segment before it. With `up`, a `..` with none before it is kept, for the walk to go up
/// from the screen the path is written in; else it is dropped.
fn path_segments(path: &str, up: bool) -> Vec<&str> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            _ if segment.starts_with('~') => {}
            ".." => match segments.last() {
                Some(last) if *last != ".." => {
                    segments.pop();
                }
                _ if up => segments.push(".."),
                _ => {}
            },
            _ => segments.push(segment),
        }
    }
    segments
}

/// Remove one value of `key`'s list that `matches`, and the key once its list is empty.
fn remove_one<T>(map: &mut HashMap<String, Vec<T>>, key: &str, matches: impl Fn(&T) -> bool) {
    if let Some(values) = map.get_mut(key) {
        if let Some(at) = values.iter().position(matches) {
            values.remove(at);
        }
        if values.is_empty() {
            map.remove(key);
        }
    }
}

/// The file name of a location without its `.xml`.
fn file_stem(location: &str) -> &str {
    let name = location.rsplit('/').next().unwrap_or(location);
    name.strip_suffix(".xml").unwrap_or(name)
}

/// The screen an item artifact belongs to and the item: a `subscreens-item` of a screen or of a `screen-extend` (an
/// item of the screen it extends), or a conf mount or `SubscreensItem` row (ids ending `@conf`, `@row`). An item with
/// no location names `<the screen's directory>/<name>.xml`, as Moqui takes it.
fn item_of(artifact: &ArtifactRow) -> Option<(String, Item)> {
    let parent = artifact.parent.as_deref()?;
    let parent = match parent.strip_prefix("screen-extend:") {
        Some(extend) => super::extract::extended_location(extend),
        None => parent.strip_prefix("screen:")?.to_string(),
    };
    let (location, no_sub_path) = match artifact.summary.strip_suffix(NO_SUB_PATH_MARK) {
        Some(location) => (location, true),
        None => (artifact.summary.as_str(), false),
    };
    let location = if location.is_empty() {
        format!("{}/{}.xml", parent.strip_suffix(".xml")?, artifact.name)
    } else if location.contains("://") {
        location.to_string()
    } else {
        return None;
    };
    // a row of every user (`@row`) wins over one of a user group (`@row:<group>`), which only its members see
    let mark = artifact.id.rsplit_once('#').map_or("", |(_, item)| item);
    let rank = match mark.rsplit_once(ROW_ITEM) {
        Some((_, "")) => 4,
        Some((_, group)) if group.starts_with(':') => 3,
        _ if mark.ends_with(CONF_ITEM) => 2,
        _ => 1,
    };
    Some((
        parent,
        Item {
            name: artifact.name.clone(),
            location,
            no_sub_path,
            rank,
        },
    ))
}

/// The services a transition passes the request's parameters to, as its summary says them ([`super::extract`]'s
/// transition summary: `calls <service>`, `own in-map` when that call takes none, `request to <service>` for each other).
#[must_use]
pub fn request_services(summary: &str) -> Vec<String> {
    let parts: Vec<&str> = summary.split(", ").collect();
    let own = parts.contains(&OWN_IN_MAP_MARK);
    parts
        .iter()
        .filter_map(|part| match part.strip_prefix(CALLS_MARK) {
            Some(service) => (!own).then_some(service),
            None => part.strip_prefix(REQUEST_MARK),
        })
        .filter(|service| crate::refs::is_literal(service))
        .map(str::to_string)
        .collect()
}

/// The screen whose directory subscreen the screen at `location` is, by Moqui's directory rule: `Foo.xml` beside the
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
