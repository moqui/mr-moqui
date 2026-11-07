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

//! A theme package: its files by path, the manifest, page templates and schemas read from them, snippet includes
//! assembled, and the checks of `validate` (design-shop §12.2, §12.3, §12.5).

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use moqui_lib::sync::lock;
use serde_json::{Map, Value as Json};

use super::settings::{self, SettingDef};
use super::{expr, markup, svg, Diagnostic, Limits, ValidateOptions, JS_FANCY, VIEW_API_VERSIONS};
use crate::script::template::{self, Token};

pub(crate) const MANIFEST: &str = "theme.json";
pub(crate) const TEMPLATE_EXTENSION: &str = ".html.fancy-locked";
const SETTINGS_SCHEMA: &str = "config/settings.schema.json";
const SETTINGS_DATA: &str = "config/settings.json";
const MAX_FILES: usize = 2000;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// An assembled template (a file with its snippets put in) past this is refused: includes cannot expand the source
/// without bound.
const MAX_ASSEMBLED_BYTES: usize = 1024 * 1024;
const ASSET_EXTENSIONS: &[&str] = &[
    "css", "svg", "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "woff", "woff2", "ttf", "otf",
];

/// The files of a theme, as data. Text files are kept by path; a binary asset (an image, a font) is only named, with
/// its digest, size and type when the caller gives them, since its bytes are the caller's to store and serve by digest
/// (Shop F55): they are never part of the package a render reads.
#[derive(Clone, Debug, Default)]
pub struct Package {
    files: BTreeMap<String, Arc<str>>,
    binary: BTreeMap<String, Option<BinaryAsset>>,
    load: Vec<Diagnostic>,
}

/// A binary asset by reference: `{digest, size, contentType}`. The type is the one its extension names, so a caller
/// serving the bytes with it serves what the theme's checks allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryAsset {
    /// The digest the caller stores the bytes by (a sha256 hex, for one): letters, digits and `+/=_:-`, at most 128.
    pub digest: String,
    pub size: u64,
    pub content_type: String,
}

/// The content type of each binary asset extension.
const BINARY_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
];

/// The largest binary asset a reference may name.
const MAX_BINARY_BYTES: u64 = 16 * 1024 * 1024;

/// One file as given: its text, a binary asset (by reference, or only named), or why the entry is not one.
enum Entry<'a> {
    Text(&'a str),
    Binary(Option<BinaryAsset>),
    Invalid(String),
}

impl BinaryAsset {
    /// `{digest, size, contentType}` of the asset at `path`, checked: the digest's form, the size, and the type its
    /// extension names.
    fn from_json(path: &str, value: &Map<String, Json>) -> Result<Self, String> {
        let digest = value.get("digest").and_then(Json::as_str).unwrap_or_default();
        if digest.is_empty()
            || digest.len() > 128
            || !digest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '_' | ':' | '-'))
        {
            return Err(format!(
                "`{path}` needs a digest of at most 128 letters, digits and +/=_:-"
            ));
        }
        let Some(size) = value
            .get("size")
            .and_then(Json::as_u64)
            .filter(|size| *size <= MAX_BINARY_BYTES)
        else {
            return Err(format!("`{path}` needs a size in bytes, at most {MAX_BINARY_BYTES}"));
        };
        let extension = path
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        let Some((_, expected)) = BINARY_TYPES.iter().find(|(known, _)| *known == extension) else {
            return Err(format!("`{path}` is not a binary asset type"));
        };
        let content_type = value.get("contentType").and_then(Json::as_str).unwrap_or_default();
        if !content_type.eq_ignore_ascii_case(expected) {
            return Err(format!("`{path}` is {expected}, not `{content_type}`"));
        }
        Ok(Self {
            digest: digest.to_string(),
            size,
            content_type: (*expected).to_string(),
        })
    }
}

impl Package {
    /// A package from `{files: {path: text}}` or `{path: text}`. A path whose value is not text is a binary asset:
    /// `{digest, size, contentType}` by reference (checked; one that does not check is left out with an
    /// `invalid-asset` error), or `true` only named. A path outside the package layout is left out with an error.
    pub fn from_json(value: &Json) -> Self {
        let files = match value.get("files") {
            Some(Json::Object(files)) => files,
            _ => match value {
                Json::Object(files) => files,
                _ => {
                    let mut package = Self::default();
                    package.load.push(Diagnostic::error(
                        "invalid-package",
                        "",
                        0,
                        "a theme package is an object of files by path",
                    ));
                    return package;
                }
            },
        };
        Self::from_entries(files.iter().map(|(path, value)| {
            let entry = match value {
                Json::String(text) => Entry::Text(text),
                Json::Object(asset) => match BinaryAsset::from_json(path, asset) {
                    Ok(asset) => Entry::Binary(Some(asset)),
                    Err(message) => Entry::Invalid(message),
                },
                _ => Entry::Binary(None),
            };
            (path.as_str(), entry)
        }))
    }

    /// A package of text files.
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self::from_entries(files.into_iter().map(|(path, text)| (path, Entry::Text(text))))
    }

    fn from_entries<'a>(entries: impl IntoIterator<Item = (&'a str, Entry<'a>)>) -> Self {
        let mut package = Self::default();
        for (path, text) in entries {
            if package.files.len() + package.binary.len() >= MAX_FILES {
                package.load.push(Diagnostic::error(
                    "budget",
                    "",
                    0,
                    format!("a package holds at most {MAX_FILES} files"),
                ));
                break;
            }
            if let Err(message) = check_path(path, matches!(text, Entry::Text(_))) {
                package.load.push(Diagnostic::error(
                    if path.ends_with(".liquid") {
                        "liquid"
                    } else {
                        "unknown-file"
                    },
                    path,
                    0,
                    message,
                ));
                continue;
            }
            match text {
                Entry::Text(text) if text.len() > MAX_TEXT_BYTES => package.load.push(Diagnostic::error(
                    "budget",
                    path,
                    0,
                    format!("a text file is at most {MAX_TEXT_BYTES} bytes"),
                )),
                Entry::Text(text) => {
                    package.files.insert(path.to_string(), Arc::from(text));
                }
                Entry::Binary(asset) => {
                    package.binary.insert(path.to_string(), asset);
                }
                Entry::Invalid(message) => package.load.push(Diagnostic::error("invalid-asset", path, 0, message)),
            }
        }
        package
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|text| &**text)
    }

    pub fn has(&self, path: &str) -> bool {
        self.files.contains_key(path) || self.binary.contains_key(path)
    }

    /// A binary asset's reference, when it was given by one.
    pub fn binary_asset(&self, path: &str) -> Option<&BinaryAsset> {
        self.binary.get(path).and_then(Option::as_ref)
    }

    /// Every path, text and binary, in order.
    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self
            .files
            .keys()
            .chain(self.binary.keys())
            .map(String::as_str)
            .collect();
        paths.sort_unstable();
        paths
    }

    /// The paths under `assets/`.
    pub(crate) fn assets(&self) -> Vec<&str> {
        self.paths()
            .into_iter()
            .filter(|path| path.starts_with("assets/"))
            .collect()
    }

    pub(crate) fn load_diagnostics(&self) -> &[Diagnostic] {
        &self.load
    }
}

