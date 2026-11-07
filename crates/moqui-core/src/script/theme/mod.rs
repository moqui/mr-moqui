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

//! The theme composition adapter on the locked renderer: a merchant theme package (layout, JSON templates, sections,
//! blocks, snippets, typed settings, assets) validated and rendered with budgets, for a public storefront such as
//! IronStone Shop (mr-pass `plan/design-shop.md` §12, impl-shop P2 and F10). The guide is `docs/ui/Themes.md`.
//!
//! - The package is data: its files as text, handed in by the caller. The adapter resolves layouts, templates,
//!   sections, blocks and snippets itself (`package`), so no template names a file or reaches a resource.
//! - Each layout, section and block renders in the locked renderer (`template::render_locked_bounded`), which sees
//!   only its snapshot, `format` and `localize`, with the page's deadline and memory.
//! - Source is checked before it runs (`expr`, `markup`), and each render's output is checked again in its HTML, URL,
//!   CSS and attribute contexts, so markup that an expression builds is refused as literal markup is.
//! - Adapter-made fragments (blocks in a section, sections in a layout, sanitized rich text, trusted components) reach a
//!   template as opaque placeholders and are put in after the checks (`compose`). Nothing else is output unescaped.
//!
//! Engines are registered by id (`js-fancy` today); a package's `theme.json` names its engine.

mod compose;
mod expr;
mod markup;
mod package;
mod settings;
mod svg;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;

pub use markup::sanitize_rich_text;
pub use package::{BinaryAsset, Package};
pub use svg::sanitize_svg;

/// The id of the first engine: the fancy dialect in the locked renderer with this adapter.
pub const JS_FANCY: &str = "js-fancy";

/// The view API versions this framework renders.
pub const VIEW_API_VERSIONS: &[u64] = &[1];

/// The version of the `js-fancy` renderer: raised when the same package and view may render differently (the
/// renderer's dialect, the adapter's composition or checks), so an app keys what it keeps of a render by it.
pub const JS_FANCY_RENDERER_VERSION: u64 = 1;

/// The budgets of a page (design-shop §12.4). The defaults are the ceilings: operator conf and a request may lower them,
/// never raise them, and the renderer's own deadline and memory cap them again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Sections rendered on one page.
    pub max_sections: usize,
    /// Blocks in one section, nested blocks included.
    pub max_blocks: usize,
    /// Composition depth: the layout is 1, a section 2, a block 3 and a nested block 4 and on.
    pub max_depth: usize,
    /// Snippet includes inside one another.
    pub max_include_depth: usize,
    /// Bytes of the page's HTML once its fragments are put in.
    pub max_output_bytes: usize,
    /// The time of one render: a layout, a section or a block.
    pub render_time: Duration,
    /// The time of the whole page.
    pub page_time: Duration,
    /// The locked sandbox's memory during a render.
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        let (time, memory) = crate::script::template::LOCKED_BUDGET;
        Self {
            max_sections: 100,
            max_blocks: 100,
            max_depth: 8,
            max_include_depth: 8,
            max_output_bytes: 2 * 1024 * 1024,
            render_time: Duration::from_secs(1).min(time),
            page_time: time,
            memory_bytes: memory,
        }
    }
}

impl Limits {
    /// The operator's limits: the `theme-limits` element of the resource facade's conf, each value lowered to its
    /// default when set higher.
    pub fn operator(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.conf_derived(|factory| Self::from_conf(&factory.conf_text()))
    }

    fn from_conf(conf_text: &str) -> Self {
        let mut limits = Self::default();
        let Ok(root) = moqui_lib::node::MNode::parse_shared("moqui-conf", conf_text) else {
            return limits;
        };
        for facade in root.named_iter("resource-facade") {
            for entry in facade.named_iter("theme-limits") {
                let pairs: Vec<(String, Json)> = [
                    "max-sections",
                    "max-blocks",
                    "max-depth",
                    "max-include-depth",
                    "max-output-bytes",
                    "render-time-ms",
                    "page-time-ms",
                    "memory-bytes",
                ]
                .iter()
                .filter_map(|name| {
                    let text = entry.attribute(name)?;
                    match text.trim().parse::<u64>() {
                        Ok(number) => Some((camel(name), json!(number))),
                        Err(_) => {
                            tracing::error!(
                                "theme-limits {name}=\"{text}\" is not a whole number; the default is used"
                            );
                            None
                        }
                    }
                })
                .collect();
                limits = limits.lowered_by(&Json::Object(pairs.into_iter().collect()));
            }
        }
        limits
    }

