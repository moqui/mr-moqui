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
//! (plans/archive/design-dev-aide.md 7.1), as Moqui fills it in:
//! - a service's parameters from its `implements` (the implemented services' first, `required` set when the element
//!   says, as `moqui_core::service::define`'s `implemented`) and its `auto-parameters` (the entity's fields by
//!   `include` and `exclude`, as `moqui_core::service::validate::auto_fields`), merged with the parameters it declares
//!   in the order written (`expand_params`): a field into a parameter written before it, a parameter written after
//!   into the field;
//! - a view entity's aliases: each `alias-all`'s, one for each field of its member, and the primary key and type of
//!   each alias from the member field it reads, as `moqui_db::model::view` completes the view;
//! - a form's fields from its `extends` (a `DbForm` left out), `auto-fields-entity` and `auto-fields-service` (an
//!   entity-auto name the noun's fields), its own fields winning, as `moqui_core::screen::form`'s `assemble_node`,
//!   `add_entity_fields` and `add_service_fields`: no `lastUpdatedStamp`, and a form-list leaves out the very long text
//!   types; each with its binding into the services the form's transition calls;
//! - a form field's binding into a transition by its path, to each service the transition reached passes the request
//!   to, by the transition's `field-map`s;
//! - a transition's parameters from the service its own `service-call` calls: its out-parameters, and its
//!   in-parameters when the call passes the request, the transition's own parameters winning; and a REST method's from
//!   the service it runs, which takes the request, its path parameters bound into the service's of their names.
//!
//! The rows are made from the declarations alone (each service's, form's and entity's own artifacts and the marks of
//! their summaries, [`super::super::extract::ArtifactRow::marks`]), never from rows expanded before, so the order they are made
//! in does not matter, and a cycle of `implements` or `extends` ends.

use std::collections::{HashMap, HashSet};

use super::{entity_auto, AutoField, Method, Resolver, FIND_DEPTH};
use crate::index::extract::{
    extended_form, extended_location, parameter_id, parse_parameter_id, short_of, split_marks, ArtifactRow, BindingRow,
    ORIGIN_ALIAS_ALL, ORIGIN_SERVICE_CALL, PK_MARK, SCREEN_PATH_OWNER,
};
use crate::index::parameter::{parameter_row, ParamDirection, ParamPlace, ParamSpec};

/// One definition of a service, form, transition, REST method or view entity whose marks say what it declares, and
/// where it is.
#[derive(Clone, Debug)]
pub(super) struct Owner {
    pub(super) kind: String,
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
    pub(super) direction: String,
    pub(super) name: String,
    kind: String,
    required: String,
    default: Option<String>,
    /// The attributes it sets (`type`, `required`, `default`) of a parameter of its name it merges into; all of them
    /// when none is said (a derived entity-auto service's).
    given: Option<Vec<String>>,
    /// Its summary as its row has it, which [`Expansion::summaries`] writes again when the merge changes it.
    stored: String,
    /// Its line and column: a name declared twice (before and after an `auto-parameters`) is two declarations, merged
    /// in order.
    line: u32,
    column: u32,
}

/// A field a form declares, in the file `path`.
#[derive(Clone, Debug)]
pub(super) struct DeclaredField {
    path: String,
    pub(super) name: String,
    widget: Widget,
}

/// An `alias` a view entity declares, in the file `path`, by its mark ([`Mark::Alias`]).
#[derive(Clone, Debug)]
pub(super) struct DeclaredAlias {
    path: String,
    line: u32,
    pub(super) name: String,
    /// The member it reads; none for a computed alias.
    pub(super) entity_alias: Option<String>,
    /// The member's field, when not the alias's name.
    pub(super) field: Option<String>,
    /// Its type when written, which wins over the member field's.
    kind: Option<String>,
    /// Its summary as its row has it, which [`Expansion::summaries`] writes again when the key and type differ.
    stored: String,
}