/// A name of a layout, template, section, block or snippet: letters, digits, `-` and `_`, and `.` between parts for
/// an alternate template (`product.featured`).
pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        })
}

/// Whether a path is part of the layout of design-shop §12.2.
fn check_path(path: &str, is_text: bool) -> Result<(), String> {
    if path.ends_with(".liquid") {
        return Err("Liquid templates are not accepted; the engine is js-fancy".to_string());
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("`{path}` is not a relative package path"));
    }
    let named = |rest: &str, suffix: &str| rest.strip_suffix(suffix).is_some_and(is_name);
    let ok = match path.split_once('/') {
        None => path == MANIFEST,
        Some(("layouts", rest)) | Some(("snippets", rest)) => named(rest, TEMPLATE_EXTENSION),
        Some(("templates", rest)) | Some(("locales", rest)) => named(rest, ".json"),
        Some(("sections", rest)) | Some(("blocks", rest)) => {
            named(rest, TEMPLATE_EXTENSION) || named(rest, ".schema.json")
        }
        Some(("config", rest)) => rest == "settings.schema.json" || rest == "settings.json",
        Some(("assets", rest)) => {
            let extension = rest
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase());
            if !extension
                .as_deref()
                .is_some_and(|extension| ASSET_EXTENSIONS.contains(&extension))
            {
                return Err(format!(
                    "`{path}` is not an asset type a theme may hold ({})",
                    ASSET_EXTENSIONS.join(", ")
                ));
            }
            if !is_text && matches!(extension.as_deref(), Some("css" | "svg")) {
                return Err(format!("`{path}` must be given as text so it can be checked"));
            }
            rest.split('/').all(|part| {
                part.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            })
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("`{path}` is not part of the theme package layout"))
    }
}

/// The 1-based line of the first `"key"` in a JSON text, or 0.
pub(crate) fn json_line(text: &str, key: &str) -> usize {
    let quoted = format!("\"{key}\"");
    text.find(&quoted).map_or(0, |at| template::line_of(text, at))
}

/// A JSON file of the package, parsed, or the diagnostic with its line.
pub(crate) fn json_file(package: &Package, path: &str) -> Result<Option<Json>, Diagnostic> {
    let Some(text) = package.text(path) else {
        return Ok(None);
    };
    let parsed: Json = serde_json::from_str(text)
        .map_err(|err| Diagnostic::error("invalid-json", path, err.line(), format!("not JSON: {err}")))?;
    let duplicates = duplicate_keys(text);
    if let Some(key) = duplicates.first() {
        return Err(Diagnostic::error(
            "duplicate-id",
            path,
            json_line(text, key),
            format!("`{key}` is given twice in one object"),
        ));
    }
    Ok(Some(parsed))
}

pub(crate) fn manifest_json(package: &Package) -> Result<Json, Diagnostic> {
    match json_file(package, MANIFEST)? {
        Some(Json::Object(map)) => Ok(Json::Object(map)),
        Some(_) => Err(Diagnostic::error(
            "invalid-manifest",
            MANIFEST,
            1,
            "theme.json is not an object",
        )),
        None => Err(Diagnostic::error(
            "missing-file",
            MANIFEST,
            0,
            "the package has no theme.json",
        )),
    }
}

/// The parts of `theme.json` the adapter uses.
#[derive(Clone, Debug, Default)]
pub(crate) struct Manifest {
    pub name: String,
    pub layout: Option<String>,
    pub required_components: Vec<String>,
    pub styles: Vec<String>,
    pub default_templates: BTreeMap<String, String>,
    /// The locale whose `locales/<name>.json` texts `t(key)` falls back to: `defaultLocale`, else `en`.
    pub default_locale: String,
}

/// The default locale of a theme that names none.
pub(crate) const DEFAULT_LOCALE: &str = "en";

/// `theme.json` read and checked.
pub(crate) fn manifest(package: &Package, diagnostics: &mut Vec<Diagnostic>) -> Option<Manifest> {
    let json = match manifest_json(package) {
        Ok(json) => json,
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            return None;
        }
    };
    let text = package.text(MANIFEST).unwrap_or_default();
    let error =
        |key: &str, message: String| Diagnostic::error("invalid-manifest", MANIFEST, json_line(text, key), message);
    if json.get("schemaVersion").and_then(Json::as_u64) != Some(1) {
        diagnostics.push(error("schemaVersion", "schemaVersion must be 1".to_string()));
    }
    let name = json.get("name").and_then(Json::as_str).unwrap_or_default().to_string();
    if name.trim().is_empty() {
        diagnostics.push(error("name", "the theme needs a name".to_string()));
    }
    if json.get("engine").and_then(Json::as_str) != Some(JS_FANCY) {
        diagnostics.push(error("engine", format!("engine must be `{JS_FANCY}`")));
    }
    match json.get("viewApiVersion").and_then(Json::as_u64) {
        Some(version) if VIEW_API_VERSIONS.contains(&version) => {}
        other => diagnostics.push(error(
            "viewApiVersion",
            format!(
                "viewApiVersion {} is not one this framework renders ({VIEW_API_VERSIONS:?})",
                other.map_or_else(|| "(none)".to_string(), |version| version.to_string())
            ),
        )),
    }
    let layout = json.get("layout").and_then(Json::as_str).map(str::to_string);
    let strings = |key: &str, diagnostics: &mut Vec<Diagnostic>| -> Vec<String> {
        match json.get(key) {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(items)) => items
                .iter()
                .filter_map(|item| match item.as_str() {
                    Some(text) => Some(text.to_string()),
                    None => {
                        diagnostics.push(error(key, format!("{key} lists names as strings")));
                        None
                    }
                })
                .collect(),
            Some(_) => {
                diagnostics.push(error(key, format!("{key} is a list")));
                Vec::new()
            }
        }
    };
    let required_components = strings("requiredComponents", diagnostics);
    let styles = strings("styles", diagnostics);
    let mut default_templates = BTreeMap::new();
    match json.get("defaultTemplates") {
        None | Some(Json::Null) => {}
        Some(Json::Object(map)) => {
            for (kind, name) in map {
                match name.as_str() {
                    Some(name) => {
                        default_templates.insert(kind.clone(), name.to_string());
                    }
                    None => diagnostics.push(error(kind, format!("the default template of `{kind}` is a name"))),
                }
            }
        }
        Some(_) => diagnostics.push(error("defaultTemplates", "defaultTemplates is an object".to_string())),
    }
    let default_locale = match json.get("defaultLocale") {
        None | Some(Json::Null) => DEFAULT_LOCALE.to_string(),
        Some(Json::String(name)) if is_name(name) => name.clone(),
        Some(_) => {
            diagnostics.push(error(
                "defaultLocale",
                "defaultLocale is the name of a locales/ file, such as en".to_string(),
            ));
            DEFAULT_LOCALE.to_string()
        }
    };
    Some(Manifest {
        name,
        layout,
        required_components,
        styles,
        default_templates,
        default_locale,
    })
}

