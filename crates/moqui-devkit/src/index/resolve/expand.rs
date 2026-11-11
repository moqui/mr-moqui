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

//! What an artifact declares to be filled in from elsewhere, expanded from the artifacts the resolver knows
//! (plans/design-dev-aide.md 7.1), as Moqui fills it in:
//! - a service's parameters from its `implements` (the implemented services' first, `required` set when the element
//!   says, as `moqui_core::service::define`'s `implemented`) and its `auto-parameters` (the entity's fields by
//!   `include` and `exclude`, as `moqui_core::service::validate::auto_fields`), the parameters it declares winning;
//! - a form's fields from its `extends` (a `DbForm` left out), `auto-fields-entity` and `auto-fields-service` (an
//!   entity-auto name the noun's fields), its own fields winning, as `moqui_core::screen::form`'s `assemble_node`,
//!   `add_entity_fields` and `add_service_fields`: no `lastUpdatedStamp`, and a form-list leaves out the very long text
//!   types; each with its binding into the services the form's transition calls;
//! - a form field's binding into a transition by its path, to each service the transition reached passes the request
//!   to, by the transition's `field-map`s.
//!
//! The rows are made from the declarations alone (each service's, form's and entity's own artifacts and the marks of
//! their summaries, [`super::super::extract::ArtifactRow::marks`]), never from rows expanded before, so the order they are made
//! in does not matter, and a cycle of `implements` or `extends` ends.

use std::collections::{HashMap, HashSet};

use super::{entity_auto, AutoField, Method, Resolver, FIND_DEPTH};
use crate::index::extract::{
    extended_form, extended_location, parameter_id, parameter_name, parse_parameter_id, short_of, split_marks,
    ArtifactRow, BindingRow, SCREEN_PATH_OWNER,
};

/// One definition of a service, form or transition whose marks say what it declares, and where it is.
#[derive(Clone, Debug)]
pub(super) struct Owner {
    kind: String,
    path: String,
    component: String,
    line: u32,
    column: u32,
    /// A `form-list`, whose auto fields leave out the very long text types.
    form_list: bool,
    marks: Vec<String>,
}

/// A parameter a service declares, in the file `path`.
#[derive(Clone, Debug)]
pub(super) struct Param {
    path: String,
    direction: String,
    name: String,
    kind: String,
    required: String,
    default: Option<String>,
    /// The attributes it sets (`type`, `required`, `default`) of a parameter of its name it merges into; all of them
    /// when none is said (a derived entity-auto service's).
    given: Option<Vec<String>>,
    /// Its summary as its row has it, which [`Expansion::summaries`] writes again when the merge changes it.
    stored: String,
}

/// A field a form declares, in the file `path`.
#[derive(Clone, Debug)]
pub(super) struct DeclaredField {
    path: String,
    name: String,
    widget: Widget,
}

/// The widget of a declared form field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Widget {
    /// A widget of its own: one the user fills (not only a submit), or not.
    Own { filled: bool },
    /// None: it takes the widget of a field of its name it merges into.
    Inherited,
}

/// What an owner's rows expand to: artifacts in its file, under it, with an expanded origin; a form's bindings; and the
/// summaries of the parameters it declares that the merge changes (a `required` an implemented service gives).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    pub artifacts: Vec<ArtifactRow>,
    pub bindings: Vec<BindingRow>,
    /// Each declared parameter's id with its summary as merged, where it differs from its row's.
    pub summaries: Vec<(String, String)>,
}

/// A service a form's transition calls, for the check that each required in-parameter is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormCall {
    /// The transition reached.
    pub transition: String,
    pub service: String,
    /// It takes the request's parameters; else only its `field-map`s reach it (a call with an `in-map` of its own).
    pub passes: bool,
    /// Its `field-map`s from a request parameter (`from`) to one of its parameters.
    pub maps: Vec<(String, String)>,
}

