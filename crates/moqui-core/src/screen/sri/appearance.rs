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

//! The appearance of a Quasar theme (docs/ui/Qvt-Client.md, Appearance): what the `/qapps` shell gets as
//! `config.appearance`, built in one place for the page, the desktop client and the scheme style sheet.
//!
//! - The appearance preferences and the policy group each belongs to ([`APPEARANCE_KEYS`]), and the tokens a scheme sets
//!   ([`BRAND_TOKENS`], [`SURFACE_TOKENS`], [`SHAPE_TOKENS`]).
//! - The style sheet of a color scheme ([`color_scheme_css`]): the `colorSchemeCss` transition serves it.
//! - The theme's user appearance policy (`STRT_APPEARANCE_POLICY`, [`AppearancePolicy`]): what the user may change, the
//!   mode the theme forces and whether the user may choose another theme. Absent allows everything. The server
//!   refuses a locked preference in [`crate::user::set_preference`] and a locked theme choice in
//!   `set#UserScreenTheme`; the page hides its control.
//! - The themes the user may choose (`themes`, the Quasar themes) and the color schemes of a theme: its own, then its
//!   parent's ([`super::theme::theme_chain`]).
//! - The extension point: each `STRT_APPEARANCE_SERVICE` of the theme is called with `userId`, `screenThemeId` and the
//!   `appearance` built so far, and answers `appearance`, so a component decides by station, tenant or host without
//!   framework code. The locked values, the effective scheme and the style sheets the page links are worked out after it.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use super::theme::{theme_chain, theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};
use crate::context::{Error, ExecutionContext};

/// The theme resource that names the theme's default color scheme (a `ScreenColorScheme` id of the theme).
pub(crate) const DEFAULT_COLOR_SCHEME: &str = "STRT_DEFAULT_COLOR_SCHEME";
/// The theme resource of the user appearance policy, JSON.
pub(crate) const APPEARANCE_POLICY: &str = "STRT_APPEARANCE_POLICY";
/// The theme resource that names a service that adjusts the appearance.
pub(crate) const APPEARANCE_SERVICE: &str = "STRT_APPEARANCE_SERVICE";

/// The appearance preferences and the policy group of each. A group the policy sets `false` locks its keys.
pub const APPEARANCE_KEYS: &[(&str, &str)] = &[
    ("QUASAR_DARK", "mode"),
    ("SHELL_DENSITY", "density"),
    ("THEME_COLOR_SCHEME", "scheme"),
    ("THEME_PRIMARY_COLOR", "accent"),
    ("THEME_PRIMARY_COLOR_DARK", "accent"),
    ("THEME_COLOR_OVERRIDES", "colors"),
    ("THEME_FONT_BODY", "fonts"),
    ("THEME_FONT_HEADING", "fonts"),
    ("THEME_FONT_MONO", "fonts"),
    ("THEME_SIZE_SMALL", "sizes"),
    ("THEME_SIZE_LARGE", "sizes"),
    ("THEME_MOTION", "motion"),
    ("THEME_LINE_HEIGHT", "motion"),
    ("THEME_LINKS_UNDERLINE", "motion"),
    ("THEME_RADIUS", "motion"),
];

/// The policy groups, in the order of [`APPEARANCE_KEYS`] and the Appearance dialog.
pub const APPEARANCE_GROUPS: &[&str] = &["mode", "density", "scheme", "accent", "colors", "fonts", "sizes", "motion"];

