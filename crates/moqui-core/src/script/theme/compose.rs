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

//! One page composed and rendered (design-shop §12.4's request flow from "choose template" on): the template's
//! sections in order, each with its blocks, then the layout around them. Each render is checked, and what a render
//! makes becomes a fragment its parent places by placeholder. An optional section or block that fails is left out and
//! logged; a required one, the layout, or a missing required component makes the page not `ok`.

use std::collections::{BTreeSet, HashSet};
use std::time::Instant;

use serde_json::{json, Map, Value as Json};

use super::markup::{self, Mode, Policy, FRAGMENT_CLOSE, FRAGMENT_OPEN};
use super::package::{self, Instance, Manifest, Package, PageTemplate};
use super::settings::{self, Resolve};
use super::{Diagnostic, Limits, Omitted, RenderRequest, Rendered};
use crate::script::template;

/// The page being rendered.
struct Page<'a> {
    package: &'a Package,
    request: &'a RenderRequest,
    limits: &'a Limits,
    nonce: String,
    /// Each fragment's HTML, and its length with its own fragments put in.
    fragments: Vec<(String, usize)>,
    deadline: Instant,
    diagnostics: Vec<Diagnostic>,
    omitted: Vec<Omitted>,
    failed_required: Vec<String>,
    dependencies: BTreeSet<String>,
    template_name: String,
    /// The base view every render starts from.
    base: Map<String, Json>,
    /// The component placeholders, by name.
    components: Vec<(String, String, bool)>,
}

/// Why one render failed.
struct Failure {
    file: String,
    message: String,
    diagnostics: Vec<Diagnostic>,
}