/// The texts `t(key)` reads for `locale`: the default locale's file, then the locale's language's (`fr` of `fr_CA`),
/// then the locale's own, each over the one before, so a key falls back from the locale to its language to the default.
/// A nested object's texts are keyed by their path (`cart.title`); values other than text are left out. A file that
/// is not there or not an object of texts adds nothing (validation reports it).
pub(crate) fn texts(package: &Package, locale: Option<&str>, default_locale: &str) -> template::Texts {
    let mut names = vec![default_locale.to_string()];
    if let Some(locale) = locale.map(str::trim).filter(|locale| !locale.is_empty()) {
        let language = locale.split(['_', '-']).next().unwrap_or(locale);
        names.push(language.to_string());
        names.push(locale.to_string());
        // `fr-CA` and `fr_CA` name one locale
        names.push(locale.replace('-', "_"));
    }
    let mut out = template::Texts::new();
    let mut seen = HashSet::new();
    for name in names {
        if !is_name(&name) || !seen.insert(name.clone()) {
            continue;
        }
        if let Ok(Some(Json::Object(map))) = json_file(package, &format!("locales/{name}.json")) {
            flatten_texts("", &map, &mut out);
        }
    }
    out
}

fn flatten_texts(prefix: &str, map: &Map<String, Json>, out: &mut template::Texts) {
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Json::String(text) => {
                out.insert(path, text.clone());
            }
            Json::Object(nested) => flatten_texts(&path, nested, out),
            _ => {}
        }
    }
}

/// A section or block of a page template.
#[derive(Clone, Debug, Default)]
pub(crate) struct Instance {
    pub id: String,
    pub kind: String,
    pub settings: Map<String, Json>,
    pub blocks: Vec<Instance>,
    pub disabled: bool,
    pub required: bool,
}

/// A page template, `templates/<name>.json`.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageTemplate {
    pub file: String,
    pub name: String,
    /// None for the theme's layout, `Some(None)` for none (`"layout": false`).
    pub layout: Option<Option<String>>,
    pub sections: Vec<Instance>,
}

impl PageTemplate {
    /// The page kind: the name before its first `.`.
    pub fn page_kind(&self) -> &str {
        self.name.split('.').next().unwrap_or(&self.name)
    }
}

pub(crate) fn template_file(name: &str) -> String {
    format!("templates/{name}.json")
}

pub(crate) fn section_file(kind: &str) -> String {
    format!("sections/{kind}{TEMPLATE_EXTENSION}")
}

pub(crate) fn block_file(kind: &str) -> String {
    format!("blocks/{kind}{TEMPLATE_EXTENSION}")
}

pub(crate) fn layout_file(name: &str) -> String {
    format!("layouts/{name}{TEMPLATE_EXTENSION}")
}

fn snippet_file(name: &str) -> String {
    format!("snippets/{name}{TEMPLATE_EXTENSION}")
}