/// An alias of a view entity as the view has it ([`Resolver::view_fields`]).
#[derive(Clone, Debug)]
pub(super) struct ViewField {
    pub(super) field: AutoField,
    /// An `alias-all` made it; else the view declares it.
    pub(super) from_alias_all: bool,
    /// The member field it reads (`field:<member entity>.<field>`), when the index knows it.
    pub(super) from: Option<String>,
    /// Its name is not the member field's as such: an alias with a `field`, or a prefixed `alias-all`'s.
    pub(super) by_other_name: bool,
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
pub(super) enum Mark<'a> {
    AutoParameters {
        direction: &'a str,
        entity: Option<&'a str>,
        include: &'a str,
        required: bool,
        excludes: Vec<&'a str>,
        /// The parameters written before it: its fields merge into them, as Moqui merges in the order written.
        before: Vec<&'a str>,
    },
    /// A view entity's `member-entity`.
    Member {
        alias: &'a str,
        entity: &'a str,
    },
    /// A view entity's `member-relationship`: the related entity of `relationship` of the member `from`.
    MemberRel {
        alias: &'a str,
        from: &'a str,
        relationship: &'a str,
    },
    /// A view entity's `alias-all`.
    AliasAll {
        alias: &'a str,
        prefix: Option<&'a str>,
        excludes: Vec<&'a str>,
    },
    /// A view entity's `alias`: the member it reads (none for a computed one), its field when not its name, its type
    /// when written.
    Alias {
        entity_alias: Option<&'a str>,
        field: Option<&'a str>,
        kind: Option<&'a str>,
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
    /// A transition's own `service-call`, whose parameters are the transition's.
    ServiceCall(&'a str),
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

/// Whether an artifact of `kind` may declare something to expand: a service, form, transition, REST method or view
/// entity, by its marks.
fn is_owner_kind(kind: &str) -> bool {
    matches!(kind, "service" | "form" | "transition" | "rest" | "view-entity")
}

fn parse_mark(text: &str) -> Option<Mark<'_>> {
    let words: Vec<&str> = text.split(' ').collect();
    fn given(word: &str) -> Option<&str> {
        (word != "-").then_some(word)
    }
    Some(match words.as_slice() {
        ["auto-parameters", direction, entity, include, required, excludes, before] => Mark::AutoParameters {
            direction,
            entity: given(entity),
            include,
            required: *required == "true",
            excludes: list(excludes),
            before: list(before),
        },
        ["member", alias, entity] => Mark::Member { alias, entity },
        ["member-rel", alias, from, relationship] => Mark::MemberRel {
            alias,
            from,
            relationship,
        },
        ["alias-all", alias, prefix, excludes] => Mark::AliasAll {
            alias,
            prefix: given(prefix),
            excludes: list(excludes),
        },
        ["alias", entity_alias, field, kind] => Mark::Alias {
            entity_alias: given(entity_alias),
            field: given(field),
            kind: given(kind),
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
        ["service-call", service] => Mark::ServiceCall(service),
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
pub(super) struct FullParam {
    pub(super) name: String,
    kind: String,
    /// `true`, `false` or `disabled`.
    pub(super) required: String,
    default: Option<String>,
    origin: &'static str,
    /// For a declared one, its summary as its row has it.
    stored: Option<String>,
    /// The artifacts it was merged from: an entity field (`field:`), an implemented service's parameter, and the
    /// parameter as declared, for a rename's family ([`Resolver::provenance`]).
    pub(super) from: Vec<String>,
}

impl FullParam {
    /// It as a parameter of `direction` of an owner ([`parameter_row`]).
    fn spec(&self, direction: ParamDirection) -> ParamSpec {
        ParamSpec::new(direction, &self.name, &self.kind, &self.required).with_default(self.default.clone())
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
        // a declared one stays declared, whichever merges into the other (an auto field into a parameter written
        // before its auto-parameters); else it is of the last to merge
        if !self.origin.is_empty() {
            self.origin = other.origin;
        }
        if other.stored.is_some() {
            self.stored = other.stored;
        }
        self.from.extend(other.from);
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
pub(super) struct FullField {
    pub(super) name: String,
    summary: String,
    /// Empty when the form declares it.
    origin: &'static str,
    /// What its widget comes from: its own origin, or an inherited field's when the form declares it with no widget.
    source: &'static str,
    /// The user fills it.
    filled: bool,
    /// The expansion makes its binding: an inherited field the user fills, or a declared one with no widget over one.
    bind: bool,
    /// The artifacts it comes from: the extended form's field, an entity field, a service's parameter, and the field as
    /// declared ([`Resolver::provenance`]).
    pub(super) from: Vec<String>,
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
        line: 0,
        column: 0,
    }
}

impl Resolver {
    /// Know what `artifact` declares for the expansions: an owner's marks, a service's parameter, a form's field. Each
    /// is kept by its file, so a second definition of an id (an override) and the removal of either keep the other.
    pub(super) fn add_declared(&mut self, artifact: &ArtifactRow) {
        if is_owner_kind(&artifact.kind) {
            let marks = split_marks(&artifact.marks);
            if !marks.is_empty() {
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
        }
        match artifact.kind.as_str() {
            "field" => {
                let (
                    Some(view),
                    Some(Mark::Alias {
                        entity_alias,
                        field,
                        kind,
                    }),
                ) = (
                    artifact.parent.as_deref().and_then(|id| id.strip_prefix("entity:")),
                    split_marks(&artifact.marks).into_iter().find_map(parse_mark),
                )
                else {
                    return;
                };
                let alias = DeclaredAlias {
                    path: artifact.path.clone(),
                    line: artifact.line,
                    name: artifact.short_name.clone(),
                    entity_alias: entity_alias.map(str::to_string),
                    field: field.map(str::to_string),
                    kind: kind.map(str::to_string),
                    stored: artifact.summary.clone(),
                };
                let aliases = self.aliases.entry(view.to_string()).or_default();
                aliases.retain(|known| !(known.path == alias.path && known.name == alias.name));
                aliases.push(alias);
            }
            "parameter" => {
                let Some((owner, direction, name)) = parse_parameter_id(&artifact.id) else {
                    return;
                };
                // a service's in and out parameters, which the expansions merge; a transition's and a REST method's own,
                // which win over the ones its service gives (a REST method's path parameters are bound into it)
                let kept = if owner.starts_with("service:") {
                    matches!(direction, "in" | "out")
                } else {
                    owner.starts_with("transition:") || owner.starts_with("rest:")
                };
                if !kept || name.contains('.') {
                    return;
                }
                let given = split_marks(&artifact.marks)
                    .into_iter()
                    .find_map(|mark| match parse_mark(mark) {
                        Some(Mark::Given(given)) => Some(given.into_iter().map(str::to_string).collect()),
                        _ => None,
                    });
                let mut param = declared_param(&artifact.path, direction, name, &artifact.summary, given);
                param.line = artifact.line;
                param.column = artifact.column;
                let params = self.params.entry(owner.to_string()).or_default();
                params.retain(|known| {
                    !(known.path == param.path
                        && known.direction == direction
                        && known.name == name
                        && known.line == param.line
                        && known.column == param.column)
                });
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
        if is_owner_kind(&artifact.kind) {
            super::remove_one(&mut self.owners, &artifact.id, |owner| owner.path == artifact.path);
        }
        match artifact.kind.as_str() {
            "entity" | "view-entity" => {
                super::remove_one(&mut self.entity_paths, &artifact.name, |path| *path == artifact.path);
            }
            "field" => {
                if let Some(view) = artifact.parent.as_deref().and_then(|id| id.strip_prefix("entity:")) {
                    super::remove_one(&mut self.aliases, view, |known| {
                        known.path == artifact.path && known.name == artifact.short_name
                    });
                }
            }
            "parameter" => {
                if let Some((owner, direction, name)) = parse_parameter_id(&artifact.id) {
                    super::remove_one(&mut self.params, owner, |param| {
                        param.path == artifact.path
                            && param.direction == direction
                            && param.name == name
                            && param.line == artifact.line
                            && param.column == artifact.column
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
    pub(super) fn owner(&self, id: &str) -> Option<&Owner> {
        self.owners.get(id)?.iter().max_by(|a, b| a.path.cmp(&b.path))
    }

    pub(super) fn marks_of(&self, id: &str) -> Vec<Mark<'_>> {
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

    /// Note that the row of the declared parameter or view alias `id` now has `summary` ([`Expansion::summaries`]
    /// written).
    pub fn set_stored_summary(&mut self, id: &str, summary: &str) {
        if let Some((view, name)) = id.strip_prefix("field:").and_then(|field| field.rsplit_once('.')) {
            for alias in self.aliases.get_mut(view).into_iter().flatten() {
                if alias.name == name {
                    alias.stored = summary.to_string();
                }
            }
            return;
        }
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
    /// by file and line, however the rows were read. A view entity's are its aliases ([`Self::view_fields`]).
    pub(super) fn ordered_fields(&self, full: &str) -> Vec<AutoField> {
        self.fields_at(full, 0)
    }

    /// [`Self::ordered_fields`] `depth` views deep (a view of views), so a cycle ends.
    fn fields_at(&self, full: &str, depth: usize) -> Vec<AutoField> {
        let view = format!("entity:{full}");
        if self.owner(&view).is_some_and(|owner| owner.kind == "view-entity") {
            return self
                .view_fields(&view, depth)
                .into_iter()
                .map(|view_field| view_field.field)
                .collect();
        }
        let own = self.entity_paths.get(full);
        let mut fields: Vec<AutoField> = self.fields.get(full).into_iter().flatten().cloned().collect();
        fields.sort_by(|a, b| {
            let extension = |field: &AutoField| !own.is_some_and(|paths| paths.contains(&field.path));
            (extension(a), &a.path, a.line).cmp(&(extension(b), &b.path, b.line))
        });
        fields
    }

    /// The members of the view entity whose marks are `marks`, in the order written, each with its entity's full name:
    /// a `member-entity`'s, and a `member-relationship`'s related entity of the relationship of the member it joins
    /// from, as moqui-db's `resolve_members` takes it; none for one the index cannot follow.
    pub(super) fn view_members<'a>(&self, marks: &[Mark<'a>]) -> Vec<(&'a str, Option<String>)> {
        let mut members: Vec<(&'a str, Option<String>)> = Vec::new();
        for mark in marks {
            match mark {
                Mark::Member { alias, entity } => {
                    members.push((
                        alias,
                        Some(self.entity(entity).unwrap_or_else(|| (*entity).to_string())),
                    ));
                }
                Mark::MemberRel {
                    alias,
                    from,
                    relationship,
                } => {
                    let from_entity = members
                        .iter()
                        .find(|(known, _)| known == from)
                        .and_then(|(_, entity)| entity.clone());
                    let entity = from_entity.and_then(|from| self.relationship(&from, relationship));
                    members.push((alias, entity));
                }
                _ => {}
            }
        }
        members
    }

    /// The aliases of the view entity `view` (its id) as Moqui completes the view (`moqui_db::model::view`): the
    /// declared ones in the order written, each with the primary key and type of the member field it reads (its own
    /// type when it has one), then each `alias-all`'s ([`ViewField`]). A member the index does not know gives its
    /// aliases no key and type, and its `alias-all` none.
    pub(super) fn view_fields(&self, view: &str, depth: usize) -> Vec<ViewField> {
        let (Some(owner), Some(full)) = (self.owner(view), view.strip_prefix("entity:")) else {
            return Vec::new();
        };
        let marks: Vec<Mark<'_>> = owner.marks.iter().filter_map(|mark| parse_mark(mark)).collect();
        // each member's entity and fields
        let mut members: HashMap<&str, (String, Vec<AutoField>)> = HashMap::new();
        for (alias, entity) in self.view_members(&marks) {
            let Some(member) = entity else {
                continue;
            };
            let fields = if depth < FIND_DEPTH && member != full {
                self.fields_at(&member, depth + 1)
            } else {
                Vec::new()
            };
            members.insert(alias, (member, fields));
        }
        let mut declared: Vec<&DeclaredAlias> = self
            .aliases
            .get(full)
            .into_iter()
            .flatten()
            .filter(|alias| alias.path == owner.path)
            .collect();
        declared.sort_by_key(|alias| alias.line);
        let mut out: Vec<ViewField> = Vec::new();
        for alias in declared {
            let source = alias.entity_alias.as_deref().and_then(|member| {
                let field = alias.field.as_deref().unwrap_or(&alias.name);
                let (entity, fields) = members.get(member)?;
                let found = fields.iter().find(|known| known.name == field)?;
                Some((format!("field:{entity}.{field}"), found))
            });
            out.push(ViewField {
                field: AutoField {
                    name: alias.name.clone(),
                    kind: alias
                        .kind
                        .clone()
                        .or_else(|| source.as_ref().map(|(_, field)| field.kind.clone()))
                        .unwrap_or_default(),
                    pk: source.as_ref().is_some_and(|(_, field)| field.pk),
                    path: alias.path.clone(),
                    line: alias.line,
                },
                from_alias_all: false,
                from: source.map(|(id, _)| id),
                by_other_name: alias.field.is_some(),
            });
        }
        let mut used: HashSet<String> = out.iter().map(|view_field| view_field.field.name.clone()).collect();
        for mark in &marks {
            let Mark::AliasAll {
                alias,
                prefix,
                excludes,
            } = mark
            else {
                continue;
            };
            let Some((entity, fields)) = members.get(alias) else {
                continue;
            };
            for field in fields {
                if field.name == "lastUpdatedStamp" || excludes.contains(&field.name.as_str()) {
                    continue;
                }
                let name = match prefix {
                    Some(prefix) => moqui_db::model::prefixed_name(prefix, &field.name),
                    None => field.name.clone(),
                };
                if !used.insert(name.clone()) {
                    continue;
                }
                out.push(ViewField {
                    field: AutoField {
                        name,
                        kind: field.kind.clone(),
                        pk: field.pk,
                        path: owner.path.clone(),
                        line: owner.line,
                    },
                    from_alias_all: true,
                    from: Some(format!("field:{entity}.{}", field.name)),
                    by_other_name: prefix.is_some(),
                });
            }
        }
        out
    }

    /// Whether the service, form, transition, REST method or view entity `id` declares something to expand.
    #[must_use]
    pub fn is_expandable(&self, id: &str) -> bool {
        self.marks_of(id).iter().any(|mark| {
            matches!(
                mark,
                Mark::AutoParameters { .. }
                    | Mark::Implements { .. }
                    | Mark::AutoFieldsEntity { .. }
                    | Mark::AutoFieldsService { .. }
                    | Mark::Extends(_)
                    | Mark::ServiceCall(_)
                    | Mark::Member { .. }
                    | Mark::MemberRel { .. }
                    | Mark::AliasAll { .. }
            )
        })
    }

    /// The service the transition's own `service-call` or the REST method's `service` names.
    pub(super) fn called_service<'a>(marks: &[Mark<'a>]) -> Option<&'a str> {
        marks.iter().find_map(|mark| match mark {
            Mark::ServiceCall(service) => Some(*service),
            _ => None,
        })
    }

    /// The parameters the transition `transition` takes from the service its own `service-call` calls
    /// ([`Self::full_params`]): each out-parameter, and when the call passes the request's parameters each
    /// in-parameter the transition declares no parameter of its name for. A `required="disabled"` one is left out. A
    /// REST method (`rest:<path>:<method>`) takes them from the service it runs, which always takes the request's
    /// parameters.
    fn transition_params(&self, transition: &str, place: &ParamPlace<'_>) -> Vec<ArtifactRow> {
        let marks = self.marks_of(transition);
        let Some(service) = Self::called_service(&marks) else {
            return Vec::new();
        };
        let passes = transition.starts_with("rest:")
            || marks
                .iter()
                .any(|mark| matches!(mark, Mark::Call { service: called, passes: true, .. } if *called == service));
        let declared: HashSet<&str> = self
            .params
            .get(transition)
            .into_iter()
            .flatten()
            .map(|param| param.name.as_str())
            .collect();
        let target = format!("service:{service}");
        let noun = service.rsplit_once('#').map(|(_, noun)| noun);
        let mut rows = Vec::new();
        for direction in [ParamDirection::In, ParamDirection::Out] {
            if direction == ParamDirection::In && !passes {
                continue;
            }
            let mut visited = HashSet::from([transition.to_string(), target.clone()]);
            for param in self.full_params(&target, direction.as_str(), &mut visited, 0, noun) {
                if param.required == "disabled"
                    || (direction == ParamDirection::In && declared.contains(param.name.as_str()))
                {
                    continue;
                }
                rows.push(parameter_row(transition, &param.spec(direction), place));
            }
        }
        rows
    }

    /// The bindings of the REST method `method`'s path parameters into the service it runs: each one the service takes
    /// as an in-parameter of its name, as the REST API passes the path's values to it.
    fn rest_bindings(&self, method: &str, line: u32) -> Vec<BindingRow> {
        let marks = self.marks_of(method);
        let Some(service) = Self::called_service(&marks) else {
            return Vec::new();
        };
        let target = format!("service:{service}");
        let noun = service.rsplit_once('#').map(|(_, noun)| noun);
        let mut visited = HashSet::from([method.to_string(), target.clone()]);
        let taken: HashSet<String> = self
            .full_params(&target, "in", &mut visited, 0, noun)
            .into_iter()
            .filter(|param| param.required != "disabled")
            .map(|param| param.name)
            .collect();
        let mut paths: Vec<&Param> = self
            .params
            .get(method)
            .into_iter()
            .flatten()
            .filter(|param| param.direction == "path" && taken.contains(&param.name))
            .collect();
        paths.sort_by(|a, b| a.name.cmp(&b.name));
        paths.dedup_by(|a, b| a.name == b.name);
        paths
            .into_iter()
            .map(|param| BindingRow {
                from: method.to_string(),
                to_id: parameter_id(&target, "in", &param.name),
                direction: "in".to_string(),
                source_kind: "parameter".to_string(),
                source_text: param.name.clone(),
                source_id: Some(parameter_id(method, "path", &param.name)),
                to_path: None,
                origin: ORIGIN_SERVICE_CALL.to_string(),
                origin_hash: String::new(),
                line,
            })
            .collect()
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
        let place = |origin| ParamPlace {
            component: &info.component,
            path: &info.path,
            line: info.line,
            column: info.column,
            origin,
        };
        let mut expansion = Expansion::default();
        match info.kind.as_str() {
            "service" => {
                let noun = owner.rsplit_once('#').map(|(_, noun)| noun);
                for direction in [ParamDirection::In, ParamDirection::Out] {
                    let mut visited = HashSet::from([owner.to_string()]);
                    for param in self.full_params(owner, direction.as_str(), &mut visited, 0, noun) {
                        let spec = param.spec(direction);
                        if param.origin.is_empty() {
                            let summary = spec.summary();
                            if param.stored.as_ref().is_some_and(|stored| *stored != summary) {
                                expansion
                                    .summaries
                                    .push((parameter_id(owner, direction.as_str(), &param.name), summary));
                            }
                            continue;
                        }
                        expansion
                            .artifacts
                            .push(parameter_row(owner, &spec, &place(param.origin)));
                    }
                }
            }
            "transition" => {
                expansion.artifacts = self.transition_params(owner, &place(ORIGIN_SERVICE_CALL));
            }
            "rest" => {
                expansion.artifacts = self.transition_params(owner, &place(ORIGIN_SERVICE_CALL));
                expansion.bindings = self.rest_bindings(owner, info.line);
            }
            "view-entity" => {
                let Some(full) = owner.strip_prefix("entity:") else {
                    return expansion;
                };
                let declared: HashMap<&str, &str> = self
                    .aliases
                    .get(full)
                    .into_iter()
                    .flatten()
                    .filter(|alias| alias.path == info.path)
                    .map(|alias| (alias.name.as_str(), alias.stored.as_str()))
                    .collect();
                for ViewField {
                    field, from_alias_all, ..
                } in self.view_fields(owner, 0)
                {
                    let summary = if field.pk {
                        format!("{}{PK_MARK}", field.kind)
                    } else {
                        field.kind.clone()
                    };
                    let id = format!("field:{full}.{}", field.name);
                    if from_alias_all {
                        expansion.artifacts.push(row(
                            id,
                            "field",
                            format!("{full}.{}", field.name),
                            summary,
                            ORIGIN_ALIAS_ALL,
                        ));
                    } else if !summary.is_empty() && declared.get(field.name.as_str()) != Some(&summary.as_str()) {
                        // a declared alias's row says the key and type of the field it reads
                        expansion.summaries.push((id, summary));
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
                // by the path the form posts to, a transition of its screen by its name ([`Self::path_bindings`]); of its
                // screen, only a field one of its calls takes, as the file's own fields (extract.rs `form_bindings`)
                let target = self.marks_of(owner).into_iter().find_map(|mark| match mark {
                    Mark::Transition(name) => Some((true, name)),
                    Mark::TransitionPath(path) => Some((false, path)),
                    _ => None,
                });
                // the transition it posts to, as its path is walked: `post`, else any method, else the first
                let same_screen_calls: Vec<FormCall> = match target {
                    Some((true, name)) => self
                        .base_transition(screen, name, Method::Post)
                        .map(|transition| self.calls_of(&transition))
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                for field in fields {
                    let source_id = format!("form-field:{form_key}@{}", field.name);
                    let taken = |name: &str| {
                        same_screen_calls
                            .iter()
                            .any(|call| call.passes || call.maps.iter().any(|(from, _)| from == name))
                    };
                    let path = target
                        .filter(|(same, _)| !same || taken(&field.name))
                        .map(|(_, path)| path);
                    if let Some(path) = path.filter(|_| field.bind) {
                        expansion.bindings.push(BindingRow {
                            from: owner.to_string(),
                            to_id: parameter_id(&format!("{SCREEN_PATH_OWNER}{screen}#{path}"), "in", &field.name),
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
    /// `auto-parameters` and the parameters it declares in the order written, as `moqui_core::service::validate`'s
    /// `expand_params` merges them: an entity field sets the type and `required` of a parameter of its name written
    /// before it, and a parameter written after one merges into it, setting what it gives. An `auto-parameters` with no
    /// entity reads the entity of `noun`, the service's own (an implemented service's its own, as Java expands it there). `visited` holds the services on the way, so a
    /// cycle ends.
    pub(super) fn full_params(
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
        // the names an earlier implemented service gave only by its auto-parameters: the runtime keeps those as the
        // auto element, not a named parameter, so a later implemented service's parameter of the name is not dropped
        // but merged into the field at the expansion (`moqui_core::service::define`'s `implemented`)
        let mut auto_only: HashSet<String> = HashSet::new();
        let auto_given = ["type".to_string(), "required".to_string()];
        for mark in &marks {
            let Mark::Implements {
                service: target,
                required,
            } = mark
            else {
                continue;
            };
            // its auto-parameters with no entity-name read its own noun
            let target_noun = target.rsplit_once('#').map(|(_, noun)| noun);
            let target = format!("service:{target}");
            if !visited.insert(target.clone()) {
                continue;
            }
            let named_before: HashSet<String> = out
                .iter()
                .filter(|known| !auto_only.contains(&known.name))
                .map(|known| known.name.clone())
                .collect();
            let auto_before = auto_only.clone();
            for mut param in self.full_params(&target, direction, visited, depth + 1, target_noun) {
                if param.required == "disabled" || named_before.contains(&param.name) {
                    continue;
                }
                let from_auto = param.origin == "auto-parameters";
                if let Some(required) = required {
                    param.required = required.to_string();
                }
                param.origin = "implements";
                param.stored = None;
                param.from = vec![parameter_id(&target, direction, &param.name)];
                if auto_before.contains(&param.name) {
                    if !from_auto {
                        auto_only.remove(&param.name);
                    }
                    let given = from_auto.then_some(&auto_given[..]);
                    merge_into(&mut out, param, given);
                    continue;
                }
                if from_auto {
                    auto_only.insert(param.name.clone());
                }
                out.push(param);
            }
        }
        let mut declared = self.declared_params(service, direction);
        // in the order written: a name declared before and after an auto-parameters is two merges
        declared.sort_by_key(|param| (param.line, param.column));
        let declared_full = |param: &Param| FullParam {
            name: param.name.clone(),
            kind: param.kind.clone(),
            required: param.required.clone(),
            default: param.default.clone(),
            origin: "",
            stored: Some(param.stored.clone()),
            from: vec![parameter_id(service, direction, &param.name)],
        };
        // the declarations merged so far, by their place in `declared`: of a name an auto-parameters' `before` lists n
        // times, the first n declarations go before it
        let mut merged: Vec<bool> = vec![false; declared.len()];
        for mark in &marks {
            let Mark::AutoParameters {
                direction: of,
                entity,
                include,
                required,
                excludes,
                before,
            } = mark
            else {
                continue;
            };
            if *of != direction {
                continue;
            }
            for (at, param) in declared.iter().enumerate() {
                let written_before = before.iter().filter(|name| **name == param.name).count();
                let earlier = declared[..at].iter().filter(|known| known.name == param.name).count();
                if !merged[at] && earlier < written_before {
                    merged[at] = true;
                    merge_into(&mut out, declared_full(param), param.given.as_deref());
                }
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
                    from: vec![format!("field:{full}.{}", field.name)],
                };
                merge_into(&mut out, param, Some(&auto_given));
            }
        }
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
                            from: vec![parameter_id(&id, direction, &param.name)],
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
                                from: vec![row.id.clone()],
                            }
                        })
                    })
                    .collect();
            }
        }
        for (at, param) in declared.iter().enumerate() {
            if !merged[at] {
                merge_into(&mut out, declared_full(param), param.given.as_deref());
            }
        }
        out
    }

    /// The fields the form `form` has: those of the form it extends (with the fields a `screen-extend` adds to that
    /// form), then those of its `auto-fields-entity` and `auto-fields-service`, then the ones it declares, each in
    /// place of one of its name (a declared one with no widget takes that one's widget).
    pub(super) fn full_fields(&self, form: &str, visited: &mut HashSet<String>, depth: usize) -> Vec<FullField> {
        let mut out: Vec<FullField> = Vec::new();
        if depth > FIND_DEPTH {
            return out;
        }
        let screen = form_screen(form);
        let form_list = self.owner(form).is_some_and(|owner| owner.form_list);
        let inherit = |out: &mut Vec<FullField>,
                       name: &str,
                       summary: String,
                       origin: &'static str,
                       filled: bool,
                       from: String| {
            if name != "lastUpdatedStamp" && !out.iter().any(|known| known.name == name) {
                out.push(FullField {
                    name: name.to_string(),
                    summary,
                    origin,
                    source: origin,
                    filled,
                    bind: filled,
                    from: vec![from],
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
                    let base_key = base.strip_prefix("form:").unwrap_or(&base).to_string();
                    let base_field = |name: &str| format!("form-field:{base_key}@{name}");
                    for field in self.full_fields(&base, visited, depth + 1) {
                        let from = base_field(&field.name);
                        inherit(&mut out, &field.name, summary.clone(), "extends", field.filled, from);
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
                                base_field(&field.name),
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
                            format!("field:{full}.{}", field.name),
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
                                    let from = parameter_id(&id, direction, &param.name);
                                    inherit(
                                        &mut out,
                                        &param.name,
                                        summary.clone(),
                                        "auto-fields-service",
                                        true,
                                        from,
                                    );
                                }
                            }
                        }
                    } else if let Some(full) = entity_auto(service).and_then(|(_, noun)| self.entity(noun)) {
                        for field in self.ordered_fields(&full) {
                            if form_list && LONG_TEXT.contains(&field.kind.as_str()) {
                                continue;
                            }
                            let from = format!("field:{full}.{}", field.name);
                            inherit(
                                &mut out,
                                &field.name,
                                summary.clone(),
                                "auto-fields-service",
                                true,
                                from,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        let own = |name: &str| format!("form-field:{}@{name}", form.strip_prefix("form:").unwrap_or(form));
        for declared in self.form_fields.get(form).into_iter().flatten() {
            match out.iter_mut().find(|known| known.name == declared.name) {
                Some(known) => {
                    known.origin = "";
                    known.from.push(own(&declared.name));
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
                    from: vec![own(&declared.name)],
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
            Mark::Transition(path) | Mark::TransitionPath(path) => self
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
            if !self.is_expandable(id) {
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
    pub(super) fn sources_of(&self, owner: &str) -> Vec<String> {
        let entity = |name: &str| format!("entity:{}", self.entity(name).unwrap_or_else(|| name.to_string()));
        let screen = form_screen(owner);
        let mut sources = Vec::new();
        let marks = self.marks_of(owner);
        // a view's members, a member-relationship's by the relationship of the member it joins from
        for (_, member) in self.view_members(&marks) {
            sources.extend(member.map(|full| format!("entity:{full}")));
        }
        for mark in marks {
            match mark {
                Mark::AutoParameters { entity: named, .. } => {
                    if let Some(name) = named.or_else(|| owner.rsplit_once('#').map(|(_, noun)| noun)) {
                        sources.push(entity(name));
                    }
                }
                Mark::Implements { service, .. } | Mark::ServiceCall(service) => {
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