impl Failure {
    fn new(file: &str, message: impl Into<String>) -> Self {
        Self {
            file: file.to_string(),
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }
}

pub(crate) fn render(package: &Package, request: &RenderRequest) -> Rendered {
    let limits = &request.limits;
    let mut setup = Vec::new();
    setup.extend(package.load_diagnostics().iter().cloned());
    let Some(manifest) = package::manifest(package, &mut setup) else {
        return failed(setup);
    };
    let name = match (&request.template, &request.page_kind) {
        (Some(name), _) => name.clone(),
        (None, Some(kind)) => manifest
            .default_templates
            .get(kind)
            .cloned()
            .unwrap_or_else(|| kind.clone()),
        (None, None) => {
            setup.push(Diagnostic::error(
                "missing-template",
                "",
                0,
                "the request names no template or page kind",
            ));
            return failed(setup);
        }
    };
    if !package::is_name(&name) {
        setup.push(Diagnostic::error(
            "missing-template",
            "",
            0,
            format!("`{name}` is not a template name"),
        ));
        return failed(setup);
    }
    let template = match package::page_template(package, &name, &mut setup) {
        Ok(template) => template,
        Err(diagnostic) => {
            setup.push(diagnostic);
            return failed(setup);
        }
    };
    let mut page = Page {
        package,
        request,
        limits,
        nonce: nonce(),
        fragments: Vec::new(),
        deadline: Instant::now() + limits.page_time,
        diagnostics: setup.into_iter().filter(|diagnostic| !diagnostic.is_error()).collect(),
        omitted: Vec::new(),
        failed_required: Vec::new(),
        dependencies: BTreeSet::new(),
        template_name: name,
        base: Map::new(),
        components: Vec::new(),
    };
    page.dependencies.insert(package::MANIFEST.to_string());
    page.dependencies.insert(template.file.clone());
    page.compose(&manifest, &template)
}

fn failed(diagnostics: Vec<Diagnostic>) -> Rendered {
    Rendered {
        ok: false,
        diagnostics,
        ..Rendered::default()
    }
}

/// A random hex nonce for this page's placeholders.
fn nonce() -> String {
    let mut bytes = [0u8; 12];
    if getrandom::fill(&mut bytes).is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        bytes[..8].copy_from_slice(&(now as u64).to_le_bytes());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// View data with the adapter's reserved characters removed, and each `{"$richText": html}` replaced by the
/// placeholder of the sanitized HTML.
fn clean_view(value: &Json, page: &mut Page<'_>) -> Json {
    match value {
        Json::String(text) => Json::String(markup::strip_reserved(text)),
        Json::Array(items) => Json::Array(items.iter().map(|item| clean_view(item, page)).collect()),
        Json::Object(map) => {
            if let (1, Some(Json::String(html))) = (map.len(), map.get("$richText")) {
                let clean = markup::sanitize_rich_text(html);
                return Json::String(page.fragment(clean));
            }
            Json::Object(
                map.iter()
                    .map(|(key, item)| (markup::strip_reserved(key), clean_view(item, page)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

impl Page<'_> {
    /// Keep `html` as a fragment and return its placeholder.
    fn fragment(&mut self, html: String) -> String {
        let size = self.expanded_len(&html);
        let index = self.fragments.len();
        self.fragments.push((html, size));
        format!("{FRAGMENT_OPEN}{}:{index}{FRAGMENT_CLOSE}", self.nonce)
    }

    /// The length of `text` with its fragments put in.
    fn expanded_len(&self, text: &str) -> usize {
        let Ok(refs) = markup::fragment_refs(text, &self.nonce) else {
            return text.len();
        };
        let mut size = text.len();
        for found in refs {
            size = size - (found.end - found.start) + self.fragments.get(found.index).map_or(0, |(_, length)| *length);
        }
        size
    }

    /// `text` with its fragments put in, recursively, within the output budget.
    fn expand(&self, text: &str, out: &mut String) -> Result<(), String> {
        let refs = markup::fragment_refs(text, &self.nonce)?;
        let mut at = 0;
        for found in refs {
            out.push_str(&text[at..found.start]);
            let (inner, _) = self
                .fragments
                .get(found.index)
                .ok_or("a fragment that does not exist")?;
            self.expand(inner, out)?;
            at = found.end;
            if out.len() > self.limits.max_output_bytes {
                return Err(format!("the page is over {} bytes", self.limits.max_output_bytes));
            }
        }
        out.push_str(&text[at..]);
        if out.len() > self.limits.max_output_bytes {
            return Err(format!("the page is over {} bytes", self.limits.max_output_bytes));
        }
        Ok(())
    }

    fn log_name(&self) -> String {
        format!(
            "release {} template {}",
            self.request.release_id.as_deref().unwrap_or("(none)"),
            self.template_name
        )
    }

    /// The URL of a package asset.
    fn asset_url(&self, path: &str) -> String {
        if let Some(url) = self.request.asset_urls.get(path).and_then(Json::as_str) {
            return url.to_string();
        }
        match &self.request.asset_base {
            Some(base) => format!(
                "{}/{}",
                base.trim_end_matches('/'),
                path.strip_prefix("assets/").unwrap_or(path)
            ),
            None => path.to_string(),
        }
    }

    fn compose(mut self, manifest: &Manifest, template: &PageTemplate) -> Rendered {
        // The base view: the caller's view, the theme's global settings, the components and the asset URLs.
        let view = self.request.view.clone();
        let mut base = match clean_view(&view, &mut self) {
            Json::Object(map) => map,
            _ => Map::new(),
        };
        let mut components = Map::new();
        if let Some(given) = self.request.components.as_object() {
            for (name, component) in given {
                let required = component.get("required").and_then(Json::as_bool).unwrap_or(false)
                    || manifest.required_components.contains(name);
                match component.get("html").and_then(Json::as_str) {
                    Some(html) if component.get("error").is_none_or(Json::is_null) => {
                        let placeholder = self.fragment(markup::strip_reserved(html));
                        components.insert(name.clone(), json!(placeholder));
                        self.components.push((name.clone(), placeholder, required));
                    }
                    _ => {
                        let reason = component
                            .get("error")
                            .and_then(Json::as_str)
                            .unwrap_or("it gave no HTML")
                            .to_string();
                        components.insert(name.clone(), Json::Null);
                        let message = format!("component `{name}` failed: {reason}");
                        if required {
                            self.failed_required.push(format!("component:{name}"));
                            self.diagnostics
                                .push(Diagnostic::error("required-failed", "", 0, message));
                        } else {
                            self.diagnostics
                                .push(Diagnostic::warning("render-failed", "", 0, message));
                        }
                    }
                }
            }
        }
        base.insert("components".to_string(), Json::Object(components));
        let assets: Map<String, Json> = self
            .package
            .assets()
            .into_iter()
            .map(|path| (path.to_string(), json!(self.asset_url(path))))
            .collect();
        base.insert("assets".to_string(), Json::Object(assets));
        base.insert("theme".to_string(), json!({"name": manifest.name}));
        let mut global_found = Vec::new();
        let defs = package::global_setting_defs(self.package, &mut global_found);
        let values = package::global_setting_values(self.package);
        let mut warnings = Vec::new();
        let mut resolver = Resolver { page: &mut self };
        let globals = settings::resolve(&defs, &values, &mut resolver, &mut warnings);
        match globals {
            Ok(globals) => {
                base.insert("settings".to_string(), globals);
            }
            Err(message) => {
                self.diagnostics
                    .push(Diagnostic::error("invalid-setting", "config/settings.json", 0, message));
                return self.finish(None);
            }
        }
        for warning in warnings {
            self.diagnostics
                .push(Diagnostic::warning("unresolved", "config/settings.json", 0, warning));
        }
        self.base = base;

        // The layout first, around a fragment the sections fill, so the sections' output budget is what the layout
        // leaves.
        let content_index = self.fragments.len();
        self.fragments.push((String::new(), 0));
        let content_placeholder = format!("{FRAGMENT_OPEN}{}:{content_index}{FRAGMENT_CLOSE}", self.nonce);
        let layout = match &template.layout {
            Some(layout) => layout.clone(),
            None => manifest.layout.clone(),
        };
        let mut layout_html = None;
        if let Some(layout) = layout {
            let file = package::layout_file(&layout);
            let mut styles = String::new();
            for style in &manifest.styles {
                styles.push_str(&format!(
                    "<link rel=\"stylesheet\" href=\"{}\">\n",
                    html_escape(&self.asset_url(style))
                ));
                self.dependencies.insert(style.clone());
            }
            let styles = self.fragment(styles);
            let mut view = self.base.clone();
            view.insert("content".to_string(), json!(content_placeholder));
            view.insert("styles".to_string(), json!(styles));
            match self.render_file(&file, &Json::Object(view)) {
                Ok(html) => layout_html = Some(html),
                Err(failure) => {
                    self.diagnostics.extend(failure.diagnostics);
                    self.diagnostics.push(Diagnostic::error(
                        "render-failed",
                        &failure.file,
                        0,
                        format!("the layout failed: {}", failure.message),
                    ));
                    tracing::error!("Theme {} layout {layout} failed: {}", self.log_name(), failure.message);
                    return self.finish(None);
                }
            }
        }
        let budget = self
            .limits
            .max_output_bytes
            .saturating_sub(layout_html.as_deref().map_or(0, |html| self.expanded_len(html)));

        // Sections, in order.
        let mut content = String::new();
        let mut content_size = 0usize;
        let mut count = 0usize;
        for section in template.sections.iter().filter(|section| !section.disabled) {
            count += 1;
            if count > self.limits.max_sections {
                self.leave_out(
                    section,
                    None,
                    &format!("over {} sections on the page", self.limits.max_sections),
                    false,
                );
                continue;
            }
            let mut schema_found = Vec::new();
            let schema = package::schema(self.package, "sections", &section.kind, &mut schema_found);
            let required = section.required || schema.required;
            match self.section(section, &schema, template) {
                Ok(html) => {
                    let size = self.expanded_len(&html);
                    if content_size + size > budget {
                        let reason = format!(
                            "its output would pass the page's {} bytes",
                            self.limits.max_output_bytes
                        );
                        self.leave_out(section, Some(&schema), &reason, required);
                        continue;
                    }
                    content_size += size;
                    content.push_str(&self.fragment(html));
                }
                Err(failure) => {
                    let Failure {
                        file,
                        message,
                        diagnostics,
                    } = failure;
                    self.diagnostics.extend(
                        diagnostics
                            .into_iter()
                            .map(|diagnostic| diagnostic.in_section(&section.id)),
                    );
                    self.diagnostics
                        .push(Diagnostic::warning("render-failed", &file, 0, message.clone()).in_section(&section.id));
                    if let Some(placeholder) = self.leave_out(section, Some(&schema), &message, required) {
                        content.push_str(&placeholder);
                    }
                }
            }
        }
        let content_size = self.expanded_len(&content);
        self.fragments[content_index] = (content, content_size);
        let page = match layout_html {
            Some(html) => self.fragment(html),
            None => content_placeholder,
        };
        self.finish(Some(page))
    }

    /// Leave a section out: log it, record it, and give the schema's neutral placeholder when it has one. A required
    /// section is recorded as failed.
    fn leave_out(
        &mut self,
        section: &Instance,
        schema: Option<&package::Schema>,
        reason: &str,
        required: bool,
    ) -> Option<String> {
        tracing::error!(
            "Theme {} section {} ({}) left out: {reason}",
            self.log_name(),
            section.id,
            section.kind
        );
        self.omitted.push(Omitted {
            section: section.id.clone(),
            block: None,
            reason: reason.to_string(),
        });
        if required {
            self.failed_required.push(format!("section:{}", section.id));
            self.diagnostics.push(
                Diagnostic::error(
                    "required-failed",
                    "",
                    0,
                    format!("required section `{}` failed: {reason}", section.id),
                )
                .in_section(&section.id),
            );
        }
        let placeholder = schema.and_then(|schema| schema.placeholder.clone())?;
        let none = HashSet::new();
        let policy = Policy {
            origins: &self.request.allowed_origins,
        };
        let mode = Mode::Output {
            nonce: &self.nonce,
            allowed: &none,
        };
        if placeholder.contains("${") || !markup::check_html(&placeholder, &mode, &policy).is_empty() {
            return None;
        }
        Some(self.fragment(placeholder))
    }

    fn section(
        &mut self,
        section: &Instance,
        schema: &package::Schema,
        template: &PageTemplate,
    ) -> Result<String, Failure> {
        let file = package::section_file(&section.kind);
        if self.package.text(&file).is_none() {
            return Err(Failure::new(
                &file,
                format!("section type `{}` is not in the package", section.kind),
            ));
        }
        if let Some(kinds) = &schema.allowed_page_kinds {
            if !kinds.iter().any(|kind| kind == template.page_kind()) {
                return Err(Failure::new(
                    &file,
                    format!(
                        "section type `{}` is not allowed on a `{}` page",
                        section.kind,
                        template.page_kind()
                    ),
                ));
            }
        }
        let settings = self.settings(schema, &section.settings, &file, &section.id)?;
        let mut count = 0;
        let blocks = self.blocks(section, schema, &section.blocks, 3, &mut count)?;
        let mut view = self.base.clone();
        view.insert(
            "section".to_string(),
            json!({"id": section.id, "type": section.kind, "settings": settings, "blocks": blocks}),
        );
        view.insert("blocks".to_string(), blocks);
        self.render_file(&file, &Json::Object(view))
    }

    fn settings(
        &mut self,
        schema: &package::Schema,
        values: &Map<String, Json>,
        file: &str,
        id: &str,
    ) -> Result<Json, Failure> {
        let mut warnings = Vec::new();
        let mut resolver = Resolver { page: self };
        let resolved = settings::resolve(&schema.settings, values, &mut resolver, &mut warnings)
            .map_err(|message| Failure::new(file, message))?;
        for warning in warnings {
            self.diagnostics
                .push(Diagnostic::warning("unresolved", file, 0, warning).in_section(id));
        }
        Ok(resolved)
    }

    /// The blocks of a section or block, each rendered to a fragment. A block that fails is left out.
    fn blocks(
        &mut self,
        section: &Instance,
        parent: &package::Schema,
        blocks: &[Instance],
        depth: usize,
        count: &mut usize,
    ) -> Result<Json, Failure> {
        let mut out = Vec::new();
        if blocks.is_empty() {
            return Ok(Json::Array(out));
        }
        if depth > self.limits.max_depth {
            return Err(Failure::new(
                &package::section_file(&section.kind),
                format!("blocks are nested deeper than {}", self.limits.max_depth),
            ));
        }
        for block in blocks.iter().filter(|block| !block.disabled) {
            *count += 1;
            if *count > self.limits.max_blocks {
                self.leave_out_block(
                    section,
                    block,
                    &format!("over {} blocks in the section", self.limits.max_blocks),
                );
                continue;
            }
            if !parent.blocks.iter().any(|allow| allow.kind == block.kind) {
                self.leave_out_block(
                    section,
                    block,
                    &format!("{} does not allow `{}` blocks", parent.file, block.kind),
                );
                continue;
            }
            let file = package::block_file(&block.kind);
            let mut found = Vec::new();
            let schema = package::schema(self.package, "blocks", &block.kind, &mut found);
            let rendered = self.package.text(&file).map_or_else(
                || {
                    Err(Failure::new(
                        &file,
                        format!("block type `{}` is not in the package", block.kind),
                    ))
                },
                |_| {
                    let settings = self.settings(&schema, &block.settings, &file, &section.id)?;
                    let children = self.blocks(section, &schema, &block.blocks, depth + 1, count)?;
                    let mut view = self.base.clone();
                    view.insert("section".to_string(), json!({"id": section.id, "type": section.kind}));
                    view.insert(
                        "block".to_string(),
                        json!({"id": block.id, "type": block.kind, "settings": settings, "blocks": children}),
                    );
                    view.insert("blocks".to_string(), children);
                    self.render_file(&file, &Json::Object(view))
                        .map(|html| (settings, html))
                },
            );
            match rendered {
                Ok((settings, html)) => {
                    let placeholder = self.fragment(html);
                    out.push(json!({"id": block.id, "type": block.kind, "settings": settings, "html": placeholder}));
                }
                Err(failure) => {
                    self.diagnostics.extend(
                        failure
                            .diagnostics
                            .into_iter()
                            .map(|diagnostic| diagnostic.in_section(&section.id).in_block(&block.id)),
                    );
                    self.diagnostics.push(
                        Diagnostic::warning("render-failed", &failure.file, 0, failure.message.clone())
                            .in_section(&section.id)
                            .in_block(&block.id),
                    );
                    self.leave_out_block(section, block, &failure.message);
                }
            }
        }
        Ok(Json::Array(out))
    }

    fn leave_out_block(&mut self, section: &Instance, block: &Instance, reason: &str) {
        tracing::error!(
            "Theme {} section {} block {} ({}) left out: {reason}",
            self.log_name(),
            section.id,
            block.id,
            block.kind
        );
        self.omitted.push(Omitted {
            section: section.id.clone(),
            block: Some(block.id.clone()),
            reason: reason.to_string(),
        });
    }

    /// Render one template file of the package with a view: assembled with its snippets, its files checked, run in
    /// the locked renderer within the budgets, and its output checked.
    fn render_file(&mut self, file: &str, view: &Json) -> Result<String, Failure> {
        let assembled =
            package::assemble(self.package, file, self.limits.max_include_depth).map_err(|diagnostic| Failure {
                file: file.to_string(),
                message: diagnostic.message.clone(),
                diagnostics: vec![diagnostic],
            })?;
        let mut problems = Vec::new();
        for used in &assembled.files {
            self.dependencies.insert(used.clone());
            if let Some(text) = self.package.text(used) {
                let text: std::sync::Arc<str> = std::sync::Arc::from(text);
                problems.extend(
                    package::check_template_file(used, &text, &self.request.allowed_origins)
                        .iter()
                        .filter(|diagnostic| diagnostic.is_error())
                        .cloned(),
                );
            }
        }
        if let Some(first) = problems.first() {
            return Err(Failure {
                file: first.file.clone(),
                message: format!("{} (line {}): {}", first.file, first.line, first.message),
                diagnostics: problems,
            });
        }
        let now = Instant::now();
        if now >= self.deadline {
            return Err(Failure::new(file, "the page's time budget is spent"));
        }
        let deadline = self.deadline.min(now + self.limits.render_time);
        let json = serde_json::to_string(view).map_err(|err| Failure::new(file, err.to_string()))?;
        let out = template::render_locked_bounded(&assembled.text, &json, deadline, self.limits.memory_bytes).map_err(
            |err| {
                let message = if Instant::now() >= deadline {
                    format!(
                        "stopped at its time budget of {} ms",
                        deadline.saturating_duration_since(now).as_millis()
                    )
                } else {
                    err.message().to_string()
                };
                Failure::new(file, message)
            },
        )?;
        if out.len() > self.limits.max_output_bytes {
            return Err(Failure::new(
                file,
                format!("its output is over {} bytes", self.limits.max_output_bytes),
            ));
        }
        // Only the fragments this render's view held may appear in its output. serde_json writes private-use
        // characters as they are, so the view's JSON text holds its placeholders.
        let allowed: HashSet<usize> = markup::fragment_refs(&json, &self.nonce)
            .map(|refs| refs.into_iter().map(|found| found.index).collect())
            .unwrap_or_default();
        let policy = Policy {
            origins: &self.request.allowed_origins,
        };
        let mode = Mode::Output {
            nonce: &self.nonce,
            allowed: &allowed,
        };
        let found = markup::check_html(&out, &mode, &policy);
        if let Some(first) = found.first() {
            let diagnostics = found
                .iter()
                .map(|problem| {
                    Diagnostic::error(
                        problem.code,
                        file,
                        0,
                        format!("in the rendered output: {}", problem.message),
                    )
                })
                .collect();
            return Err(Failure {
                file: file.to_string(),
                message: format!("its output is refused: {}", first.message),
                diagnostics,
            });
        }
        Ok(out)
    }

    fn finish(mut self, page: Option<String>) -> Rendered {
        let mut html = String::new();
        let mut ok = page.is_some();
        let root = page.unwrap_or_default();
        if ok {
            if let Err(message) = self.expand(&root, &mut html) {
                self.diagnostics.push(Diagnostic::error(
                    "budget",
                    "",
                    0,
                    format!("the page was not rendered: {message}"),
                ));
                tracing::error!("Theme {} not rendered: {message}", self.log_name());
                html.clear();
                ok = false;
            }
        }
        if ok {
            for (name, placeholder, required) in &self.components {
                if *required && !self.placed(&root, placeholder) {
                    self.failed_required.push(format!("component:{name}"));
                    self.diagnostics.push(Diagnostic::error(
                        "required-failed",
                        "",
                        0,
                        format!("required component `{name}` is not on the page"),
                    ));
                }
            }
        }
        let failed_required = std::mem::take(&mut self.failed_required);
        Rendered {
            ok: ok && failed_required.is_empty(),
            html: if ok { html } else { String::new() },
            diagnostics: self.diagnostics,
            omitted: self.omitted,
            failed_required,
            dependencies: self.dependencies.into_iter().collect(),
        }
    }

    /// Whether a component's placeholder is reached from the page: in the page fragment or a fragment it places.
    fn placed(&self, root: &str, placeholder: &str) -> bool {
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(text) = stack.pop() {
            if text.contains(placeholder) {
                return true;
            }
            if let Ok(refs) = markup::fragment_refs(text, &self.nonce) {
                for found in refs {
                    if seen.insert(found.index) {
                        if let Some((inner, _)) = self.fragments.get(found.index) {
                            stack.push(inner);
                        }
                    }
                }
            }
        }
        false
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What a setting resolves through: the request's references, media and sources.
struct Resolver<'p, 'a> {
    page: &'p mut Page<'a>,
}

impl Resolve for Resolver<'_, '_> {
    fn locale(&self) -> Option<String> {
        self.page
            .request
            .view
            .get("locale")
            .and_then(Json::as_str)
            .map(str::to_string)
    }

    fn rich_text(&mut self, html: &str) -> Json {
        let clean = markup::sanitize_rich_text(html);
        json!(self.page.fragment(clean))
    }

    fn image(&mut self, value: &str) -> Option<Json> {
        if value.starts_with("assets/") {
            self.page.dependencies.insert(value.to_string());
            return Some(json!({"url": self.page.asset_url(value), "alt": ""}));
        }
        let media = self.page.request.media.get(value)?.clone();
        Some(clean_view(&media, self.page))
    }

    fn reference(&mut self, key: &str) -> Option<Json> {
        let found = self.page.request.references.get(key)?.clone();
        Some(clean_view(&found, self.page))
    }

    fn source(&mut self, name: &str) -> Option<Json> {
        let found = self.page.request.sources.get(name)?.clone();
        Some(clean_view(&found, self.page))
    }

    fn has_asset(&self, path: &str) -> bool {
        self.page.package.has(path)
    }
}