/// Read a page template. Errors that make it unusable are returned; the rest are added to `diagnostics`.
pub(crate) fn page_template(
    package: &Package,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<PageTemplate, Diagnostic> {
    let file = template_file(name);
    let json = json_file(package, &file)?
        .ok_or_else(|| Diagnostic::error("missing-file", &file, 0, format!("there is no template `{name}`")))?;
    let text = package.text(&file).unwrap_or_default();
    let Json::Object(map) = json else {
        return Err(Diagnostic::error(
            "invalid-template",
            &file,
            1,
            "a template is an object",
        ));
    };
    let layout = match map.get("layout") {
        None | Some(Json::Null) => None,
        Some(Json::Bool(false)) => Some(None),
        Some(Json::String(layout)) => Some(Some(layout.clone())),
        Some(_) => {
            return Err(Diagnostic::error(
                "invalid-template",
                &file,
                json_line(text, "layout"),
                "layout is a layout name, or false for none",
            ))
        }
    };
    let empty = Map::new();
    let sections = match map.get("sections") {
        None | Some(Json::Null) => &empty,
        Some(Json::Object(sections)) => sections,
        Some(_) => {
            return Err(Diagnostic::error(
                "invalid-template",
                &file,
                json_line(text, "sections"),
                "sections is an object of section instances by id",
            ))
        }
    };
    let order = ordered(sections, map.get("order"), &file, text, "order", diagnostics)?;
    let mut instances = Vec::new();
    for id in order {
        let Some(value) = sections.get(&id) else {
            continue;
        };
        instances.push(instance(value, &id, &file, text, diagnostics)?);
    }
    Ok(PageTemplate {
        file,
        name: name.to_string(),
        layout,
        sections: instances,
    })
}

/// The ids of `items` in the order `order` gives. Without an order there must be at most one item, since a JSON
/// object has no order of its own here.
fn ordered(
    items: &Map<String, Json>,
    order: Option<&Json>,
    file: &str,
    text: &str,
    order_key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<String>, Diagnostic> {
    let ids: Vec<String> = match order {
        None | Some(Json::Null) => {
            if items.len() > 1 {
                return Err(Diagnostic::error(
                    "invalid-template",
                    file,
                    0,
                    format!("`{order_key}` must list the ids when there is more than one"),
                ));
            }
            items.keys().cloned().collect()
        }
        Some(Json::Array(list)) => {
            let mut seen = HashSet::new();
            let mut ids = Vec::new();
            for item in list {
                let Some(id) = item.as_str() else {
                    return Err(Diagnostic::error(
                        "invalid-template",
                        file,
                        json_line(text, order_key),
                        format!("`{order_key}` lists ids as strings"),
                    ));
                };
                if !seen.insert(id) {
                    return Err(Diagnostic::error(
                        "duplicate-id",
                        file,
                        json_line(text, order_key),
                        format!("`{id}` is in `{order_key}` twice"),
                    ));
                }
                if !items.contains_key(id) {
                    return Err(Diagnostic::error(
                        "invalid-reference",
                        file,
                        json_line(text, order_key),
                        format!("`{order_key}` names `{id}`, which is not defined"),
                    ));
                }
                ids.push(id.to_string());
            }
            ids
        }
        Some(_) => {
            return Err(Diagnostic::error(
                "invalid-template",
                file,
                json_line(text, order_key),
                format!("`{order_key}` is a list of ids"),
            ))
        }
    };
    for id in items.keys() {
        if !ids.contains(id) {
            diagnostics.push(Diagnostic::warning(
                "not-rendered",
                file,
                json_line(text, id),
                format!("`{id}` is not in `{order_key}`, so it is not rendered"),
            ));
        }
    }
    Ok(ids)
}

fn instance(
    value: &Json,
    id: &str,
    file: &str,
    text: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Instance, Diagnostic> {
    let line = json_line(text, id);
    let invalid = |message: String| Diagnostic::error("invalid-template", file, line, message);
    if !is_name(id) || id.contains('.') {
        return Err(invalid(format!("`{id}` is not an id: letters, digits, - and _")));
    }
    let Json::Object(map) = value else {
        return Err(invalid(format!("`{id}` is not an object")));
    };
    let kind = map
        .get("type")
        .and_then(Json::as_str)
        .filter(|kind| is_name(kind))
        .ok_or_else(|| invalid(format!("`{id}` needs a type")))?;
    let settings = match map.get("settings") {
        None | Some(Json::Null) => Map::new(),
        Some(Json::Object(settings)) => settings.clone(),
        Some(_) => return Err(invalid(format!("the settings of `{id}` are an object"))),
    };
    let empty = Map::new();
    let blocks = match map.get("blocks") {
        None | Some(Json::Null) => &empty,
        Some(Json::Object(blocks)) => blocks,
        Some(_) => return Err(invalid(format!("the blocks of `{id}` are an object by id"))),
    };
    let order = ordered(blocks, map.get("blockOrder"), file, text, "blockOrder", diagnostics)?;
    let mut children = Vec::new();
    for child in order {
        if let Some(value) = blocks.get(&child) {
            children.push(instance(value, &child, file, text, diagnostics)?);
        }
    }
    let flag = |name: &str| map.get(name).and_then(Json::as_bool).unwrap_or(false);
    Ok(Instance {
        id: id.to_string(),
        kind: kind.to_string(),
        settings,
        blocks: children,
        disabled: flag("disabled"),
        required: flag("required"),
    })
}

/// A block a section or block allows, with how many of it.
#[derive(Clone, Debug)]
pub(crate) struct BlockAllow {
    pub kind: String,
    pub limit: Option<usize>,
}

/// A section or block schema, `sections/<type>.schema.json` or `blocks/<type>.schema.json`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Schema {
    pub file: String,
    pub settings: Vec<SettingDef>,
    pub blocks: Vec<BlockAllow>,
    pub max_blocks: Option<usize>,
    pub allowed_page_kinds: Option<Vec<String>>,
    pub required: bool,
    /// Static markup shown in place of the section when it fails.
    pub placeholder: Option<String>,
}

/// The schema of a section (`sections`) or block (`blocks`) type. A type with no schema file has no settings and
/// takes no blocks.
pub(crate) fn schema(package: &Package, dir: &str, kind: &str, diagnostics: &mut Vec<Diagnostic>) -> Schema {
    let file = format!("{dir}/{kind}.schema.json");
    let mut schema = Schema {
        file: file.clone(),
        ..Schema::default()
    };
    let json = match json_file(package, &file) {
        Ok(Some(json)) => json,
        Ok(None) => return schema,
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            return schema;
        }
    };
    let text = package.text(&file).unwrap_or_default();
    let error = |key: &str, message: String| Diagnostic::error("invalid-schema", &file, json_line(text, key), message);
    let Json::Object(map) = json else {
        diagnostics.push(error("", "a schema is an object".to_string()));
        return schema;
    };
    let has_asset = |path: &str| package.has(path);
    schema.settings = settings::parse_defs(map.get("settings"), &file, text, &has_asset, diagnostics);
    match map.get("blocks") {
        None | Some(Json::Null) => {}
        Some(Json::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Json::as_str) {
                    Some(kind) if is_name(kind) => schema.blocks.push(BlockAllow {
                        kind: kind.to_string(),
                        limit: item
                            .get("limit")
                            .and_then(Json::as_u64)
                            .and_then(|limit| usize::try_from(limit).ok()),
                    }),
                    _ => diagnostics.push(error("blocks", "each allowed block has a type".to_string())),
                }
            }
        }
        Some(_) => diagnostics.push(error("blocks", "blocks is a list of {type, limit}".to_string())),
    }
    schema.max_blocks = map
        .get("maxBlocks")
        .and_then(Json::as_u64)
        .and_then(|limit| usize::try_from(limit).ok());
    schema.allowed_page_kinds = map
        .get("allowedPageKinds")
        .and_then(Json::as_array)
        .map(|kinds| kinds.iter().filter_map(Json::as_str).map(str::to_string).collect());
    schema.required = map.get("required").and_then(Json::as_bool).unwrap_or(false);
    match map.get("placeholder") {
        None | Some(Json::Null) => {}
        Some(Json::String(html)) => schema.placeholder = Some(html.clone()),
        Some(_) => diagnostics.push(error("placeholder", "placeholder is markup text".to_string())),
    }
    schema
}

/// A template file with its snippets put in where it includes them.
#[derive(Clone, Debug)]
pub(crate) struct Assembled {
    pub text: Arc<str>,
    /// The files it was made of: the file and its snippets.
    pub files: Vec<String>,
}