/// One mark, parsed ([`super::super::extract::ArtifactRow::marks`]).
enum Mark<'a> {
    AutoParameters {
        direction: &'a str,
        entity: Option<&'a str>,
        include: &'a str,
        required: bool,
        excludes: Vec<&'a str>,
    },
    Implements {
        service: &'a str,
        required: Option<bool>,
    },
    AutoFieldsEntity {
        entity: &'a str,
        include: &'a str,
        field_type: &'a str,
        excludes: Vec<&'a str>,
    },
    AutoFieldsService {
        service: &'a str,
        include: &'a str,
        field_type: &'a str,
        excludes: Vec<&'a str>,
    },
    Extends(&'a str),
    Transition(&'a str),
    TransitionPath(&'a str),
    Call {
        service: &'a str,
        passes: bool,
        maps: Vec<(&'a str, &'a str)>,
    },
    /// A form's parameters that are no field of it; a parameter's attributes it sets.
    Given(Vec<&'a str>),
    PassThrough,
    NoWidget,
    Unfilled,
}

fn list(text: &str) -> Vec<&str> {
    if text == "-" {
        Vec::new()
    } else {
        text.split(',').filter(|item| !item.is_empty()).collect()
    }
}

fn parse_mark(text: &str) -> Option<Mark<'_>> {
    let words: Vec<&str> = text.split(' ').collect();
    Some(match words.as_slice() {
        ["auto-parameters", direction, entity, include, required, excludes] => Mark::AutoParameters {
            direction,
            entity: (*entity != "-").then_some(*entity),
            include,
            required: *required == "true",
            excludes: list(excludes),
        },
        ["implements", service, required] => Mark::Implements {
            service,
            required: match *required {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
        },
        ["auto-fields-entity", entity, include, field_type, excludes] => Mark::AutoFieldsEntity {
            entity,
            include,
            field_type,
            excludes: list(excludes),
        },
        ["auto-fields-service", service, include, field_type, excludes] => Mark::AutoFieldsService {
            service,
            include,
            field_type,
            excludes: list(excludes),
        },
        ["extends", form] => Mark::Extends(form),
        ["transition", name] => Mark::Transition(name),
        ["transition-path", path] => Mark::TransitionPath(path),
        ["call", service, passes, maps] => Mark::Call {
            service,
            passes: *passes == "y",
            maps: list(maps).into_iter().filter_map(|map| map.split_once('=')).collect(),
        },
        ["given", names] => Mark::Given(list(names)),
        ["pass-through"] => Mark::PassThrough,
        ["no-widget"] => Mark::NoWidget,
        ["unfilled"] => Mark::Unfilled,
        _ => return None,
    })
}

/// A parameter as the service takes it, merged as Moqui merges them (`moqui_core::service::define::merge_param`), with
/// where it comes from (empty: declared).
#[derive(Clone, Debug)]
struct FullParam {
    name: String,
    kind: String,
    /// `true`, `false` or `disabled`.
    required: String,
    default: Option<String>,
    origin: &'static str,
    /// For a declared one, its summary as its row has it.
    stored: Option<String>,
}

impl FullParam {
    fn summary(&self, direction: &str) -> String {
        let mut summary = format!("{direction} {}, required={}", self.kind, self.required);
        if let Some(default) = &self.default {
            summary.push_str(&format!(", default {default}"));
        }
        summary
    }

    /// Merge `other` into this one: the attributes it sets (all when `given` is none).
    fn merge(&mut self, other: FullParam, given: Option<&[String]>) {
        let sets = |attribute: &str| given.is_none_or(|given| given.iter().any(|known| known == attribute));
        if sets("type") {
            self.kind = other.kind;
        }
        if sets("required") {
            self.required = other.required;
        }
        if sets("default") {
            self.default = other.default;
        }
        self.origin = other.origin;
        if other.stored.is_some() {
            self.stored = other.stored;
        }
    }
}

/// Merge `param` into `out` by its name ([`FullParam::merge`]), or add it.
fn merge_into(out: &mut Vec<FullParam>, param: FullParam, given: Option<&[String]>) {
    match out.iter_mut().find(|known| known.name == param.name) {
        Some(known) => known.merge(param, given),
        None => out.push(param),
    }
}

/// A field as the form has it, with where it comes from.
#[derive(Clone, Debug)]
struct FullField {
    name: String,
    summary: String,
    /// Empty when the form declares it.
    origin: &'static str,
    /// What its widget comes from: its own origin, or an inherited field's when the form declares it with no widget.
    source: &'static str,
    /// The user fills it.
    filled: bool,
    /// The expansion makes its binding: an inherited field the user fills, or a declared one with no widget over one.
    bind: bool,
}

/// The text types a form-list's auto fields leave out, as Moqui's `add_entity_fields`.
const LONG_TEXT: [&str; 3] = ["text-very-long", "text-fts", "binary-very-long"];

/// The written id and slot of a form field's binding into a transition by its path: its
/// [`BindingRow::to_path`] (`<id>#<k>`), else its `to_id` while not yet resolved. None for any other binding.
#[must_use]
pub fn path_seed(binding: &BindingRow) -> Option<(String, usize)> {
    let prefix = format!("parameter:{SCREEN_PATH_OWNER}");
    let path = binding
        .to_path
        .as_deref()
        .or_else(|| binding.to_id.starts_with(&prefix).then_some(binding.to_id.as_str()))?;
    match path.rsplit_once('#') {
        Some((seed, slot))
            if seed.contains('@') && !slot.is_empty() && slot.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some((seed.to_string(), slot.parse().unwrap_or(0)))
        }
        _ => Some((path.to_string(), 0)),
    }
}

/// The base screen's form a `screen-extend`'s form adds its fields to (`form:<screen-extend location>#<name>`), none
/// for a form of a screen.
fn extension_base(form: &str) -> Option<String> {
    let (location, name) = form.strip_prefix("form:")?.rsplit_once('#')?;
    location
        .contains("/screen-extend/")
        .then(|| format!("form:{}#{name}", extended_location(location)))
}

/// The screen location of a form id (`form:<screen>#<name>`).
fn form_screen(form: &str) -> &str {
    form.strip_prefix("form:")
        .and_then(|key| key.rsplit_once('#'))
        .map_or("", |(screen, _)| screen)
}

/// A declared parameter's type, required and default from its summary (`in Long, required=true, default 1`), each
/// only when it sets it (`given`), else Moqui's default.
fn declared_param(path: &str, direction: &str, name: &str, summary: &str, given: Option<Vec<String>>) -> Param {
    let sets = |attribute: &str| {
        given
            .as_ref()
            .is_none_or(|given| given.iter().any(|known| known == attribute))
    };
    let mut kind = "String".to_string();
    let mut required = "false".to_string();
    let mut default = None;
    for (at, part) in summary.split(", ").enumerate() {
        if at == 0 {
            if let Some((_, found)) = part.split_once(' ') {
                if sets("type") {
                    kind = found.to_string();
                }
            }
        } else if let Some(found) = part.strip_prefix("required=") {
            if sets("required") {
                required = found.to_string();
            }
        } else if let Some(found) = part.strip_prefix("default ") {
            if sets("default") {
                default = Some(found.to_string());
            }
        }
    }
    Param {
        path: path.to_string(),
        direction: direction.to_string(),
        name: name.to_string(),
        kind,
        required,
        default,
        given,
        stored: summary.to_string(),
    }
}

impl Resolver {
    /// Know what `artifact` declares for the expansions: an owner's marks, a service's parameter, a form's field. Each
    /// is kept by its file, so a second definition of an id (an override) and the removal of either keep the other.
    pub(super) fn add_declared(&mut self, artifact: &ArtifactRow) {
        match artifact.kind.as_str() {
            "service" | "form" | "transition" => {
                let marks = split_marks(&artifact.marks);
                if marks.is_empty() {
                    return;
                }
                let owners = self.owners.entry(artifact.id.clone()).or_default();
                owners.retain(|owner| owner.path != artifact.path);
                owners.push(Owner {
                    kind: artifact.kind.clone(),
                    path: artifact.path.clone(),
                    component: artifact.component.clone(),
                    line: artifact.line,
                    column: artifact.column,
                    form_list: artifact.summary.starts_with("form-list"),
                    marks: marks.into_iter().map(str::to_string).collect(),
                });
            }
            "parameter" => {
                let Some((owner, direction, name)) = parse_parameter_id(&artifact.id) else {
                    return;
                };
                if !owner.starts_with("service:") || name.contains('.') || !matches!(direction, "in" | "out") {
                    return;
                }
                let given = split_marks(&artifact.marks)
                    .into_iter()
                    .find_map(|mark| match parse_mark(mark) {
                        Some(Mark::Given(given)) => Some(given.into_iter().map(str::to_string).collect()),
                        _ => None,
                    });
                let param = declared_param(&artifact.path, direction, name, &artifact.summary, given);
                let params = self.params.entry(owner.to_string()).or_default();
                params
                    .retain(|known| !(known.path == param.path && known.direction == direction && known.name == name));
                params.push(param);
            }
            "entity" | "view-entity" => {
                let paths = self.entity_paths.entry(artifact.name.clone()).or_default();
                if !paths.contains(&artifact.path) {
                    paths.push(artifact.path.clone());
                }
            }
            "form-field" => {
                let (Some(form), Some((_, name))) = (&artifact.parent, artifact.id.rsplit_once('@')) else {
                    return;
                };
                let marks = split_marks(&artifact.marks);
                let widget = if marks.contains(&"no-widget") {
                    Widget::Inherited
                } else {
                    Widget::Own {
                        filled: !marks.contains(&"unfilled"),
                    }
                };
                let field = DeclaredField {
                    path: artifact.path.clone(),
                    name: name.to_string(),
                    widget,
                };
                // a screen-extend's form adds its fields to the base screen's form of its name
                if let Some(base) = extension_base(form) {
                    let fields = self.form_extensions.entry(base).or_default();
                    fields.retain(|known| !(known.path == field.path && known.name == field.name));
                    fields.push(field.clone());
                }
                let fields = self.form_fields.entry(form.clone()).or_default();
                fields.retain(|known| !(known.path == field.path && known.name == field.name));
                fields.push(field);
            }
            _ => {}
        }
    }

    /// Forget what `artifact` declared in its file ([`Self::add_declared`]).
    pub(super) fn remove_declared(&mut self, artifact: &ArtifactRow) {
        match artifact.kind.as_str() {
            "entity" | "view-entity" => {
                super::remove_one(&mut self.entity_paths, &artifact.name, |path| *path == artifact.path);
            }
            "service" | "form" | "transition" => {
                super::remove_one(&mut self.owners, &artifact.id, |owner| owner.path == artifact.path);
            }
            "parameter" => {
                if let Some((owner, direction, name)) = parse_parameter_id(&artifact.id) {
                    super::remove_one(&mut self.params, owner, |param| {
                        param.path == artifact.path && param.direction == direction && param.name == name
                    });
                }
            }
            "form-field" => {
                if let (Some(form), Some((_, name))) = (&artifact.parent, artifact.id.rsplit_once('@')) {
                    if let Some(base) = extension_base(form) {
                        super::remove_one(&mut self.form_extensions, &base, |known| {
                            known.path == artifact.path && known.name == name
                        });
                    }
                    super::remove_one(&mut self.form_fields, form, |known| {
                        known.path == artifact.path && known.name == name
                    });
                }
            }
            _ => {}
        }
    }

    /// The definition of `id` the expansions read: of several (an override), the one whose file sorts last, so it
    /// does not depend on the order the files were read in.
    fn owner(&self, id: &str) -> Option<&Owner> {
        self.owners.get(id)?.iter().max_by(|a, b| a.path.cmp(&b.path))
    }

    fn marks_of(&self, id: &str) -> Vec<Mark<'_>> {
        self.owner(id)
            .map(|owner| owner.marks.iter().filter_map(|mark| parse_mark(mark)).collect())
            .unwrap_or_default()
    }

    /// The file whose definition of the service `service` the expansions read: its owner's, else the last of the
    /// files that declare parameters of it.
    fn service_path(&self, service: &str) -> Option<&str> {
        if let Some(owner) = self.owner(service) {
            return Some(&owner.path);
        }
        self.params.get(service)?.iter().map(|param| param.path.as_str()).max()
    }

    /// The parameters of `direction` the definition of `service` the expansions read declares.
    fn declared_params(&self, service: &str, direction: &str) -> Vec<&Param> {
        let path = self.service_path(service);
        self.params
            .get(service)
            .into_iter()
            .flatten()
            .filter(|param| param.direction == direction && Some(param.path.as_str()) == path)
            .collect()
    }

    /// Note that the row of the declared parameter `id` now has `summary` ([`Expansion::summaries`] written).
    pub fn set_stored_summary(&mut self, id: &str, summary: &str) {
        let Some((owner, direction, name)) = parse_parameter_id(id) else {
            return;
        };
        let path = self.service_path(owner).map(str::to_string);
        if let Some(params) = self.params.get_mut(owner) {
            for param in params.iter_mut() {
                if param.direction == direction && param.name == name && Some(&param.path) == path.as_ref() {
                    param.stored = summary.to_string();
                }
            }
        }
    }

    /// The fields of the entity `full` in a stable order: those of its own files first, then each `extend-entity`'s,
    /// by file and line, however the rows were read.
    pub(super) fn ordered_fields(&self, full: &str) -> Vec<&AutoField> {
        let own = self.entity_paths.get(full);
        let mut fields: Vec<&AutoField> = self.fields.get(full).into_iter().flatten().collect();
        fields.sort_by(|a, b| {
            let extension = |field: &AutoField| !own.is_some_and(|paths| paths.contains(&field.path));
            (extension(a), &a.path, a.line).cmp(&(extension(b), &b.path, b.line))
        });
        fields
    }

    /// Whether the service or form `id` declares something to expand.
    #[must_use]
    pub fn is_expandable(&self, id: &str) -> bool {
        self.owner(id).is_some_and(|owner| owner.kind != "transition")
            && self.marks_of(id).iter().any(|mark| {
                matches!(
                    mark,
                    Mark::AutoParameters { .. }
                        | Mark::Implements { .. }
                        | Mark::AutoFieldsEntity { .. }
                        | Mark::AutoFieldsService { .. }
                        | Mark::Extends(_)
                )
            })
    }

    /// The services and forms that declare something to expand, in id order.
    #[must_use]
    pub fn expandable(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .owners
            .keys()
            .filter(|id| self.is_expandable(id))
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// The file of the service or form `owner`'s definition the expansions read, when it declares something.
    #[must_use]
    pub fn owner_path(&self, owner: &str) -> Option<String> {
        self.owner(owner).map(|info| info.path.clone())
    }

    /// The rows expanded for `owner` as last written ([`Self::add`] of rows of an expanded origin).
    #[must_use]
    pub fn expanded_rows(&self, owner: &str) -> &[ArtifactRow] {
        self.expanded.get(owner).map_or(&[], Vec::as_slice)
    }

    /// The parameters a form posts that are no field of it (the ids of a row selection, its hidden parameters), and
    /// whether it passes on the parameters of its URL (`pass-through-parameters`).
    #[must_use]
    pub fn form_given(&self, form: &str) -> (Vec<String>, bool) {
        let mut given = Vec::new();
        let mut pass_through = false;
        for mark in self.marks_of(form) {
            match mark {
                Mark::Given(names) => given.extend(names.into_iter().map(str::to_string)),
                Mark::PassThrough => pass_through = true,
                _ => {}
            }
        }
        (given, pass_through)
    }

    /// What `owner` expands to now: the parameters of a service and the fields of a form that it does not declare
    /// itself (see the module doc), the bindings of the fields, and the summaries of the parameters it declares as
    /// merged. Empty for one with nothing to expand.
    #[must_use]
    pub fn expand(&self, owner: &str) -> Expansion {
        let Some(info) = self.owner(owner) else {
            return Expansion::default();
        };
        let row = |id: String, kind: &str, name: String, summary: String, origin: &str| ArtifactRow {
            short_name: short_of(kind, &name),
            id,
            kind: kind.to_string(),
            name,
            component: info.component.clone(),
            path: info.path.clone(),
            line: info.line,
            column: info.column,
            summary,
            parent: Some(owner.to_string()),
            origin: origin.to_string(),
            marks: String::new(),
        };
        let mut expansion = Expansion::default();
        match info.kind.as_str() {
            "service" => {
                let noun = owner.rsplit_once('#').map(|(_, noun)| noun);
                for direction in ["in", "out"] {
                    let mut visited = HashSet::from([owner.to_string()]);
                    for param in self.full_params(owner, direction, &mut visited, 0, noun) {
                        let id = parameter_id(owner, direction, &param.name);
                        let summary = param.summary(direction);
                        if param.origin.is_empty() {
                            if param.stored.as_ref().is_some_and(|stored| *stored != summary) {
                                expansion.summaries.push((id, summary));
                            }
                            continue;
                        }
                        expansion.artifacts.push(row(
                            id,
                            "parameter",
                            parameter_name(owner, direction, &param.name),
                            summary,
                            param.origin,
                        ));
                    }
                }
            }
            "form" => {
                let Some(form_key) = owner.strip_prefix("form:") else {
                    return expansion;
                };
                let form_name = form_key.rsplit_once('#').map_or(form_key, |(_, name)| name);
                let mut visited = HashSet::from([owner.to_string()]);
                let fields: Vec<FullField> = self
                    .full_fields(owner, &mut visited, 0)
                    .into_iter()
                    .filter(|field| !field.origin.is_empty() || field.bind)
                    .collect();
                if fields.is_empty() {
                    return expansion;
                }
                let screen = form_screen(owner);
                let target = self.marks_of(owner).into_iter().find_map(|mark| match mark {
                    Mark::Transition(name) => Some((false, name)),
                    Mark::TransitionPath(path) => Some((true, path)),
                    _ => None,
                });
                let same_screen_calls: Vec<FormCall> = match target {
                    Some((false, name)) => self
                        .transitions
                        .get(&format!("transition:{screen}#{name}"))
                        .into_iter()
                        .flatten()
                        .flat_map(|transition| self.calls_of(transition))
                        .collect(),
                    _ => Vec::new(),
                };
                for field in fields {
                    let source_id = format!("form-field:{form_key}@{}", field.name);
                    if field.bind {
                        let mut targets: Vec<String> = Vec::new();
                        if let Some((true, path)) = target {
                            targets.push(parameter_id(
                                &format!("{SCREEN_PATH_OWNER}{screen}#{path}"),
                                "in",
                                &field.name,
                            ));
                        }
                        for call in &same_screen_calls {
                            let mapped = call
                                .maps
                                .iter()
                                .find(|(from, _)| *from == field.name)
                                .map(|(_, to)| to.as_str());
                            let parameter = match mapped {
                                Some(parameter) => parameter,
                                None if call.passes => field.name.as_str(),
                                None => continue,
                            };
                            let id = parameter_id(&format!("service:{}", call.service), "in", parameter);
                            if !targets.contains(&id) {
                                targets.push(id);
                            }
                        }
                        for to_id in targets {
                            expansion.bindings.push(BindingRow {
                                from: owner.to_string(),
                                to_id,
                                direction: "in".to_string(),
                                source_kind: "form-field".to_string(),
                                source_text: field.name.clone(),
                                source_id: Some(source_id.clone()),
                                to_path: None,
                                origin: field.source.to_string(),
                                origin_hash: String::new(),
                                line: info.line,
                            });
                        }
                    }
                    if !field.origin.is_empty() {
                        expansion.artifacts.push(row(
                            source_id,
                            "form-field",
                            format!("{form_name}@{}", field.name),
                            field.summary,
                            field.origin,
                        ));
                    }
                }
            }
            _ => {}
        }
        expansion
    }

    /// The parameters of `direction` the service `service` takes, merged by name as Moqui merges them: its implemented
    /// services' first (a `required="disabled"` one left out, `required` set when the `implements` says), then its
    /// `auto-parameters` (an entity field sets the type and `required` of a parameter of its name), then the ones it
    /// declares (each setting what it gives). An `auto-parameters` with no entity reads the entity of `noun`, the
    /// implementing service's. `visited` holds the services on the way, so a cycle ends.
    ///
    /// Not as Moqui: a declared parameter merges after every `auto-parameters`, wherever it is written.
    fn full_params(
        &self,
        service: &str,
        direction: &str,
        visited: &mut HashSet<String>,
        depth: usize,
        noun: Option<&str>,
    ) -> Vec<FullParam> {
        let mut out: Vec<FullParam> = Vec::new();
        if depth > FIND_DEPTH {
            return out;
        }
        let marks = self.marks_of(service);
        for mark in &marks {
            let Mark::Implements {
                service: target,
                required,
            } = mark
            else {
                continue;
            };
            let target = format!("service:{target}");
            if !visited.insert(target.clone()) {
                continue;
            }
            for mut param in self.full_params(&target, direction, visited, depth + 1, noun) {
                if param.required == "disabled" || out.iter().any(|known| known.name == param.name) {
                    continue;
                }
                if let Some(required) = required {
                    param.required = required.to_string();
                }
                param.origin = "implements";
                param.stored = None;
                out.push(param);
            }
        }
        let auto_given = ["type".to_string(), "required".to_string()];
        for mark in &marks {
            let Mark::AutoParameters {
                direction: of,
                entity,
                include,
                required,
                excludes,
            } = mark
            else {
                continue;
            };
            if *of != direction {
                continue;
            }
            let Some(full) = entity.or(noun).and_then(|name| self.entity(name)) else {
                continue;
            };
            for field in self.ordered_fields(&full) {
                // as `moqui_core::service::validate::auto_fields`: any include but pk and nonpk takes all
                let take = match *include {
                    "pk" => field.pk,
                    "nonpk" => !field.pk,
                    _ => true,
                };
                if !take || excludes.contains(&field.name.as_str()) {
                    continue;
                }
                let param = FullParam {
                    name: field.name.clone(),
                    kind: field.kind.clone(),
                    required: required.to_string(),
                    default: None,
                    origin: "auto-parameters",
                    stored: None,
                };
                merge_into(&mut out, param, Some(&auto_given));
            }
        }
        let declared = self.declared_params(service, direction);
        if declared.is_empty() && marks.is_empty() {
            // an entity-auto name no file defines: the parameters of its derived service, made or not
            if let Some(id) = service
                .strip_prefix("service:")
                .and_then(|name| self.entity_auto_service(name))
            {
                if self.params.contains_key(&id) {
                    return self
                        .declared_params(&id, direction)
                        .into_iter()
                        .map(|param| FullParam {
                            name: param.name.clone(),
                            kind: param.kind.clone(),
                            required: param.required.clone(),
                            default: param.default.clone(),
                            origin: "",
                            stored: None,
                        })
                        .collect();
                }
                return self
                    .derived_service(&id)
                    .into_iter()
                    .filter(|row| row.kind == "parameter")
                    .filter_map(|row| {
                        let (_, of, name) = parse_parameter_id(&row.id)?;
                        (of == direction).then(|| {
                            let param = declared_param("", of, name, &row.summary, None);
                            FullParam {
                                name: param.name,
                                kind: param.kind,
                                required: param.required,
                                default: param.default,
                                origin: "",
                                stored: None,
                            }
                        })
                    })
                    .collect();
            }
        }
        for param in declared {
            let full = FullParam {
                name: param.name.clone(),
                kind: param.kind.clone(),
                required: param.required.clone(),
                default: param.default.clone(),
                origin: "",
                stored: Some(param.stored.clone()),
            };
            merge_into(&mut out, full, param.given.as_deref());
        }
        out
    }

    /// The fields the form `form` has: those of the form it extends (with the fields a `screen-extend` adds to that
    /// form), then those of its `auto-fields-entity` and `auto-fields-service`, then the ones it declares, each in
    /// place of one of its name (a declared one with no widget takes that one's widget).
    fn full_fields(&self, form: &str, visited: &mut HashSet<String>, depth: usize) -> Vec<FullField> {
        let mut out: Vec<FullField> = Vec::new();
        if depth > FIND_DEPTH {
            return out;
        }
        let screen = form_screen(form);
        let form_list = self.owner(form).is_some_and(|owner| owner.form_list);
        let inherit = |out: &mut Vec<FullField>, name: &str, summary: String, origin: &'static str, filled: bool| {
            if name != "lastUpdatedStamp" && !out.iter().any(|known| known.name == name) {
                out.push(FullField {
                    name: name.to_string(),
                    summary,
                    origin,
                    source: origin,
                    filled,
                    bind: filled,
                });
            }
        };
        for mark in self.marks_of(form) {
            match mark {
                Mark::Extends(extends) => {
                    let Some((location, name)) = extended_form(extends, screen) else {
                        continue;
                    };
                    let base = format!("form:{}#{name}", self.screen(&location).unwrap_or(location));
                    if !visited.insert(base.clone()) {
                        continue;
                    }
                    let summary = format!("extends {base}");
                    for field in self.full_fields(&base, visited, depth + 1) {
                        inherit(&mut out, &field.name, summary.clone(), "extends", field.filled);
                    }
                    for field in self.form_extensions.get(&base).into_iter().flatten() {
                        match (out.iter_mut().find(|known| known.name == field.name), field.widget) {
                            (Some(known), Widget::Own { filled }) => {
                                known.filled = filled;
                                known.bind = filled;
                            }
                            (Some(_), Widget::Inherited) => {}
                            (None, widget) => inherit(
                                &mut out,
                                &field.name,
                                summary.clone(),
                                "extends",
                                widget == Widget::Own { filled: true },
                            ),
                        }
                    }
                }
                Mark::AutoFieldsEntity {
                    entity,
                    include,
                    field_type,
                    excludes,
                } => {
                    let Some(full) = self.entity(entity) else {
                        continue;
                    };
                    // as Moqui's `add_entity_fields`: all, pk or nonpk, and any other include takes none
                    let (pk, nonpk) = match include {
                        "all" => (true, true),
                        "pk" => (true, false),
                        "nonpk" => (false, true),
                        _ => (false, false),
                    };
                    for field in self.ordered_fields(&full) {
                        if !(if field.pk { pk } else { nonpk }) || excludes.contains(&field.name.as_str()) {
                            continue;
                        }
                        if form_list && LONG_TEXT.contains(&field.kind.as_str()) {
                            continue;
                        }
                        inherit(
                            &mut out,
                            &field.name,
                            format!("{field_type} of {full}"),
                            "auto-fields-entity",
                            true,
                        );
                    }
                }
                Mark::AutoFieldsService {
                    service,
                    include,
                    field_type,
                    excludes,
                } => {
                    let id = format!("service:{service}");
                    let summary = format!("{field_type} of {service}");
                    // a defined service's parameters; an entity-auto name's are its noun's fields, as Moqui takes them
                    if self.has(&id) && !self.derived.contains_key(&id) {
                        let directions: &[&str] = match include {
                            "in" => &["in"],
                            "out" => &["out"],
                            "all" => &["in", "out"],
                            _ => &[],
                        };
                        let noun = service.rsplit_once('#').map(|(_, noun)| noun);
                        for direction in directions {
                            let mut seen = HashSet::from([id.clone()]);
                            for param in self.full_params(&id, direction, &mut seen, 0, noun) {
                                if !excludes.contains(&param.name.as_str()) {
                                    inherit(&mut out, &param.name, summary.clone(), "auto-fields-service", true);
                                }
                            }
                        }
                    } else if let Some(full) = entity_auto(service).and_then(|(_, noun)| self.entity(noun)) {
                        for field in self.ordered_fields(&full) {
                            if form_list && LONG_TEXT.contains(&field.kind.as_str()) {
                                continue;
                            }
                            inherit(&mut out, &field.name, summary.clone(), "auto-fields-service", true);
                        }
                    }
                }
                _ => {}
            }
        }
        for declared in self.form_fields.get(form).into_iter().flatten() {
            match out.iter_mut().find(|known| known.name == declared.name) {
                Some(known) => {
                    known.origin = "";
                    match declared.widget {
                        // merged into the inherited field, whose widget it keeps: bound as that one
                        Widget::Inherited => known.bind = known.filled,
                        Widget::Own { filled } => {
                            known.filled = filled;
                            known.bind = false;
                        }
                    }
                }
                None => out.push(FullField {
                    name: declared.name.clone(),
                    summary: String::new(),
                    origin: "",
                    source: "",
                    filled: declared.widget == Widget::Own { filled: true },
                    bind: false,
                }),
            }
        }
        out
    }

    /// The services the transition `transition` calls with the request or with `field-map`s from it, by its marks;
    /// else the services its summary says it passes the request to.
    fn calls_of(&self, transition: &str) -> Vec<FormCall> {
        let calls: Vec<FormCall> = self
            .marks_of(transition)
            .into_iter()
            .filter_map(|mark| match mark {
                Mark::Call { service, passes, maps } => Some(FormCall {
                    transition: transition.to_string(),
                    service: service.to_string(),
                    passes,
                    maps: maps
                        .into_iter()
                        .map(|(from, to)| (from.to_string(), to.to_string()))
                        .collect(),
                }),
                _ => None,
            })
            .collect();
        if !calls.is_empty() {
            return calls;
        }
        self.requests
            .get(transition)
            .into_iter()
            .flatten()
            .map(|service| FormCall {
                transition: transition.to_string(),
                service: service.clone(),
                passes: true,
                maps: Vec::new(),
            })
            .collect()
    }

    /// The services the form `form` posts to: those its transition calls ([`Self::calls_of`]), the transition of its
    /// screen (a `post` one first) or the one its path reaches.
    #[must_use]
    pub fn form_calls(&self, form: &str) -> Vec<FormCall> {
        let screen = form_screen(form);
        let transition = self.marks_of(form).into_iter().find_map(|mark| match mark {
            Mark::Transition(name) => self.base_transition(screen, name, Method::Post),
            Mark::TransitionPath(path) => self
                .screen_path_as(screen, path, Method::Post)
                .filter(|id| id.starts_with("transition:")),
            _ => None,
        });
        transition
            .map(|transition| self.calls_of(&transition))
            .unwrap_or_default()
    }

    /// The in-parameters the service `service` takes ([`Self::full_params`]), each with whether it is required and
    /// whether it has a default; none when the index knows no parameters of it.
    #[must_use]
    pub fn in_parameters(&self, service: &str) -> Option<Vec<(String, bool, bool)>> {
        let id = format!("service:{service}");
        let mut visited = HashSet::from([id.clone()]);
        let noun = service.rsplit_once('#').map(|(_, noun)| noun);
        let params = self.full_params(&id, "in", &mut visited, 0, noun);
        if params.is_empty() && !self.has(&id) {
            return None;
        }
        Some(
            params
                .into_iter()
                .map(|param| (param.name, param.required == "true", param.default.is_some()))
                .collect(),
        )
    }

    /// The bindings a form field's binding into a transition by its path stands for (`seed`, its written id
    /// `parameter:screen-path:<screen>#<path>@in.<field>`): one for each service the transition the path reaches calls
    /// with the request or a `field-map` of the field, to the parameter of the field's name or the one the field-map
    /// names, each with its slot in [`BindingRow::to_path`] (`<written id>#<k>`); one to the written id while the path
    /// reaches no such service. A binding of no path is answered as it is.
    #[must_use]
    pub fn path_bindings(&self, seed: &BindingRow) -> Vec<BindingRow> {
        let Some((written, _)) = path_seed(seed) else {
            return vec![seed.clone()];
        };
        let slot = |to_id: String, at: usize| BindingRow {
            to_id,
            to_path: Some(format!("{written}#{at}")),
            ..seed.clone()
        };
        let Some((owner, direction, field)) = parse_parameter_id(&written) else {
            return vec![slot(written.clone(), 0)];
        };
        let transition = owner
            .strip_prefix(SCREEN_PATH_OWNER)
            .and_then(|rest| rest.split_once('#'))
            .and_then(|(from, path)| self.screen_path_as(from, path, Method::Post))
            .filter(|id| id.starts_with("transition:"));
        let mut targets: Vec<String> = Vec::new();
        for call in transition
            .map(|transition| self.calls_of(&transition))
            .unwrap_or_default()
        {
            let mapped = call
                .maps
                .iter()
                .find(|(from, _)| from == field)
                .map(|(_, to)| to.as_str());
            let parameter = match mapped {
                Some(parameter) => parameter,
                None if call.passes => field,
                None => continue,
            };
            let id = self
                .service_parameter(&call.service, direction, parameter)
                .unwrap_or_else(|| parameter_id(&format!("service:{}", call.service), direction, parameter));
            if !targets.contains(&id) {
                targets.push(id);
            }
        }
        if targets.is_empty() {
            return vec![slot(written.clone(), 0)];
        }
        targets
            .into_iter()
            .enumerate()
            .map(|(at, to_id)| slot(to_id, at))
            .collect()
    }

    /// `bindings` resolved: each into a transition by its path made again from its first slot
    /// ([`Self::path_bindings`]), its later slots dropped; each other one by [`Self::resolve_binding`].
    #[must_use]
    pub fn resolve_bindings(&self, bindings: Vec<BindingRow>) -> Vec<BindingRow> {
        let mut out = Vec::with_capacity(bindings.len());
        for mut binding in bindings {
            match path_seed(&binding) {
                Some((_, 0)) => out.extend(self.path_bindings(&binding)),
                Some(_) => {}
                None => {
                    self.resolve_binding(&mut binding);
                    out.push(binding);
                }
            }
        }
        out
    }

    /// The services and forms whose expansion may differ now that the artifacts `changed` (entities, services and forms
    /// by id, as declared) changed: those that read one of them, and those that read one of those, in turn.
    #[must_use]
    pub fn dependents(&self, changed: &HashSet<String>) -> HashSet<String> {
        let mut readers: HashMap<String, Vec<&str>> = HashMap::new();
        for id in self.owners.keys() {
            if self.owner(id).is_none_or(|owner| owner.kind == "transition") {
                continue;
            }
            for source in self.sources_of(id) {
                readers.entry(source).or_default().push(id.as_str());
            }
        }
        let mut found: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = changed.iter().cloned().collect();
        while let Some(id) = frontier.pop() {
            for reader in readers.get(&id).into_iter().flatten() {
                if found.insert((*reader).to_string()) {
                    frontier.push((*reader).to_string());
                }
            }
        }
        found
    }

    /// The ids of the entities, services and forms the expansion of `owner` reads.
    fn sources_of(&self, owner: &str) -> Vec<String> {
        let entity = |name: &str| format!("entity:{}", self.entity(name).unwrap_or_else(|| name.to_string()));
        let screen = form_screen(owner);
        let mut sources = Vec::new();
        for mark in self.marks_of(owner) {
            match mark {
                Mark::AutoParameters { entity: named, .. } => {
                    if let Some(name) = named.or_else(|| owner.rsplit_once('#').map(|(_, noun)| noun)) {
                        sources.push(entity(name));
                    }
                }
                Mark::Implements { service, .. } => {
                    sources.push(format!("service:{service}"));
                    // an entity-auto name's parameters are its entity's fields
                    if let Some((_, noun)) = entity_auto(service) {
                        sources.push(entity(noun));
                    }
                }
                Mark::AutoFieldsEntity { entity: named, .. } => sources.push(entity(named)),
                Mark::AutoFieldsService { service, .. } => {
                    sources.push(format!("service:{service}"));
                    if let Some((_, noun)) = entity_auto(service) {
                        sources.push(entity(noun));
                    }
                }
                Mark::Extends(extends) => {
                    if let Some((location, name)) = extended_form(extends, screen) {
                        sources.push(format!("form:{}#{name}", self.screen(&location).unwrap_or(location)));
                    }
                }
                _ => {}
            }
        }
        sources
    }
}