    /// These limits with each value of `overrides` (the camelCase names of `docs/ui/Themes.md`) that is lower.
    /// A value above the current one is ignored: a caller can only tighten.
    pub fn lowered_by(&self, overrides: &Json) -> Self {
        let mut out = self.clone();
        let Some(map) = overrides.as_object() else {
            return out;
        };
        let number = |name: &str| map.get(name).and_then(Json::as_u64);
        let lower = |current: &mut usize, name: &str| {
            if let Some(value) = number(name).and_then(|value| usize::try_from(value).ok()) {
                *current = (*current).min(value);
            }
        };
        lower(&mut out.max_sections, "maxSections");
        lower(&mut out.max_blocks, "maxBlocks");
        lower(&mut out.max_depth, "maxDepth");
        lower(&mut out.max_include_depth, "maxIncludeDepth");
        lower(&mut out.max_output_bytes, "maxOutputBytes");
        lower(&mut out.memory_bytes, "memoryBytes");
        if let Some(ms) = number("renderTimeMs") {
            out.render_time = out.render_time.min(Duration::from_millis(ms));
        }
        if let Some(ms) = number("pageTimeMs") {
            out.page_time = out.page_time.min(Duration::from_millis(ms));
        }
        out
    }

    pub fn to_json(&self) -> Json {
        json!({
            "maxSections": self.max_sections,
            "maxBlocks": self.max_blocks,
            "maxDepth": self.max_depth,
            "maxIncludeDepth": self.max_include_depth,
            "maxOutputBytes": self.max_output_bytes,
            "renderTimeMs": u64::try_from(self.render_time.as_millis()).unwrap_or(u64::MAX),
            "pageTimeMs": u64::try_from(self.page_time.as_millis()).unwrap_or(u64::MAX),
            "memoryBytes": self.memory_bytes,
        })
    }
}

/// `max-sections` as `maxSections`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for ch in name.chars() {
        if ch == '-' {
            upper = true;
        } else if upper {
            out.push(ch.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One finding: what file and line, which section or block of the page, and why. A diagnostic is for the theme's
/// developer (preview, Commerce logs); a public page shows a safe message instead (design-shop §12.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// A stable code: `forbidden-name`, `unsafe-markup`, `unsafe-url`, `unsafe-css`, `recursion`, `budget`,
    /// `invalid-setting`, `render-failed` and the others listed in `docs/ui/Themes.md`.
    pub code: &'static str,
    /// The package file, such as `sections/hero.html.fancy-locked`; empty for the package as a whole.
    pub file: String,
    /// The line in that file from 1, or 0 when the finding is not at a line (a rendered output, a missing file).
    pub line: usize,
    pub message: String,
    /// The section instance of the page template, when the finding is about one.
    pub section: Option<Box<str>>,
    /// The block instance, when the finding is about one.
    pub block: Option<Box<str>>,
}

impl Diagnostic {
    pub(crate) fn error(code: &'static str, file: &str, line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            file: file.to_string(),
            line,
            message: message.into(),
            section: None,
            block: None,
        }
    }

    pub(crate) fn warning(code: &'static str, file: &str, line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, file, line, message)
        }
    }

    pub(crate) fn in_section(mut self, section: &str) -> Self {
        self.section = Some(section.into());
        self
    }

    pub(crate) fn in_block(mut self, block: &str) -> Self {
        self.block = Some(block.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn to_json(&self) -> Json {
        let mut out = json!({
            "severity": if self.is_error() { "error" } else { "warning" },
            "code": self.code,
            "file": self.file,
            "line": self.line,
            "message": self.message,
        });
        if let Some(section) = &self.section {
            out["section"] = json!(section);
        }
        if let Some(block) = &self.block {
            out["block"] = json!(block);
        }
        out
    }
}

/// What `validate` checks a package against, besides its own files.
#[derive(Clone, Debug, Default)]
pub struct ValidateOptions {
    /// The page kinds the caller's app needs a template for (`home`, `product`...): a missing one is an error.
    pub required_page_kinds: Vec<String>,
    /// The view API version the caller assembles; checked against `theme.json` and the engine.
    pub api_version: Option<u64>,
    /// Origins besides the page's own that a `src` or CSS `url()` may load from.
    pub allowed_origins: Vec<String>,
    pub limits: Limits,
}

/// The result of `validate`.
#[derive(Clone, Debug, Default)]
pub struct Validation {
    pub engine: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        !self.diagnostics.iter().any(Diagnostic::is_error)
    }

    pub fn to_json(&self) -> Json {
        json!({
            "ok": self.ok(),
            "engine": self.engine,
            "diagnostics": self.diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
        })
    }
}