/// One `<#include "name">` of a template.
struct Include {
    start: usize,
    end: usize,
    name: String,
}

/// The `<#include "name">` directives of a source, outside comments. An include the adapter cannot read is an error
/// with its line.
fn includes(file: &str, text: &str) -> Result<Vec<Include>, Diagnostic> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(at) = text[index..].find("<#").map(|at| index + at) {
        let rest = &text[at..];
        if at > 0 && text.as_bytes()[at - 1] == b'\\' {
            index = at + 2;
        } else if rest.starts_with("<#--") {
            index = rest.find("-->").map_or(text.len(), |end| at + end + 3);
        } else if let Some(after) = rest.strip_prefix("<#include") {
            let line = template::line_of(text, at);
            let bad = || {
                Diagnostic::error(
                    "invalid-include",
                    file,
                    line,
                    "an include is <#include \"snippet-name\"> with the snippet's name in quotes",
                )
            };
            let after_space = after.trim_start();
            if after_space.len() == after.len() {
                return Err(bad());
            }
            let quoted = after_space.strip_prefix('"').ok_or_else(bad)?;
            let (name, tail) = quoted.split_once('"').ok_or_else(bad)?;
            let tail_trimmed = tail.trim_start();
            if !tail_trimmed.starts_with('>') || !is_name(name) {
                return Err(bad());
            }
            let end = text.len() - tail_trimmed.len() + 1;
            found.push(Include {
                start: at,
                end,
                name: name.to_string(),
            });
            index = end;
        } else {
            index = at + 2;
        }
    }
    Ok(found)
}

/// `text` with each include replaced by spaces, its newlines kept, so the rest keeps its lines and offsets.
fn blank_includes(text: &str, includes: &[Include]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for include in includes {
        out.push_str(&text[at..include.start]);
        for ch in text[include.start..include.end].chars() {
            out.push(if ch == '\n' { '\n' } else { ' ' });
        }
        at = include.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Assemble a template file: each include replaced by its snippet, itself assembled. A snippet that includes itself
/// (directly or through others) or includes deeper than `max_depth` is an error at the include's line.
pub(crate) fn assemble(package: &Package, file: &str, max_depth: usize) -> Result<Assembled, Diagnostic> {
    let mut files = Vec::new();
    let mut stack = Vec::new();
    let text = assemble_at(package, file, max_depth, &mut stack, &mut files)?;
    if text.len() > MAX_ASSEMBLED_BYTES {
        return Err(Diagnostic::error(
            "budget",
            file,
            0,
            format!("with its snippets put in, the template is over {MAX_ASSEMBLED_BYTES} bytes"),
        ));
    }
    Ok(Assembled {
        text: Arc::from(text),
        files,
    })
}

fn assemble_at(
    package: &Package,
    file: &str,
    max_depth: usize,
    stack: &mut Vec<String>,
    files: &mut Vec<String>,
) -> Result<String, Diagnostic> {
    let text = package
        .text(file)
        .ok_or_else(|| Diagnostic::error("missing-file", file, 0, format!("there is no {file}")))?;
    if !files.iter().any(|seen| seen == file) {
        files.push(file.to_string());
    }
    let found = includes(file, text)?;
    if found.is_empty() {
        return Ok(text.to_string());
    }
    stack.push(file.to_string());
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for include in &found {
        let line = template::line_of(text, include.start);
        let snippet = snippet_file(&include.name);
        if stack.contains(&snippet) {
            let chain = format!("{} > {snippet}", stack.join(" > "));
            return Err(Diagnostic::error(
                "recursion",
                file,
                line,
                format!("snippet `{}` includes itself: {chain}", include.name),
            ));
        }
        if stack.len() > max_depth {
            return Err(Diagnostic::error(
                "include-depth",
                file,
                line,
                format!("includes are nested deeper than {max_depth}"),
            ));
        }
        if package.text(&snippet).is_none() {
            return Err(Diagnostic::error(
                "missing-file",
                file,
                line,
                format!("there is no snippet `{}`", include.name),
            ));
        }
        out.push_str(&text[at..include.start]);
        // On an error the stack is left as it is: the whole assembly fails.
        out.push_str(&assemble_at(package, &snippet, max_depth, stack, files)?);
        at = include.end;
        if out.len() > MAX_ASSEMBLED_BYTES {
            break;
        }
    }
    out.push_str(&text[at..]);
    stack.pop();
    Ok(out)
}

/// The errors of a template with its snippets put in, when it has any. Each file is checked alone with its includes
/// blank, so an expression or tag that only the assembly makes (`${"` before an include and `"}` after it, with the
/// snippet between) is checked here, once the files pass alone. The lines are those of the assembled text.
pub(crate) fn check_assembled(
    package: &Package,
    file: &str,
    assembled: &Assembled,
    origins: &[String],
) -> Vec<Diagnostic> {
    if assembled.files.len() <= 1 {
        return Vec::new();
    }
    let alone_fails = assembled.files.iter().any(|used| {
        package.files.get(used).is_some_and(|text| {
            check_template_file(used, text, origins)
                .iter()
                .any(Diagnostic::is_error)
        })
    });
    if alone_fails {
        return Vec::new();
    }
    check_template_file(file, &assembled.text, origins)
        .iter()
        .filter(|diagnostic| diagnostic.is_error())
        .map(|diagnostic| Diagnostic {
            message: format!("with its snippets put in: {}", diagnostic.message),
            ..diagnostic.clone()
        })
        .collect()
}

type CheckCache = Mutex<crate::script::BoundedMap<(String, Arc<str>), Arc<[Diagnostic]>>>;

fn check_cache() -> &'static CheckCache {
    static CACHE: std::sync::OnceLock<CheckCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(crate::script::BoundedMap::new(4096)))
}

/// The findings of one template file alone (its includes blank): the dialect, each expression, and the markup with
/// its expressions as placeholders. Cached by file and text, as the live render checks every file it uses.
pub(crate) fn check_template_file(file: &str, text: &Arc<str>, origins: &[String]) -> Arc<[Diagnostic]> {
    let key = (file.to_string(), Arc::clone(text));
    if origins.is_empty() {
        if let Some(found) = lock(check_cache()).get(&key) {
            return found;
        }
    }
    let found: Arc<[Diagnostic]> = Arc::from(check_template_uncached(file, text, origins));
    if origins.is_empty() {
        lock(check_cache()).insert(key, Arc::clone(&found));
    }
    found
}