/// Quasar's brand colors: a scheme sets them as `--q-*`, `!important` over the brand the Quasar config writes.
pub const BRAND_TOKENS: &[&str] = &["primary", "secondary", "accent", "positive", "negative", "info", "warning"];
/// The surface and text colors of `WebrootVue.qvt.css` (`--m-*`), with the readable text of each semantic color (a
/// scheme that darkens or lightens a brand color sets its text too; unset, the style sheet mixes one from the brand).
pub const SURFACE_TOKENS: &[&str] = &[
    "page-bg",
    "panel-bg",
    "header-bg",
    "header-fg",
    "drawer-bg",
    "text",
    "text-muted",
    "link",
    "positive-text",
    "negative-text",
    "warning-text",
    "info-text",
    "secondary-text",
    "accent-text",
    "border",
    "divider",
];
/// The shape tokens of `WebrootVue.qvt.css` (`--m-*`): what a scheme changes beyond its colors without reaching into the
/// shell's markup. Unset, each keeps the style sheet's and Quasar's own look.
pub const SHAPE_TOKENS: &[&str] = &[
    "header-bg-image",
    "header-border",
    "header-shadow",
    "page-bg-image",
    "btn-radius",
    "btn-border",
    "btn-primary-bg",
    "card-radius",
    "card-border",
    "card-shadow",
    "drawer-current-bg",
    "drawer-current-fg",
];

/// True for a preference the appearance policy governs: one of [`APPEARANCE_KEYS`], or any other `THEME_*` key.
pub fn is_appearance_key(key: &str) -> bool {
    key.starts_with("THEME_") || APPEARANCE_KEYS.iter().any(|(name, _)| *name == key)
}

/// A token name a scheme may set; another could end its declaration.
fn is_token_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 41
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

/// A token value a scheme may set: not empty, and nothing that could end its declaration or the sheet.
fn is_token_value(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(|ch| matches!(ch, ';' | '{' | '}' | '<' | '>' | '\\' | '"' | '\'' | '!'))
}

/// The declarations of one mode's tokens: a brand color as `--q-*` `!important`, the rest as `--m-*`; in dark mode
/// `panel-bg` and `page-bg` are also Quasar's `--q-dark` and `--q-dark-page`, and the link gives way to the user's dark
/// accent. A name or value that could end its declaration is logged and left out.
fn token_declarations(owner: &str, colors: &[(String, String)], dark: bool) -> Vec<String> {
    let mut decls = Vec::new();
    for (name, value) in colors {
        let value = value.trim();
        if !is_token_name(name) || !is_token_value(value) {
            tracing::error!("Color scheme {owner} color {name} is not a CSS name and value, left out");
            continue;
        }
        if BRAND_TOKENS.contains(&name.as_str()) {
            decls.push(format!("--q-{name}:{value} !important"));
        } else if dark && name == "link" {
            decls.push(format!("--m-link:var(--m-accent-dark-link, {value})"));
        } else {
            decls.push(format!("--m-{name}:{value}"));
        }
        if dark && name == "panel-bg" {
            decls.push(format!("--q-dark:{value} !important"));
        }
        if dark && name == "page-bg" {
            decls.push(format!("--q-dark-page:{value} !important"));
        }
    }
    decls
}

/// A color scheme as the page and the style sheet read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorScheme {
    pub id: String,
    pub description: String,
    /// The colors (token name, CSS value) of light mode, by name.
    pub light: Vec<(String, String)>,
    pub dark: Vec<(String, String)>,
    /// CSS written after the colors; an administrator's, never a user's.
    pub extra_css: String,
    /// The `ScreenFontOption` the scheme uses for a role (`body`, `heading`, `mono`) when the user has chosen none.
    pub fonts: BTreeMap<String, String>,
}

impl ColorScheme {
    /// The scheme's style sheet: the colors of each mode as tokens, then its extra CSS.
    pub fn css(&self) -> String {
        let light = token_declarations(&self.id, &self.light, false);
        let dark = token_declarations(&self.id, &self.dark, true);
        let mut css = format!("/* color scheme {} */\n", self.id);
        if !light.is_empty() {
            css.push_str(&format!("body:not(.body--dark) {{ {}; }}\n", light.join("; ")));
        }
        if !dark.is_empty() {
            css.push_str(&format!("body.body--dark {{ {}; }}\n", dark.join("; ")));
        }
        if !self.extra_css.is_empty() {
            css.push_str(&self.extra_css);
            css.push('\n');
        }
        css
    }

    /// A short hash of what the page shows of the scheme, so a changed scheme's sheet is fetched again.
    fn version(&self, css: &str) -> String {
        let fonts: Vec<&str> = self.fonts.values().map(String::as_str).collect();
        text_hash(&format!("{css}\u{0}{}", fonts.join(",")))
    }