/// One page to render. Every field is plain data the caller has approved; see `docs/ui/Themes.md` for each.
#[derive(Clone, Debug, Default)]
pub struct RenderRequest {
    /// The template to render: `product` for `templates/product.json`, `product.featured` for an alternate.
    pub template: Option<String>,
    /// The page kind, which picks `theme.json`'s default template when `template` is not given.
    pub page_kind: Option<String>,
    /// The view (design-shop §12.3's contract: `site`, `shop`, `locale`, `page`, `product`...).
    pub view: Json,
    /// Trusted fragments by component name, made by installed code: `{html, required}` or `{error, required}`.
    pub components: Json,
    /// Resolved references of `resourceLink`, `product`, `category` and `page` settings, by `kind:id`.
    pub references: Json,
    /// Resolved media of `image` settings, by media id: `{url, alt, width, height}`.
    pub media: Json,
    /// Approved dynamic source data of `source` settings, by source name.
    pub sources: Json,
    /// The URL of each package asset, by path; `asset_base` is the prefix of the rest.
    pub asset_urls: Json,
    pub asset_base: Option<String>,
    /// Origins (`https://cdn.example.com`) besides the page's own that a `src` or CSS `url()` may load from.
    pub allowed_origins: Vec<String>,
    /// For log lines: the release this package is.
    pub release_id: Option<String>,
    pub limits: Limits,
}

/// A section or block left out of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Omitted {
    pub section: String,
    pub block: Option<String>,
    pub reason: String,
}

/// The result of `render`. `ok` is false when the page cannot be used for a purchase: the layout or template failed
/// (then `html` is empty), or a required section or component failed (then `html` is the rest of the page).
#[derive(Clone, Debug, Default)]
pub struct Rendered {
    pub ok: bool,
    pub html: String,
    pub diagnostics: Vec<Diagnostic>,
    pub omitted: Vec<Omitted>,
    /// Required sections and components that failed or are not on the page, by name.
    pub failed_required: Vec<String>,
    /// The package files the page used.
    pub dependencies: Vec<String>,
}

impl Rendered {
    pub fn to_json(&self) -> Json {
        json!({
            "ok": self.ok,
            "html": self.html,
            "diagnostics": self.diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
            "omitted": self.omitted.iter().map(|omitted| {
                let mut out = json!({"section": omitted.section, "reason": omitted.reason});
                if let Some(block) = &omitted.block {
                    out["block"] = json!(block);
                }
                out
            }).collect::<Vec<_>>(),
            "failedRequired": self.failed_required,
            "dependencies": self.dependencies,
        })
    }
}

/// A theme engine: one template syntax with its composition rules, by id.
pub trait ThemeEngine: Send + Sync {
    fn id(&self) -> &'static str;

    /// The view API versions the engine renders.
    fn view_api_versions(&self) -> &'static [u64];

    /// The renderer's version: a new one may render the same package and view differently.
    fn renderer_version(&self) -> u64;

    /// Check a package: its manifest, schemas, settings, templates and assets, with a file and line for each finding.
    fn validate(&self, package: &Package, options: &ValidateOptions) -> Validation;

    /// Render one page of a package.
    fn render(&self, package: &Package, request: &RenderRequest) -> Rendered;
}

/// The `js-fancy` engine.
pub struct JsFancy;