fn check_template_uncached(file: &str, text: &str, origins: &[String]) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    if let Some(at) = text.find(markup::is_reserved_char) {
        found.push(Diagnostic::error(
            "unsafe-markup",
            file,
            template::line_of(text, at),
            "the source holds a private-use character the adapter reserves",
        ));
        return found;
    }
    let include_list = match includes(file, text) {
        Ok(list) => list,
        Err(diagnostic) => return vec![diagnostic],
    };
    let blank = blank_includes(text, &include_list);
    let tokens = match template::tokens_at(&blank) {
        Ok(tokens) => tokens,
        Err(err) => {
            let message = err.message();
            let (line, message) = match message
                .strip_prefix("template line ")
                .and_then(|rest| rest.split_once(": "))
            {
                Some((line, rest)) => (line.parse().unwrap_or(0), rest.to_string()),
                None => (0, message.to_string()),
            };
            return vec![Diagnostic::error("syntax", file, line, message)];
        }
    };
    // Each expression, with its line; and the open tags, so an unclosed one is reported where it opens.
    let mut open: Vec<(&'static str, usize)> = Vec::new();
    for (offset, token) in &tokens {
        let line = template::line_of(&blank, *offset);
        let mut check = |what: &str, expression: &str| {
            for problem in expr::check(expression) {
                found.push(Diagnostic::error(
                    problem.code,
                    file,
                    line,
                    format!("{what}: {}", problem.message),
                ));
            }
        };
        match token {
            Token::Interp(expression) => check("${}", expression),
            Token::If(condition) => {
                check("<#if>", condition);
                open.push(("<#if>", line));
            }
            Token::ElseIf(condition) => {
                check("<#elseif>", condition);
                if !matches!(open.last(), Some(("<#if>", _))) {
                    found.push(Diagnostic::error(
                        "syntax",
                        file,
                        line,
                        "<#elseif> without an open <#if>",
                    ));
                }
            }
            Token::Else => {
                if !matches!(open.last(), Some(("<#if>", _))) {
                    found.push(Diagnostic::error("syntax", file, line, "<#else> without an open <#if>"));
                }
            }
            Token::EndIf => {
                if !matches!(open.pop(), Some(("<#if>", _))) {
                    found.push(Diagnostic::error("syntax", file, line, "</#if> without an open <#if>"));
                }
            }
            Token::List(items, name) => {
                check("<#list>", items);
                check_variable(name, file, line, &mut found);
                open.push(("<#list>", line));
            }
            Token::EndList => {
                if !matches!(open.pop(), Some(("<#list>", _))) {
                    found.push(Diagnostic::error(
                        "syntax",
                        file,
                        line,
                        "</#list> without an open <#list>",
                    ));
                }
            }
            Token::Assign(name, value) => {
                check("<#assign>", value);
                check_variable(name, file, line, &mut found);
            }
            Token::Text(_) | Token::Comment => {}
        }
    }
    for (tag, line) in open {
        found.push(Diagnostic::error("syntax", file, line, format!("{tag} is not closed")));
    }
    if found.iter().any(Diagnostic::is_error) {
        return found;
    }
    if let Some(renderer) = template::renderer_by_class("fancy-locked-html") {
        if let Err(err) = renderer.check(&blank) {
            found.push(Diagnostic::error("syntax", file, 0, err.message().to_string()));
            return found;
        }
    }
    // The markup with each expression as a placeholder and each tag as nothing, newlines kept.
    let mut markup_text = String::with_capacity(blank.len());
    for (index, (offset, token)) in tokens.iter().enumerate() {
        let end = tokens.get(index + 1).map_or(blank.len(), |(next, _)| *next);
        let span = &blank[*offset..end];
        match token {
            Token::Text(_) => markup_text.push_str(span),
            other => {
                if matches!(other, Token::Interp(_)) {
                    markup_text.push(markup::EXPRESSION_MARK);
                }
                markup_text.extend(span.chars().filter(|ch| *ch == '\n'));
            }
        }
    }
    let policy = markup::Policy { origins };
    for problem in markup::check_html(&markup_text, &markup::Mode::Source, &policy) {
        found.push(Diagnostic::error(problem.code, file, problem.line, problem.message));
    }
    found
}

fn check_variable(name: &str, file: &str, line: usize, found: &mut Vec<Diagnostic>) {
    if expr::is_denied_name(name) {
        found.push(Diagnostic::error(
            "forbidden-name",
            file,
            line,
            format!("`{name}` cannot be a variable of a theme template"),
        ));
    }
}

/// Every check of a package (design-shop §12.5's validation, less the representative renders that the caller makes
/// with `render`).
pub(crate) fn validate(package: &Package, options: &ValidateOptions) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let limits = &options.limits;
    let Some(manifest) = manifest(package, &mut diagnostics) else {
        return diagnostics;
    };
    if let Some(version) = options.api_version {
        let declared = package
            .text(MANIFEST)
            .and_then(|text| serde_json::from_str::<Json>(text).ok())
            .and_then(|json| json.get("viewApiVersion").and_then(Json::as_u64));
        if declared != Some(version) {
            diagnostics.push(Diagnostic::error(
                "invalid-manifest",
                MANIFEST,
                json_line(package.text(MANIFEST).unwrap_or_default(), "viewApiVersion"),
                format!("the caller assembles view API {version}; the theme declares {declared:?}"),
            ));
        }
    }
    let manifest_text = package.text(MANIFEST).unwrap_or_default();
    if let Some(layout) = &manifest.layout {
        if package.text(&layout_file(layout)).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-file",
                MANIFEST,
                json_line(manifest_text, "layout"),
                format!("there is no layout `{layout}`"),
            ));
        }
    }
    for style in &manifest.styles {
        if !style.starts_with("assets/") || !style.ends_with(".css") || package.text(style).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-file",
                MANIFEST,
                json_line(manifest_text, style),
                format!("style `{style}` is not a CSS asset of the package"),
            ));
        }
    }
    for (kind, name) in &manifest.default_templates {
        if package.text(&template_file(name)).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-file",
                MANIFEST,
                json_line(manifest_text, kind),
                format!("the default template of `{kind}` is `{name}`, which is not in the package"),
            ));
        }
    }
    for kind in &options.required_page_kinds {
        let name = manifest
            .default_templates
            .get(kind)
            .cloned()
            .unwrap_or_else(|| kind.clone());
        if package.text(&template_file(&name)).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-template",
                MANIFEST,
                0,
                format!("the theme has no template for page kind `{kind}`"),
            ));
        }
    }
    let paths: Vec<String> = package.paths().into_iter().map(str::to_string).collect();
    for path in &paths {
        validate_file(package, path, options, &mut diagnostics);
    }
    // Global settings.
    let mut settings_diagnostics = Vec::new();
    let defs = global_setting_defs(package, &mut settings_diagnostics);
    diagnostics.append(&mut settings_diagnostics);
    match json_file(package, SETTINGS_DATA) {
        Ok(Some(Json::Object(values))) => {
            let text = package.text(SETTINGS_DATA).unwrap_or_default();
            let has_asset = |path: &str| package.has(path);
            settings::check_values(&defs, &values, SETTINGS_DATA, text, &has_asset, None, &mut diagnostics);
        }
        Ok(Some(_)) => diagnostics.push(Diagnostic::error(
            "invalid-setting",
            SETTINGS_DATA,
            1,
            "settings.json is an object of values by setting id",
        )),
        Ok(None) => {}
        Err(diagnostic) => diagnostics.push(diagnostic),
    }
    // Page templates.
    for path in &paths {
        let Some(name) = path
            .strip_prefix("templates/")
            .and_then(|rest| rest.strip_suffix(".json"))
        else {
            continue;
        };
        validate_page_template(package, name, &manifest, limits, &mut diagnostics);
    }
    diagnostics
}