    fn as_value(&self) -> Value {
        let css = self.css();
        let mode = |colors: &[(String, String)]| {
            Value::Map(colors.iter().map(|(name, value)| (name.clone(), Value::string(value))).collect())
        };
        let mut map = IndexMap::new();
        map.insert("id".to_string(), Value::string(&self.id));
        map.insert("description".to_string(), Value::string(&self.description));
        map.insert("light".to_string(), mode(&self.light));
        map.insert("dark".to_string(), mode(&self.dark));
        let custom = !self.light.is_empty() || !self.dark.is_empty() || !self.extra_css.is_empty();
        map.insert("custom".to_string(), Value::Bool(custom));
        map.insert("version".to_string(), Value::string(self.version(&css)));
        map.insert(
            "fonts".to_string(),
            Value::Map(self.fonts.iter().map(|(role, id)| (role.clone(), Value::string(id))).collect()),
        );
        Value::Map(map)
    }
}

/// djb2 with xor, in base 36, as the page's own hashes.
fn text_hash(text: &str) -> String {
    let mut hash: u32 = 5381;
    for unit in text.encode_utf16() {
        hash = hash.wrapping_mul(33) ^ u32::from(unit);
    }
    let mut digits = Vec::new();
    loop {
        digits.push(std::char::from_digit(hash % 36, 36).unwrap_or('0'));
        hash /= 36;
        if hash == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

fn field(row: &crate::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn entity_facade() -> Option<crate::entity::EntityFacade> {
    ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
}

/// The `ScreenColorScheme` of `id` with its colors, or `None` when there is no such scheme.
///
/// # Errors
///
/// Returns an error when the entities cannot be read.
pub fn color_scheme(id: &str) -> Result<Option<ColorScheme>, Error> {
    let Some(facade) = entity_facade() else {
        return Ok(None);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find("moqui.screen.ScreenColorScheme")?
        .eq("colorSchemeId", Value::string(id))
        .use_cache(true)
        .one()?;
    row.map(|row| scheme_of(&facade, &row)).transpose()
}

fn scheme_of(facade: &crate::entity::EntityFacade, row: &crate::entity::EntityValue) -> Result<ColorScheme, Error> {
    let id = field(row, "colorSchemeId");
    let colors = facade
        .find("moqui.screen.ScreenColorSchemeColor")?
        .eq("colorSchemeId", Value::string(&id))
        .order_by("colorName", false)
        .use_cache(true)
        .list()?;
    let mut scheme = ColorScheme {
        description: Some(field(row, "description")).filter(|text| !text.is_empty()).unwrap_or_else(|| id.clone()),
        extra_css: field(row, "extraCss"),
        id,
        ..ColorScheme::default()
    };
    for color in colors.iter() {
        let entry = (field(color, "colorName"), field(color, "colorValue"));
        match field(color, "colorModeEnumId").as_str() {
            "SCM_LIGHT" => scheme.light.push(entry),
            "SCM_DARK" => scheme.dark.push(entry),
            _ => {}
        }
    }
    for (role, name) in [("body", "defaultFontBodyId"), ("heading", "defaultFontHeadingId"), ("mono", "defaultFontMonoId")] {
        let font = field(row, name);
        if !font.is_empty() {
            scheme.fonts.insert(role.to_string(), font);
        }
    }
    Ok(scheme)
}

/// The style sheet of the scheme `id` ([`ColorScheme::css`]); empty for an unknown scheme.
///
/// # Errors
///
/// Returns an error when the entities cannot be read.
pub fn color_scheme_css(id: &str) -> Result<String, Error> {
    Ok(color_scheme(id)?.map(|scheme| scheme.css()).unwrap_or_default())
}

/// What a user may change in Appearance, and the mode the theme forces. The default allows everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppearancePolicy {
    /// True when the theme locks every appearance preference.
    pub locked: bool,
    /// The mode the theme forces: `light`, `dark`, `auto`, or empty for the user's choice.
    pub mode: String,
    /// The groups of [`APPEARANCE_GROUPS`] the theme locks.
    pub locked_groups: BTreeSet<String>,
    /// More keys the theme (or an appearance service) locks by name.
    pub locked_keys: BTreeSet<String>,
    /// True when the user may not choose another theme (`theme: false`).
    pub theme_locked: bool,
}

impl AppearancePolicy {
    /// Every appearance preference is refused.
    pub fn locked() -> Self {
        Self {
            locked: true,
            ..Self::default()
        }
    }

    /// The policy of a `STRT_APPEARANCE_POLICY` text: a JSON object of `appearance` (false locks everything), `mode`, a
    /// `false` per group to lock, `lockedKeys`, and `theme` (false locks the choice of theme). None or blank allows
    /// everything; one that is not a JSON object is logged and locks everything rather than being trusted.
    pub fn parse(text: Option<&str>) -> Self {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
            return Self::default();
        };
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(json) if json.is_object() => Self::from_value(&moqui_lib::json::from_json(&json)),
            _ => {
                tracing::error!("Theme {APPEARANCE_POLICY} is not a JSON object, user appearance is locked");
                Self::locked()
            }
        }
    }

    /// The policy of a map, as [`AppearancePolicy::parse`] reads it and an appearance service answers it. Not a map
    /// allows everything.
    pub fn from_value(value: &Value) -> Self {
        let Value::Map(map) = value else {
            return Self::default();
        };
        let flag = |name: &str| match map.get(name) {
            Some(Value::Bool(flag)) => Some(*flag),
            Some(Value::String(text)) if text == "true" || text == "false" => Some(text == "true"),
            _ => None,
        };
        let mode = map
            .get("mode")
            .map(to_plain_string)
            .map(|mode| mode.trim().to_string())
            .filter(|mode| matches!(mode.as_str(), "light" | "dark" | "auto"))
            .unwrap_or_default();
        let locked_keys = match map.get("lockedKeys") {
            Some(Value::List(items)) => items.iter().map(to_plain_string).filter(|key| is_appearance_key(key)).collect(),
            _ => BTreeSet::new(),
        };
        Self {
            locked: flag("appearance") == Some(false),
            mode,
            locked_groups: APPEARANCE_GROUPS
                .iter()
                .filter(|group| flag(group) == Some(false))
                .map(|group| (*group).to_string())
                .collect(),
            locked_keys,
            theme_locked: flag("theme") == Some(false),
        }
    }

    /// Why the user may not choose another theme, or `None` when the policy allows it.
    pub fn theme_denial(&self) -> Option<String> {
        (self.locked || self.theme_locked).then(|| "The theme does not allow choosing another theme".to_string())
    }

    /// Every key of [`APPEARANCE_KEYS`] the policy locks, with the extra ones it names.
    pub fn all_locked_keys(&self) -> BTreeSet<String> {
        let mut keys: BTreeSet<String> = APPEARANCE_KEYS
            .iter()
            .filter(|(key, group)| self.locked || self.locked_groups.contains(*group) || (*key == "QUASAR_DARK" && !self.mode.is_empty()))
            .map(|(key, _)| (*key).to_string())
            .collect();
        keys.extend(self.locked_keys.iter().cloned());
        keys
    }

    /// Why `key` may not be written, or `None` when the policy allows it. A key that is not an appearance preference is
    /// always allowed; a `THEME_*` key of a component is locked with the rest when the theme locks everything.
    pub fn denial(&self, key: &str) -> Option<String> {
        if !is_appearance_key(key) {
            return None;
        }
        if key == "QUASAR_DARK" && !self.mode.is_empty() && !self.locked {
            return Some(format!("The theme forces {} mode", self.mode));
        }
        (self.locked || self.all_locked_keys().contains(key))
            .then(|| format!("The theme does not allow the {key} preference"))
    }

    /// The policy as the page and an appearance service read it: `appearance`, a flag per group (`mode` may instead
    /// name a forced mode), `theme`, and `lockedKeys`, every key it locks.
    pub fn as_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert("appearance".to_string(), Value::Bool(!self.locked));
        for group in APPEARANCE_GROUPS {
            map.insert((*group).to_string(), Value::Bool(!self.locked && !self.locked_groups.contains(*group)));
        }
        // `mode` is also a permission group. Preserve a forced mode instead of replacing it with that group's flag.
        if !self.mode.is_empty() {
            map.insert("mode".to_string(), Value::string(&self.mode));
        }
        map.insert("theme".to_string(), Value::Bool(self.theme_denial().is_none()));
        map.insert(
            "lockedKeys".to_string(),
            Value::List(self.all_locked_keys().into_iter().map(Value::String).collect()),
        );
        Value::Map(map)
    }
}

