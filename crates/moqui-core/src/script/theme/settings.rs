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

//! Typed theme settings (design-shop §12.3): the definitions of a schema, the values of a template or
//! `config/settings.json` checked against them, and the values resolved for a render. A default applies only when a
//! value is not given; an explicit empty, false, zero or null is kept. No setting holds an expression.

use std::collections::HashSet;

use serde_json::{json, Map, Value as Json};

use super::package::json_line;
use super::Diagnostic;

/// The setting types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    MultilineText,
    RichText,
    Boolean,
    Number,
    Enum,
    Color,
    Image,
    ResourceLink,
    Product,
    Category,
    Page,
    Source,
}

impl Kind {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "text" => Self::Text,
            "multilineText" => Self::MultilineText,
            "richText" => Self::RichText,
            "boolean" => Self::Boolean,
            "number" => Self::Number,
            "enum" => Self::Enum,
            "color" => Self::Color,
            "image" => Self::Image,
            "resourceLink" => Self::ResourceLink,
            "product" => Self::Product,
            "category" => Self::Category,
            "page" => Self::Page,
            "source" => Self::Source,
            _ => return None,
        })
    }

    fn default_max_length(self) -> usize {
        match self {
            Self::MultilineText => 10_000,
            Self::RichText => 50_000,
            _ => 1_000,
        }
    }

    fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::MultilineText | Self::RichText)
    }
}

const KINDS: &str =
    "text, multilineText, richText, boolean, number, enum, color, image, resourceLink, product, category, page, source";

/// One setting of a schema.
#[derive(Clone, Debug)]
pub(crate) struct SettingDef {
    pub id: String,
    pub kind: Kind,
    pub default: Option<Json>,
    pub required: bool,
    pub max_length: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub options: Vec<String>,
    pub localized: bool,
    pub sources: Vec<String>,
}