fn validate_file(package: &Package, path: &str, options: &ValidateOptions, diagnostics: &mut Vec<Diagnostic>) {
    let origins = &options.allowed_origins;
    if path.ends_with(TEMPLATE_EXTENSION) {
        let Some(text) = package.files.get(path) else {
            return;
        };
        diagnostics.extend(check_template_file(path, text, origins).iter().cloned());
        match assemble(package, path, options.limits.max_include_depth) {
            Ok(assembled) => diagnostics.extend(check_assembled(package, path, &assembled, origins)),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
        if let Some(kind) = path
            .strip_prefix("sections/")
            .or_else(|| path.strip_prefix("blocks/"))
            .and_then(|rest| rest.strip_suffix(TEMPLATE_EXTENSION))
        {
            let dir = if path.starts_with("sections/") {
                "sections"
            } else {
                "blocks"
            };
            let mut found = Vec::new();
            let schema = schema(package, dir, kind, &mut found);
            diagnostics.append(&mut found);
            if let Some(placeholder) = &schema.placeholder {
                let policy = markup::Policy { origins };
                if placeholder.contains("${") || placeholder.contains("<#") {
                    diagnostics.push(Diagnostic::error(
                        "invalid-schema",
                        &schema.file,
                        json_line(package.text(&schema.file).unwrap_or_default(), "placeholder"),
                        "a placeholder is static markup, with no expressions or tags",
                    ));
                }
                for problem in markup::check_html(placeholder, &markup::Mode::Source, &policy) {
                    diagnostics.push(Diagnostic::error(
                        problem.code,
                        &schema.file,
                        json_line(package.text(&schema.file).unwrap_or_default(), "placeholder"),
                        format!("placeholder: {}", problem.message),
                    ));
                }
            }
        }
    } else if let Some(kind) = path
        .strip_suffix(".schema.json")
        .and_then(|rest| rest.strip_prefix("sections/").or_else(|| rest.strip_prefix("blocks/")))
    {
        let dir = if path.starts_with("sections/") {
            "sections"
        } else {
            "blocks"
        };
        let template = format!("{dir}/{kind}{TEMPLATE_EXTENSION}");
        if package.text(&template).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-file",
                path,
                0,
                format!("the schema has no template {template}"),
            ));
        }
    } else if path.starts_with("locales/") {
        match json_file(package, path) {
            Ok(Some(Json::Object(_))) | Ok(None) => {}
            Ok(Some(_)) => diagnostics.push(Diagnostic::error(
                "invalid-json",
                path,
                1,
                "a locale file is an object of texts",
            )),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    } else if path.starts_with("assets/") {
        let Some(text) = package.text(path) else {
            return;
        };
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".css") {
            let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let resolve = |url: &str| package.has(&join_path(dir, url));
            let policy = markup::Policy { origins };
            for problem in markup::check_css(text, &policy, Some(&resolve)) {
                diagnostics.push(Diagnostic::error(problem.code, path, problem.line, problem.message));
            }
        } else if lower.ends_with(".svg") {
            let (_, problems) = svg::clean(text);
            for (line, message) in problems {
                diagnostics.push(Diagnostic::error("unsafe-svg", path, line, message));
            }
        }
    } else if path.starts_with("templates/") {
        if let Err(diagnostic) = json_file(package, path) {
            diagnostics.push(diagnostic);
        }
    }
}