fn names(list: &[&str]) -> Value {
    Value::List(list.iter().map(|name| Value::string(*name)).collect())
}

/// The appearance before the extension point: the theme's schemes (its own, then its parent's), the fonts, the
/// themes the user may choose, the default scheme, the policy, the user's values and the names the page and an
/// appearance service need.
fn base_appearance(user_id: Option<&str>, theme_id: &str) -> Result<IndexMap<String, Value>, Error> {
    let mut appearance = IndexMap::new();
    let mut schemes = Vec::new();
    let mut fonts = Vec::new();
    let mut themes = Vec::new();
    if let Some(facade) = entity_facade() {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        for owner in theme_chain(theme_id) {
            let rows = facade
                .find("moqui.screen.ScreenColorScheme")?
                .eq("screenThemeId", Value::string(&owner))
                .order_by("sequenceNum", false)
                .use_cache(true)
                .list()?;
            for row in rows.iter() {
                schemes.push(scheme_of(&facade, row)?.as_value());
            }
        }
        let rows = facade
            .find("moqui.screen.ScreenTheme")?
            .eq("screenThemeTypeEnumId", Value::string(QUASAR_THEME_TYPE))
            .use_cache(true)
            .list()?;
        let mut found: Vec<(String, String)> = rows
            .iter()
            .map(|row| {
                let id = field(row, "screenThemeId");
                let description = Some(field(row, "description"))
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| id.clone());
                (description, id)
            })
            .collect();
        found.sort();
        for (description, id) in found {
            themes.push(Value::Map(IndexMap::from([
                ("id".to_string(), Value::String(id)),
                ("description".to_string(), Value::String(description)),
            ])));
        }
        let rows = facade
            .find("moqui.screen.ScreenFontOption")?
            .order_by("sequenceNum", false)
            .use_cache(true)
            .list()?;
        for row in rows.iter() {
            let id = field(row, "fontOptionId");
            let kind = match field(row, "fontKindEnumId").as_str() {
                "SFK_MONO" => "mono",
                "SFK_SERIF" => "serif",
                _ => "sans",
            };
            let mut font = IndexMap::new();
            font.insert(
                "description".to_string(),
                Value::string(Some(field(row, "description")).filter(|text| !text.is_empty()).unwrap_or_else(|| id.clone())),
            );
            font.insert("id".to_string(), Value::string(id));
            font.insert("kind".to_string(), Value::string(kind));
            font.insert("family".to_string(), Value::string(field(row, "fontFamily")));
            font.insert("stylesheet".to_string(), Value::string(field(row, "stylesheetLocation")));
            fonts.push(Value::Map(font));
        }
    }
    let first = |resource: &str| match theme_resource_values(theme_id, resource) {
        Ok(values) => values.into_iter().next(),
        Err(err) => {
            tracing::error!("Could not read {resource} of theme {theme_id}: {err}");
            None
        }
    };
    let policy = AppearancePolicy::parse(first(APPEARANCE_POLICY).as_deref());
    // only the user's own choices of the keys the page applies; the mode and density are the shell's own settings
    let mut values = IndexMap::new();
    let signed_in = user_id.is_some_and(|id| !id.is_empty());
    for (key, _) in APPEARANCE_KEYS.iter().filter(|(key, _)| signed_in && key.starts_with("THEME_")) {
        if let Ok(Some(value)) = crate::user::get_preference_for(key, user_id) {
            if !value.is_empty() {
                values.insert((*key).to_string(), Value::String(value));
            }
        }
    }
    let keys = APPEARANCE_KEYS
        .iter()
        .map(|(key, group)| {
            Value::Map(IndexMap::from([
                ("key".to_string(), Value::string(*key)),
                ("group".to_string(), Value::string(*group)),
            ]))
        })
        .collect();
    let mut tokens = IndexMap::new();
    tokens.insert("brand".to_string(), names(BRAND_TOKENS));
    tokens.insert("surface".to_string(), names(SURFACE_TOKENS));
    tokens.insert("shape".to_string(), names(SHAPE_TOKENS));
    appearance.insert("screenThemeId".to_string(), Value::string(theme_id));
    appearance.insert("colorSchemes".to_string(), Value::List(schemes));
    appearance.insert("fonts".to_string(), Value::List(fonts));
    appearance.insert("themes".to_string(), Value::List(themes));
    appearance.insert("defaultSchemeId".to_string(), Value::string(first(DEFAULT_COLOR_SCHEME).unwrap_or_default()));
    appearance.insert("policy".to_string(), policy.as_value());
    appearance.insert("values".to_string(), Value::Map(values));
    appearance.insert("keys".to_string(), Value::List(keys));
    appearance.insert("tokens".to_string(), Value::Map(tokens));
    Ok(appearance)
}