impl ThemeEngine for JsFancy {
    fn id(&self) -> &'static str {
        JS_FANCY
    }

    fn view_api_versions(&self) -> &'static [u64] {
        VIEW_API_VERSIONS
    }

    fn renderer_version(&self) -> u64 {
        JS_FANCY_RENDERER_VERSION
    }

    fn validate(&self, package: &Package, options: &ValidateOptions) -> Validation {
        Validation {
            engine: JS_FANCY.to_string(),
            diagnostics: package::validate(package, options),
        }
    }

    fn render(&self, package: &Package, request: &RenderRequest) -> Rendered {
        compose::render(package, request)
    }
}

static ENGINES: &[&dyn ThemeEngine] = &[&JsFancy];

/// The registered engine with this id.
pub fn engine(id: &str) -> Option<&'static dyn ThemeEngine> {
    ENGINES.iter().copied().find(|engine| engine.id() == id)
}

/// The ids of the registered engines.
pub fn engine_ids() -> Vec<&'static str> {
    ENGINES.iter().map(|engine| engine.id()).collect()
}

/// Each registered engine with its renderer and view API versions: `[{id, rendererVersion, viewApiVersions}]`.
pub fn engine_info() -> Json {
    Json::Array(
        ENGINES
            .iter()
            .map(|engine| {
                json!({
                    "id": engine.id(),
                    "rendererVersion": engine.renderer_version(),
                    "viewApiVersions": engine.view_api_versions(),
                })
            })
            .collect(),
    )
}

/// The engine `theme.json` names, or the diagnostic that says why there is none.
fn engine_of(package: &Package) -> Result<&'static dyn ThemeEngine, Diagnostic> {
    let manifest = package::manifest_json(package)?;
    let id = manifest.get("engine").and_then(Json::as_str).unwrap_or(JS_FANCY);
    engine(id).ok_or_else(|| {
        Diagnostic::error(
            "unknown-engine",
            package::MANIFEST,
            0,
            format!(
                "engine `{id}` is not registered; the engines are {}",
                engine_ids().join(", ")
            ),
        )
    })
}

/// Validate a package with the engine its `theme.json` names.
pub fn validate(package: &Package, options: &ValidateOptions) -> Validation {
    let mut diagnostics = package.load_diagnostics().to_vec();
    match engine_of(package) {
        Ok(engine) => {
            let mut found = engine.validate(package, options);
            diagnostics.append(&mut found.diagnostics);
            Validation {
                engine: found.engine,
                diagnostics,
            }
        }
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            Validation {
                engine: String::new(),
                diagnostics,
            }
        }
    }
}

/// Render a page with the engine its `theme.json` names.
pub fn render(package: &Package, request: &RenderRequest) -> Rendered {
    match engine_of(package) {
        Ok(engine) => engine.render(package, request),
        Err(diagnostic) => Rendered {
            diagnostics: vec![diagnostic],
            ..Rendered::default()
        },
    }
}

impl ValidateOptions {
    /// Options from the JSON a script passes: `{requiredPageKinds, apiVersion, limits}`, with `limits` lowering
    /// `base`.
    pub fn from_json(options: &Json, base: &Limits) -> Self {
        Self {
            required_page_kinds: string_list(options.get("requiredPageKinds")),
            api_version: options.get("apiVersion").and_then(Json::as_u64),
            allowed_origins: string_list(options.get("allowedOrigins")),
            limits: base.lowered_by(options.get("limits").unwrap_or(&Json::Null)),
        }
    }
}

impl RenderRequest {
    /// A request from the JSON a script passes (`docs/ui/Themes.md`), with `limits` lowering `base`.
    pub fn from_json(request: &Json, base: &Limits) -> Self {
        let field = |name: &str| request.get(name).cloned().unwrap_or(Json::Null);
        let text = |name: &str| {
            request
                .get(name)
                .and_then(Json::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Self {
            template: text("template"),
            page_kind: text("pageKind"),
            view: field("view"),
            components: field("components"),
            references: field("references"),
            media: field("media"),
            sources: field("sources"),
            asset_urls: field("assetUrls"),
            asset_base: text("assetBase"),
            allowed_origins: string_list(request.get("allowedOrigins")),
            release_id: text("releaseId"),
            limits: base.lowered_by(request.get("limits").unwrap_or(&Json::Null)),
        }
    }
}

fn string_list(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