fn is_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The definitions of a schema's `settings` list. A bad definition is left out with an error.
pub(crate) fn parse_defs(
    value: Option<&Json>,
    file: &str,
    text: &str,
    has_asset: &dyn Fn(&str) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SettingDef> {
    let items = match value {
        None | Some(Json::Null) => return Vec::new(),
        Some(Json::Array(items)) => items,
        Some(_) => {
            diagnostics.push(Diagnostic::error(
                "invalid-schema",
                file,
                json_line(text, "settings"),
                "settings is a list",
            ));
            return Vec::new();
        }
    };
    let mut seen = HashSet::new();
    let mut defs = Vec::new();
    for item in items {
        let id = item.get("id").and_then(Json::as_str).unwrap_or_default();
        let line = json_line(text, id);
        let error = |message: String| Diagnostic::error("invalid-schema", file, line, message);
        if !is_id(id) {
            diagnostics.push(error(format!("setting id `{id}` is not a name")));
            continue;
        }
        if !seen.insert(id.to_string()) {
            diagnostics.push(Diagnostic::error(
                "duplicate-id",
                file,
                line,
                format!("setting `{id}` is defined twice"),
            ));
            continue;
        }
        let type_name = item.get("type").and_then(Json::as_str).unwrap_or_default();
        let Some(kind) = Kind::parse(type_name) else {
            diagnostics.push(Diagnostic::error(
                "unknown-type",
                file,
                line,
                format!("setting `{id}` has type `{type_name}`; the types are {KINDS}"),
            ));
            continue;
        };
        let number = |name: &str| item.get(name).and_then(Json::as_f64);
        let options: Vec<String> = item
            .get("options")
            .and_then(Json::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        option
                            .as_str()
                            .or_else(|| option.get("value").and_then(Json::as_str))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let def = SettingDef {
            id: id.to_string(),
            kind,
            default: item.get("default").cloned(),
            required: item.get("required").and_then(Json::as_bool).unwrap_or(false),
            max_length: item
                .get("maxLength")
                .and_then(Json::as_u64)
                .and_then(|max| usize::try_from(max).ok())
                .map_or(kind.default_max_length(), |max| max.min(kind.default_max_length() * 10)),
            min: number("min"),
            max: number("max"),
            step: number("step").filter(|step| *step > 0.0),
            options,
            localized: item.get("localized").and_then(Json::as_bool).unwrap_or(false),
            sources: item
                .get("sources")
                .and_then(Json::as_array)
                .map(|sources| sources.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        };
        if kind == Kind::Enum && def.options.is_empty() {
            diagnostics.push(error(format!("enum setting `{id}` needs options")));
            continue;
        }
        if let (Some(min), Some(max)) = (def.min, def.max) {
            if min > max {
                diagnostics.push(error(format!("setting `{id}` has min above max")));
                continue;
            }
        }
        if let Some(default) = &def.default {
            if let Err(message) = check_value(&def, default, has_asset) {
                diagnostics.push(error(format!("the default of `{id}`: {message}")));
                continue;
            }
        }
        defs.push(def);
    }
    defs
}

fn is_hex_color(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn is_reference_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
}

/// Whether `value` is one the setting takes. Null is a value only when the setting is not required.
pub(crate) fn check_value(def: &SettingDef, value: &Json, has_asset: &dyn Fn(&str) -> bool) -> Result<(), String> {
    if value.is_null() {
        return if def.required {
            Err("a value is required".to_string())
        } else {
            Ok(())
        };
    }
    let text_ok = |text: &str| -> Result<(), String> {
        if text.chars().count() > def.max_length {
            return Err(format!("longer than {} characters", def.max_length));
        }
        if def.kind == Kind::Text && text.contains('\n') {
            return Err("a text setting is one line".to_string());
        }
        if def.required && text.trim().is_empty() {
            return Err("a value is required".to_string());
        }
        Ok(())
    };
    match (def.kind, value) {
        (kind, Json::String(text)) if kind.is_text() => text_ok(text),
        (kind, Json::Object(by_locale)) if kind.is_text() && def.localized => {
            if by_locale.is_empty() {
                return Err("a localized value needs at least one locale".to_string());
            }
            for text in by_locale.values() {
                text_ok(text.as_str().ok_or("each locale's value is text")?)?;
            }
            Ok(())
        }
        (Kind::Boolean, Json::Bool(_)) => Ok(()),
        (Kind::Number, Json::Number(number)) => {
            let number = number.as_f64().ok_or("not a number")?;
            if def.min.is_some_and(|min| number < min) || def.max.is_some_and(|max| number > max) {
                return Err(format!(
                    "{number} is outside {}..{}",
                    def.min.map_or_else(String::new, |min| min.to_string()),
                    def.max.map_or_else(String::new, |max| max.to_string())
                ));
            }
            if let Some(step) = def.step {
                let steps = (number - def.min.unwrap_or(0.0)) / step;
                if (steps - steps.round()).abs() > 1e-9 {
                    return Err(format!("{number} is not a step of {step}"));
                }
            }
            Ok(())
        }
        (Kind::Enum, Json::String(text)) if def.options.contains(text) => Ok(()),
        (Kind::Enum, _) => Err(format!("not one of {}", def.options.join(", "))),
        (Kind::Color, Json::String(text)) if is_hex_color(text) => Ok(()),
        (Kind::Color, _) => Err("a color is #rgb, #rgba, #rrggbb or #rrggbbaa".to_string()),
        (Kind::Image, Json::String(text)) => {
            if text.starts_with("assets/") {
                if has_asset(text) {
                    Ok(())
                } else {
                    Err(format!("`{text}` is not an asset of the package"))
                }
            } else if is_reference_id(text) {
                Ok(())
            } else {
                Err("an image is an asset path or a media id".to_string())
            }
        }
        (Kind::ResourceLink, Json::String(text)) => match text.split_once(':') {
            Some((kind, id)) if is_id(kind) && is_reference_id(id) => Ok(()),
            _ => Err("a link is `kind:id` (`page:about`, `product:10000`)".to_string()),
        },
        (Kind::Product | Kind::Category | Kind::Page, Json::String(id)) if is_reference_id(id) => Ok(()),
        (Kind::Source, Json::String(name)) if is_reference_id(name) => {
            if def.sources.is_empty() || def.sources.contains(name) {
                Ok(())
            } else {
                Err(format!("source `{name}` is not one of {}", def.sources.join(", ")))
            }
        }
        _ => Err(format!("not a value of a {:?} setting", def.kind)),
    }
}

/// Check the values of an instance (or the global settings) against the definitions. `context` names the instance for
/// the messages.
pub(crate) fn check_values(
    defs: &[SettingDef],
    values: &Map<String, Json>,
    file: &str,
    text: &str,
    has_asset: &dyn Fn(&str) -> bool,
    context: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let line_of = |key: &str| {
        // The value's line: the first `"key"` after the instance's id, or the first anywhere.
        let start = context.and_then(|id| text.find(&format!("\"{id}\""))).unwrap_or(0);
        text[start..].find(&format!("\"{key}\"")).map_or_else(
            || json_line(text, key),
            |at| crate::script::template::line_of(text, start + at),
        )
    };
    for (key, value) in values {
        let Some(def) = defs.iter().find(|def| &def.id == key) else {
            diagnostics.push(Diagnostic::warning(
                "unknown-setting",
                file,
                line_of(key),
                format!("setting `{key}` is not in the schema and is ignored"),
            ));
            continue;
        };
        if let Err(message) = check_value(def, value, has_asset) {
            diagnostics.push(Diagnostic::error(
                "invalid-setting",
                file,
                line_of(key),
                format!("setting `{key}`: {message}"),
            ));
        }
    }
    for def in defs {
        if def.required && !values.contains_key(&def.id) && def.default.is_none() {
            diagnostics.push(Diagnostic::error(
                "invalid-setting",
                file,
                context.map_or(0, |id| json_line(text, id)),
                format!("setting `{}` is required", def.id),
            ));
        }
    }
}

/// What resolving a value needs from the page being rendered.
pub(crate) trait Resolve {
    /// The view's locale, for a localized text.
    fn locale(&self) -> Option<String>;
    /// A placeholder for sanitized rich text.
    fn rich_text(&mut self, html: &str) -> Json;
    /// An image: a package asset or a media projection.
    fn image(&mut self, value: &str) -> Option<Json>;
    /// A resolved `kind:id` reference.
    fn reference(&mut self, key: &str) -> Option<Json>;
    /// The data of an approved source.
    fn source(&mut self, name: &str) -> Option<Json>;
    fn has_asset(&self, path: &str) -> bool;
}

/// The value of a localized text for `locale`: the locale, its language, `default`, then the first given.
fn localized(by_locale: &Map<String, Json>, locale: Option<&str>) -> String {
    let pick = |key: &str| by_locale.get(key).and_then(Json::as_str).map(str::to_string);
    locale
        .and_then(|locale| {
            pick(locale)
                .or_else(|| pick(&locale.replace('_', "-")))
                .or_else(|| locale.split(['-', '_']).next().and_then(pick))
        })
        .or_else(|| pick("default"))
        .or_else(|| by_locale.values().find_map(Json::as_str).map(str::to_string))
        .unwrap_or_default()
}

/// The settings of a render: each definition's value, given or default, checked and resolved. A value that does not
/// check, or a required reference that does not resolve, is an error; an optional reference that does not resolve is
/// null with a warning.
pub(crate) fn resolve(
    defs: &[SettingDef],
    values: &Map<String, Json>,
    resolver: &mut dyn Resolve,
    warnings: &mut Vec<String>,
) -> Result<Json, String> {
    let mut out = Map::new();
    let has_asset = |path: &str| resolver.has_asset(path);
    let mut checked = Vec::new();
    for def in defs {
        let value = match values.get(&def.id) {
            Some(value) => value.clone(),
            None => def.default.clone().unwrap_or(Json::Null),
        };
        check_value(def, &value, &has_asset).map_err(|message| format!("setting `{}`: {message}", def.id))?;
        checked.push((def, value));
    }
    for (def, value) in checked {
        let resolved = match (&value, def.kind) {
            (Json::Null, _) => Json::Null,
            (Json::Object(by_locale), kind) if kind.is_text() => {
                let text = localized(by_locale, resolver.locale().as_deref());
                if kind == Kind::RichText {
                    resolver.rich_text(&text)
                } else {
                    json!(text)
                }
            }
            (Json::String(html), Kind::RichText) => resolver.rich_text(html),
            (Json::String(reference), Kind::Image) => resolver.image(reference).unwrap_or(Json::Null),
            (Json::String(reference), Kind::ResourceLink) => resolver.reference(reference).unwrap_or(Json::Null),
            (Json::String(id), Kind::Product) => resolver.reference(&format!("product:{id}")).unwrap_or(Json::Null),
            (Json::String(id), Kind::Category) => resolver.reference(&format!("category:{id}")).unwrap_or(Json::Null),
            (Json::String(id), Kind::Page) => resolver.reference(&format!("page:{id}")).unwrap_or(Json::Null),
            (Json::String(name), Kind::Source) => resolver.source(name).unwrap_or(Json::Null),
            (other, _) => other.clone(),
        };
        if resolved.is_null() && !value.is_null() {
            let message = format!("setting `{}`: `{}` did not resolve", def.id, value);
            if def.required {
                return Err(message);
            }
            warnings.push(message);
        }
        out.insert(def.id.clone(), resolved);
    }
    Ok(Json::Object(out))
}