/// Each `STRT_APPEARANCE_SERVICE` of the theme in sequence, with the appearance so far; its `appearance` answer is
/// what the next one gets, each in its own transaction with authorization disabled. One that fails or answers no map is
/// logged and skipped.
fn call_appearance_services(user_id: Option<&str>, theme_id: &str, mut appearance: IndexMap<String, Value>) -> IndexMap<String, Value> {
    let services = theme_resource_values(theme_id, APPEARANCE_SERVICE).unwrap_or_else(|err| {
        tracing::error!("Could not read {APPEARANCE_SERVICE} of theme {theme_id}: {err}");
        Vec::new()
    });
    for service in services {
        let parameters = BTreeMap::from([
            ("userId".to_string(), user_id.map_or(Value::Null, Value::string)),
            ("screenThemeId".to_string(), Value::string(theme_id)),
            ("appearance".to_string(), Value::Map(appearance.clone())),
        ]);
        // in a transaction of its own, so a failing one leaves the caller's (a setPreference) as it was
        let options = crate::service::call::CallOptions {
            disable_authz: true,
            require_new_transaction: Some(true),
            ..Default::default()
        };
        match crate::service::shell::call_tolerant_with(&service, &parameters, &options) {
            Ok(out) => match out.get("appearance") {
                Some(Value::Map(answer)) => appearance = answer.clone(),
                _ => tracing::error!("The appearance service {service} (theme {theme_id}) answered no appearance map; skipped"),
            },
            Err(err) => tracing::error!("The appearance service {service} (theme {theme_id}) failed; skipped: {err}"),
        }
    }
    appearance
}

