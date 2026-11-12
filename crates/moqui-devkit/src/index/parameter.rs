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

//! The parameters of the artifacts that have them (plans/design-dev-aide.md 7.1): a service, a screen, a transition and
//! a REST method. Each owner gives its parameters as [`ParamSpec`]s (a direction, a name, a type, whether it is
//! required and a default), and [`parameter_row`] makes the one id, name and summary form of a parameter artifact for
//! every owner kind, read from a file, expanded or derived.

use super::extract::{short_of, ArtifactRow};

/// Which way a parameter goes: into its owner, out of it, or in the owner's path (a transition's `path-parameter`, a
/// REST route's variable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamDirection {
    In,
    Out,
    Path,
}

impl ParamDirection {
    /// The direction as its id has it: `in`, `out` or `path`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::Path => "path",
        }
    }

    /// The direction `text` names, none for any other text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "in" => Some(Self::In),
            "out" => Some(Self::Out),
            "path" => Some(Self::Path),
            _ => None,
        }
    }
}

/// One parameter an owner gives: its direction, its name (a nested one by its dotted path, `order.items`), its type,
/// `required` (`true`, `false`, or `disabled` for a service parameter an implementing service turns off) and its
/// default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub direction: ParamDirection,
    pub name: String,
    pub kind: String,
    pub required: String,
    pub default: Option<String>,
}

impl ParamSpec {
    /// A parameter with no default.
    #[must_use]
    pub fn new(direction: ParamDirection, name: &str, kind: &str, required: &str) -> Self {
        Self {
            direction,
            name: name.to_string(),
            kind: kind.to_string(),
            required: required.to_string(),
            default: None,
        }
    }

    /// This parameter with the default `default`.
    #[must_use]
    pub fn with_default(mut self, default: Option<String>) -> Self {
        self.default = default;
        self
    }

    /// Its summary as its row has it: `in Long, required=true, default 1`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut summary = format!("{} {}, required={}", self.direction.as_str(), self.kind, self.required);
        if let Some(default) = &self.default {
            summary.push_str(&format!(", default {default}"));
        }
        summary
    }
}

/// Where an owner's parameter rows are: the owner's component and file, the line and column of the element that
/// declares them, and their origin (empty for one a file declares, else [`super::extract::ORIGIN_DERIVED`] or an
/// expanded origin).
#[derive(Clone, Copy, Debug)]
pub struct ParamPlace<'a> {
    pub component: &'a str,
    pub path: &'a str,
    pub line: u32,
    pub column: u32,
    pub origin: &'a str,
}

/// The artifact of the parameter `spec` of the artifact `owner`: its id ([`parameter_id`]), name ([`parameter_name`]),
/// summary ([`ParamSpec::summary`]), and its parent, the owner, or for a nested one the parameter it is in.
#[must_use]
pub fn parameter_row(owner: &str, spec: &ParamSpec, place: &ParamPlace<'_>) -> ArtifactRow {
    let direction = spec.direction.as_str();
    let name = parameter_name(owner, direction, &spec.name);
    let parent = match spec.name.rsplit_once('.') {
        Some((up, _)) => parameter_id(owner, direction, up),
        None => owner.to_string(),
    };
    ArtifactRow {
        id: parameter_id(owner, direction, &spec.name),
        kind: "parameter".to_string(),
        short_name: short_of("parameter", &name),
        name,
        component: place.component.to_string(),
        path: place.path.to_string(),
        line: place.line,
        column: place.column,
        summary: spec.summary(),
        parent: Some(parent),
        origin: place.origin.to_string(),
        marks: String::new(),
    }
}

/// The artifacts of the parameters `specs` of `owner`, all at `place` ([`parameter_row`]).
#[must_use]
pub fn parameter_rows<'a>(
    owner: &str,
    specs: impl IntoIterator<Item = &'a ParamSpec>,
    place: &ParamPlace<'_>,
) -> Vec<ArtifactRow> {
    specs
        .into_iter()
        .map(|spec| parameter_row(owner, spec, place))
        .collect()
}

/// The id of the parameter `name` (a nested one by its dotted path) of the artifact `owner`, `direction` `in`, `out`
/// or `path`: `parameter:<owner>@<direction>.<name>`. [`parse_parameter_id`] reads it back.
#[must_use]
pub fn parameter_id(owner: &str, direction: &str, name: &str) -> String {
    format!("parameter:{owner}@{direction}.{name}")
}

/// The name of a parameter artifact: its owner's name, the direction and its own (`a.B.save#Order@in.quantity`), so a
/// find of a bare name such as `orderId` matches it by its short name (the last part) only, after an entity's field.
#[must_use]
pub fn parameter_name(owner: &str, direction: &str, name: &str) -> String {
    let owner = owner.split_once(':').map_or(owner, |(_, name)| name);
    format!("{owner}@{direction}.{name}")
}

/// The owner id, direction and name of a [`parameter_id`]: the text after the last `@` (a parameter's name has none)
/// is the direction, a dot, and the name. Owners may include a method suffix (`:post`, `:any`).
#[must_use]
pub fn parse_parameter_id(id: &str) -> Option<(&str, &str, &str)> {
    let (owner, rest) = id.strip_prefix("parameter:")?.rsplit_once('@')?;
    let (direction, name) = rest.split_once('.')?;
    (ParamDirection::parse(direction).is_some() && !owner.is_empty() && !name.is_empty())
        .then_some((owner, direction, name))
}

#[cfg(test)]
#[path = "parameter_owner_tests.rs"]
mod tests;