/// `url` relative to the directory `dir`, as a package path; a URL with a scheme, a query or rooted at `/` is kept as
/// written so it matches no package path.
fn join_path(dir: &str, url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if path.starts_with('/') || path.contains(':') {
        return path.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The definitions of the global theme settings, `config/settings.schema.json`: `{settings: [...]}` or a list of
/// groups `[{name, settings: [...]}]`.
pub(crate) fn global_setting_defs(package: &Package, diagnostics: &mut Vec<Diagnostic>) -> Vec<SettingDef> {
    let json = match json_file(package, SETTINGS_SCHEMA) {
        Ok(Some(json)) => json,
        Ok(None) => return Vec::new(),
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            return Vec::new();
        }
    };
    let text = package.text(SETTINGS_SCHEMA).unwrap_or_default();
    let has_asset = |path: &str| package.has(path);
    match &json {
        Json::Array(groups) => {
            let all: Vec<Json> = groups
                .iter()
                .filter_map(|group| group.get("settings").and_then(Json::as_array))
                .flatten()
                .cloned()
                .collect();
            settings::parse_defs(Some(&Json::Array(all)), SETTINGS_SCHEMA, text, &has_asset, diagnostics)
        }
        Json::Object(map) => settings::parse_defs(map.get("settings"), SETTINGS_SCHEMA, text, &has_asset, diagnostics),
        _ => {
            diagnostics.push(Diagnostic::error(
                "invalid-schema",
                SETTINGS_SCHEMA,
                1,
                "the settings schema is {settings: [...]} or a list of groups",
            ));
            Vec::new()
        }
    }
}

/// The global setting values, `config/settings.json`, as an object.
pub(crate) fn global_setting_values(package: &Package) -> Map<String, Json> {
    match json_file(package, SETTINGS_DATA) {
        Ok(Some(Json::Object(values))) => values,
        _ => Map::new(),
    }
}

fn validate_page_template(
    package: &Package,
    name: &str,
    manifest: &Manifest,
    limits: &Limits,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let template = match page_template(package, name, diagnostics) {
        Ok(template) => template,
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            return;
        }
    };
    let file = template.file.clone();
    let text = package.text(&file).unwrap_or_default();
    let layout = match &template.layout {
        Some(layout) => layout.clone(),
        None => manifest.layout.clone(),
    };
    if let Some(layout) = layout {
        if package.text(&layout_file(&layout)).is_none() {
            diagnostics.push(Diagnostic::error(
                "missing-file",
                &file,
                json_line(text, "layout"),
                format!("there is no layout `{layout}`"),
            ));
        }
    }
    let rendered = template.sections.iter().filter(|section| !section.disabled).count();
    if rendered > limits.max_sections {
        diagnostics.push(Diagnostic::error(
            "budget",
            &file,
            0,
            format!("{rendered} sections; a page renders at most {}", limits.max_sections),
        ));
    }
    let has_asset = |path: &str| package.has(path);
    for section in &template.sections {
        let line = json_line(text, &section.id);
        if package.text(&section_file(&section.kind)).is_none() {
            diagnostics.push(
                Diagnostic::error(
                    "unknown-type",
                    &file,
                    line,
                    format!("section type `{}` is not in the package", section.kind),
                )
                .in_section(&section.id),
            );
            continue;
        }
        let mut found = Vec::new();
        let schema = schema(package, "sections", &section.kind, &mut found);
        if let Some(kinds) = &schema.allowed_page_kinds {
            if !kinds.iter().any(|kind| kind == template.page_kind()) {
                diagnostics.push(
                    Diagnostic::error(
                        "not-allowed",
                        &file,
                        line,
                        format!(
                            "section type `{}` is not allowed on a `{}` page",
                            section.kind,
                            template.page_kind()
                        ),
                    )
                    .in_section(&section.id),
                );
            }
        }
        let mut values = Vec::new();
        settings::check_values(
            &schema.settings,
            &section.settings,
            &file,
            text,
            &has_asset,
            Some(&section.id),
            &mut values,
        );
        diagnostics.extend(values.into_iter().map(|diagnostic| diagnostic.in_section(&section.id)));
        let mut count = 0;
        check_blocks(
            package,
            &file,
            text,
            &section.id,
            &schema,
            &section.blocks,
            3,
            &mut count,
            limits,
            diagnostics,
        );
        if count > limits.max_blocks {
            diagnostics.push(
                Diagnostic::error(
                    "budget",
                    &file,
                    line,
                    format!("{count} blocks; a section holds at most {}", limits.max_blocks),
                )
                .in_section(&section.id),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_blocks(
    package: &Package,
    file: &str,
    text: &str,
    section: &str,
    parent: &Schema,
    blocks: &[Instance],
    depth: usize,
    count: &mut usize,
    limits: &Limits,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if blocks.is_empty() {
        return;
    }
    if depth > limits.max_depth {
        diagnostics.push(
            Diagnostic::error(
                "budget",
                file,
                json_line(text, &blocks[0].id),
                format!("blocks are nested deeper than {}", limits.max_depth),
            )
            .in_section(section),
        );
        return;
    }
    if let Some(max) = parent.max_blocks {
        if blocks.len() > max {
            diagnostics.push(
                Diagnostic::error(
                    "budget",
                    file,
                    json_line(text, &blocks[0].id),
                    format!("{} blocks where {} allows {max}", blocks.len(), parent.file),
                )
                .in_section(section),
            );
        }
    }
    let has_asset = |path: &str| package.has(path);
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for block in blocks {
        *count += 1;
        let line = json_line(text, &block.id);
        let error = |code: &'static str, message: String| {
            Diagnostic::error(code, file, line, message)
                .in_section(section)
                .in_block(&block.id)
        };
        let Some(allow) = parent.blocks.iter().find(|allow| allow.kind == block.kind) else {
            diagnostics.push(error(
                "not-allowed",
                format!("block type `{}` is not one {} allows", block.kind, parent.file),
            ));
            continue;
        };
        let seen = by_kind.entry(&block.kind).or_default();
        *seen += 1;
        if allow.limit.is_some_and(|limit| *seen > limit) {
            diagnostics.push(error(
                "budget",
                format!(
                    "more `{}` blocks than the {} {} allows",
                    block.kind,
                    allow.limit.unwrap_or_default(),
                    parent.file
                ),
            ));
        }
        if package.text(&block_file(&block.kind)).is_none() {
            diagnostics.push(error(
                "unknown-type",
                format!("block type `{}` is not in the package", block.kind),
            ));
            continue;
        }
        let mut found = Vec::new();
        let schema = schema(package, "blocks", &block.kind, &mut found);
        let mut values = Vec::new();
        settings::check_values(
            &schema.settings,
            &block.settings,
            file,
            text,
            &has_asset,
            Some(&block.id),
            &mut values,
        );
        diagnostics.extend(
            values
                .into_iter()
                .map(|diagnostic| diagnostic.in_section(section).in_block(&block.id)),
        );
        check_blocks(
            package,
            file,
            text,
            section,
            &schema,
            &block.blocks,
            depth + 1,
            count,
            limits,
            diagnostics,
        );
    }
}

/// The keys given twice in one object of a JSON text. serde_json keeps the last silently, so they are found on a
/// second pass.
fn duplicate_keys(text: &str) -> Vec<String> {
    use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

    struct Dups<'a>(&'a mut Vec<String>);

    impl<'de> DeserializeSeed<'de> for Dups<'_> {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            deserializer.deserialize_any(self)
        }
    }

    impl<'de> Visitor<'de> for Dups<'_> {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("JSON")
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(Dups(&mut *self.0))?.is_some() {}
            Ok(())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key.clone()) {
                    self.0.push(key);
                }
                map.next_value_seed(Dups(&mut *self.0))?;
            }
            Ok(())
        }
    }

    let mut found = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let _ = Dups(&mut found).deserialize(&mut deserializer);
    found
}