/// `text` as a URL query component, as the page's `encodeURIComponent`.
fn url_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn text_of(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn scheme_ids(appearance: &IndexMap<String, Value>) -> Vec<String> {
    match appearance.get("colorSchemes") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(scheme) => Some(text_of(scheme, "id")),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// What follows from the appearance once the services have answered: the policy normalized (`lockedKeys` every key it
/// locks), the locked values left out, the default scheme only when it is a scheme of the theme, the effective scheme
/// (`schemeId`, the user's, else the default) and the style sheets the page links before the client starts (`links`:
/// the effective scheme's, and the font of each role, the user's else the scheme's).
fn finish_appearance(mut appearance: IndexMap<String, Value>) -> IndexMap<String, Value> {
    let policy = AppearancePolicy::from_value(appearance.get("policy").unwrap_or(&Value::Null));
    let locked = policy.all_locked_keys();
    let mut values = match appearance.get("values") {
        Some(Value::Map(values)) => values.clone(),
        _ => IndexMap::new(),
    };
    values.retain(|key, value| !locked.contains(key) && !(policy.locked && is_appearance_key(key)) && !to_plain_string(value).is_empty());
    let ids = scheme_ids(&appearance);
    let default_id = Some(text_of(&appearance, "defaultSchemeId")).filter(|id| ids.contains(id)).unwrap_or_default();
    let own = values.get("THEME_COLOR_SCHEME").map(to_plain_string).filter(|id| ids.contains(id));
    let scheme_id = own.unwrap_or_else(|| default_id.clone());
    let scheme = match appearance.get("colorSchemes") {
        Some(Value::List(items)) => items.iter().find_map(|item| match item {
            Value::Map(scheme) if text_of(scheme, "id") == scheme_id => Some(scheme.clone()),
            _ => None,
        }),
        _ => None,
    };
    let fonts = match appearance.get("fonts") {
        Some(Value::List(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut links: Vec<Value> = Vec::new();
    let mut link = |id: &str, href: String| {
        if !href.is_empty() && !links.iter().any(|link| matches!(link, Value::Map(map) if text_of(map, "href") == href)) {
            links.push(Value::Map(IndexMap::from([
                ("id".to_string(), Value::string(id)),
                ("href".to_string(), Value::String(href)),
            ])));
        }
    };
    if let Some(scheme) = scheme.as_ref().filter(|scheme| matches!(scheme.get("custom"), Some(Value::Bool(true)))) {
        link(
            "m-color-scheme",
            format!(
                "/apps/colorSchemeCss?colorSchemeId={}&v={}",
                url_component(&scheme_id),
                url_component(&text_of(scheme, "version"))
            ),
        );
    }
    for (key, role) in [("THEME_FONT_BODY", "body"), ("THEME_FONT_HEADING", "heading"), ("THEME_FONT_MONO", "mono")] {
        let chosen = values.get(key).map(to_plain_string).or_else(|| match scheme.as_ref().and_then(|scheme| scheme.get("fonts")) {
            Some(Value::Map(fonts)) => fonts.get(role).map(to_plain_string),
            _ => None,
        });
        let Some(chosen) = chosen else { continue };
        let sheet = fonts.iter().find_map(|font| match font {
            Value::Map(font) if text_of(font, "id") == chosen => Some(text_of(font, "stylesheet")),
            _ => None,
        });
        link(&format!("m-font-{role}"), sheet.unwrap_or_default());
    }
    appearance.insert("policy".to_string(), policy.as_value());
    appearance.insert("values".to_string(), Value::Map(values));
    appearance.insert("defaultSchemeId".to_string(), Value::String(default_id));
    appearance.insert("schemeId".to_string(), Value::String(scheme_id));
    appearance.insert("links".to_string(), Value::List(links));
    appearance
}

/// The appearance of `theme_id` for `user_id` (the module docs): `screenThemeId`, `colorSchemes` (each `id`,
/// `description`, `light`, `dark`, `custom`, `version`, `fonts`), `fonts`, `themes` (each `id`, `description`),
/// `defaultSchemeId`, `schemeId`,
/// `policy`, `values`, `keys`, `tokens` and `links`.
///
/// # Errors
///
/// Returns an error when the theme's entities cannot be read.
pub fn build_appearance(user_id: Option<&str>, theme_id: &str) -> Result<IndexMap<String, Value>, Error> {
    let base = base_appearance(user_id, theme_id)?;
    Ok(finish_appearance(call_appearance_services(user_id, theme_id, base)))
}

/// The policy of the Quasar theme of `user_id`, resolved as the shell resolves it and after the theme's appearance
/// services. A theme that cannot be read is logged and allows everything.
pub fn appearance_policy_for_user(user_id: Option<&str>) -> AppearancePolicy {
    let theme = user_theme_id(user_id, QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    match build_appearance(user_id, &theme) {
        Ok(appearance) => AppearancePolicy::from_value(appearance.get("policy").unwrap_or(&Value::Null)),
        Err(err) => {
            tracing::error!("Could not build the appearance of theme {theme}: {err}");
            AppearancePolicy::default()
        }
    }
}

#[cfg(test)]
#[path = "appearance_tests.rs"]
mod tests;
