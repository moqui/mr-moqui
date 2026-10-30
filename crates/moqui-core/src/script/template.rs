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

//! The fancy template dialect: `if`, `list` and `assign` tags plus `${expr}`, compiled to one
//! JavaScript function and cached.
//!
//! The trusted renderer runs that function on the script runtime with the live context. The
//! locked-down renderer runs it in a separate QuickJS context that holds nothing but the plain
//! snapshot of a map, `format` and `localize`. Tags are listed in Templating in
//! `plans/archive/Architecture-Proposal.md`. Every template also has `raw(v)` (output as is), `html(v)` (HTML escaped
//! once) and `cssString(v)` (a quoted CSS string that cannot end a `<style>` element, for `@page` margin boxes).

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::runtime::js_error;

use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use rquickjs::function::Opt;
use rquickjs::{CatchResultExt, Context, Ctx, Exception, Function, Persistent, Runtime, Value as JsValue};

use crate::context::{Error, ExecutionContextFactory};

const MAX_COMPILED: usize = 4096;
const LOCKED_TIME_LIMIT: Duration = Duration::from_secs(5);
const LOCKED_MEMORY_LIMIT: usize = 32 * 1024 * 1024;

/// How `${expr}` output is encoded. `Html` also serves XML: both escape `& < > " '`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Escape {
    None,
    Html,
}

/// Which names the template sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Live,
    Snapshot,
}

/// A renderer for one template syntax. Conf maps a file extension to an implementation.
pub trait TemplateRenderer: Send + Sync {
    /// Render `source`. Trusted renderers read the live context and add the entries of `data` in a
    /// frame of their own. A locked-down renderer sees only `data`.
    ///
    /// # Errors
    ///
    /// Returns an error when the template does not compile or throws.
    fn render(&self, source: &str, data: Option<&Value>) -> Result<String, Error>;

    /// The compiled form, for a renderer that has one. Used to check a template without running it.
    ///
    /// # Errors
    ///
    /// Returns an error when the template does not compile.
    fn check(&self, source: &str) -> Result<(), Error>;

    /// The text a file template is kept as between renders. Markdown is converted once here, as
    /// Java keeps the rendered markdown of a location in `resource.markdown.location`.
    ///
    /// # Errors
    ///
    /// Returns an error when the source cannot be prepared.
    fn prepare(&self, source: &str) -> Result<String, Error> {
        Ok(source.to_string())
    }

    /// Render text that `prepare` returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the template does not compile or throws.
    fn render_prepared(&self, prepared: &str, data: Option<&Value>) -> Result<String, Error> {
        self.render(prepared, data)
    }

    /// Whether the output depends on the `sri.baseLinkUrl` of the screen being rendered, as markdown's relative
    /// links do. Such a file is rendered from its source with [`Self::render`] while a base is set, not from the
    /// kept prepared text, which was made without one.
    fn links_follow_base(&self) -> bool {
        false
    }
}

/// The trusted fancy renderer: the live context, `ec` and `sri`.
pub struct FancyRenderer {
    escape: Escape,
}

/// The locked-down fancy renderer: only the snapshot of the map passed in.
pub struct LockedRenderer {
    escape: Escape,
}

impl TemplateRenderer for FancyRenderer {
    fn render(&self, source: &str, data: Option<&Value>) -> Result<String, Error> {
        let compiled = compiled(source, self.escape, Scope::Live)?;
        let _frame = data.is_some().then(super::ContextFrame::push);
        if let Some(Value::Map(entries)) = data {
            super::put_context_all(entries);
        }
        super::runtime::run_template_function(&compiled)
    }

    fn check(&self, source: &str) -> Result<(), Error> {
        compiled(source, self.escape, Scope::Live).map(|_| ())
    }
}

impl TemplateRenderer for LockedRenderer {
    fn render(&self, source: &str, data: Option<&Value>) -> Result<String, Error> {
        let compiled = compiled(source, self.escape, Scope::Snapshot)?;
        let json = data.map_or_else(|| "{}".to_string(), |value| unwrap_json(value).to_string());
        render_locked(&compiled, &json)
    }

    fn check(&self, source: &str) -> Result<(), Error> {
        compiled(source, self.escape, Scope::Snapshot).map(|_| ())
    }
}

/// `.md` and `.markdown`: CommonMark with tables to HTML, raw HTML passed through as Java's
/// flexmark renderer does. There is no context: the file is not a template.
pub struct MarkdownRenderer;

impl TemplateRenderer for MarkdownRenderer {
    fn render(&self, source: &str, _data: Option<&Value>) -> Result<String, Error> {
        Ok(moqui_lib::markdown::to_html_with_base(
            source,
            current_base_link_url().as_deref(),
        ))
    }

    fn check(&self, _source: &str) -> Result<(), Error> {
        Ok(())
    }

    fn prepare(&self, source: &str) -> Result<String, Error> {
        Ok(moqui_lib::markdown::to_html(source))
    }

    fn render_prepared(&self, prepared: &str, _data: Option<&Value>) -> Result<String, Error> {
        Ok(prepared.to_string())
    }

    fn links_follow_base(&self) -> bool {
        true
    }
}

/// `.md.fancy` and `.markdown.fancy`, the port of Java's `.md.ftl`: the markdown is converted to
/// HTML first and the result is a trusted fancy template. Unlike Java, the tags and `${}` of the
/// template are set aside before the markdown pass, so markdown does not escape their text, and
/// `${}` output is HTML escaped as in `.html.fancy` (FreeMarker in Java did not escape it).
pub struct MarkdownFancyRenderer;

impl TemplateRenderer for MarkdownFancyRenderer {
    fn render(&self, source: &str, data: Option<&Value>) -> Result<String, Error> {
        let prepared = markdown_fancy_source_with_base(source, current_base_link_url().as_deref())?;
        self.render_prepared(&prepared, data)
    }

    fn check(&self, source: &str) -> Result<(), Error> {
        FancyRenderer { escape: Escape::Html }.check(&self.prepare(source)?)
    }

    fn prepare(&self, source: &str) -> Result<String, Error> {
        markdown_fancy_source(source)
    }

    fn render_prepared(&self, prepared: &str, data: Option<&Value>) -> Result<String, Error> {
        FancyRenderer { escape: Escape::Html }.render(prepared, data)
    }

    fn links_follow_base(&self) -> bool {
        true
    }
}

/// The `sri.baseLinkUrl` of the screen being rendered, when one is set and not empty.
fn current_base_link_url() -> Option<String> {
    crate::screen::with_current_render(|state| state.base_link_url.borrow().clone())
        .flatten()
        .filter(|base| !base.is_empty())
}

/// The fancy template that a markdown template becomes: the markdown converted to HTML with each
/// tag, comment, `${}` and escape of the template put back as written. A tag alone in a paragraph
/// loses the paragraph, so a tag on its own line stays on its own line. A tag alone on a line inside
/// a table loses the row the markdown made of it, so a `<#list>` around a row repeats the row; Java's
/// `.md.ftl` also ran markdown first, and there the tag stayed in a row of its own.
///
/// # Errors
///
/// Returns an error when a tag or `${` of the template is not closed.
pub fn markdown_fancy_source(source: &str) -> Result<String, Error> {
    markdown_fancy_source_with_base(source, None)
}

/// [`markdown_fancy_source`] with the relative links of the markdown resolved against `base`, as
/// [`moqui_lib::markdown::to_html_with_base`]. A destination that starts with a `${}` of the template is left as
/// written: what it evaluates to is not known here.
///
/// # Errors
///
/// Same as [`markdown_fancy_source`].
pub fn markdown_fancy_source_with_base(source: &str, base: Option<&str>) -> Result<String, Error> {
    let mut prefix_number = 0;
    let prefix = loop {
        let candidate = format!("mqfancy{prefix_number}p");
        if !source.contains(&candidate) {
            break candidate;
        }
        prefix_number += 1;
    };
    let mut held: Vec<Held> = Vec::new();
    let mut markdown = String::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let rest = &source[index..];
        let span = if rest.starts_with("\\${") || rest.starts_with("\\<#") {
            Some((index + 3, false))
        } else if rest.starts_with("<#--") {
            let end = rest
                .find("-->")
                .ok_or_else(|| fail(source, index, "comment is not closed"))?;
            Some((index + end + 3, true))
        } else if rest.starts_with("${") {
            let end =
                scan_expression(source, index + 2, b'}').ok_or_else(|| fail(source, index, "${ has no closing }"))?;
            Some((end + 1, false))
        } else if rest.starts_with("<#") || rest.starts_with("</#") || rest.starts_with("<@") || rest.starts_with("</@")
        {
            let (_, next) = tag(source, index)?;
            Some((next, true))
        } else {
            None
        };
        if let Some((end, is_tag)) = span {
            let placeholder = format!("{prefix}{}x", held.len());
            markdown.push_str(&placeholder);
            held.push(Held {
                placeholder,
                original: source[index..end].to_string(),
                is_tag,
            });
            index = end;
        } else {
            let ch = rest.chars().next().unwrap_or('\u{FFFD}');
            markdown.push(ch);
            index += ch.len_utf8();
        }
    }
    let markdown_html = match base.filter(|base| !base.is_empty()) {
        Some(base) => moqui_lib::markdown::to_html_mapped(&markdown, |dest| {
            (!dest.starts_with(&prefix))
                .then(|| moqui_lib::markdown::resolve_link(base, dest))
                .flatten()
        }),
        None => moqui_lib::markdown::to_html(&markdown),
    };
    let mut html = lift_tag_rows(&markdown_html, &prefix, &held);
    for Held {
        placeholder,
        original,
        is_tag,
    } in &held
    {
        if *is_tag {
            html = html.replace(&format!("<p>{placeholder}</p>"), original);
        }
        html = html.replace(placeholder, original);
    }
    Ok(html)
}

/// A piece of the fancy source that markdown must not see, and the placeholder that stands for it.
struct Held {
    placeholder: String,
    original: String,
    /// A tag (or comment) rather than an expression: its line may become a table row or a paragraph to lift.
    is_tag: bool,
}

/// The markdown HTML with each table row that markdown made of a tag line replaced by the line's
/// placeholders: a row whose first cell is only tag placeholders and whose other cells are empty.
/// pulldown-cmark writes each body row on a line of its own.
fn lift_tag_rows(html: &str, prefix: &str, held: &[Held]) -> String {
    let mut out = String::with_capacity(html.len());
    for line in html.split_inclusive('\n') {
        match tag_row_text(line.trim_end_matches('\n'), prefix, held) {
            Some(tags) => {
                out.push_str(tags);
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out
}

/// The first cell's text when `row` is a table row of tag placeholders alone, else `None`.
fn tag_row_text<'a>(row: &'a str, prefix: &str, held: &[Held]) -> Option<&'a str> {
    let mut cells = row.strip_prefix("<tr>")?.strip_suffix("</tr>")?;
    let mut first = None;
    while !cells.is_empty() {
        let (_, cell) = cells.strip_prefix("<td")?.split_once('>')?;
        let (content, after) = cell.split_once("</td>")?;
        if first.is_none() {
            first = Some(content);
        } else if !content.is_empty() {
            return None;
        }
        cells = after;
    }
    let text = first?;
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let after = rest.strip_prefix(prefix)?;
        let (digits, after) = after.split_at_checked(after.find(|c: char| !c.is_ascii_digit())?)?;
        let index: usize = digits.parse().ok()?;
        if !held.get(index)?.is_tag {
            return None;
        }
        rest = after.strip_prefix('x')?.trim_start();
    }
    Some(text)
}

/// The renderer a conf `class` names: `fancy`, `fancy-html`, `fancy-locked`, `fancy-locked-html`,
/// `markdown` or `markdown-fancy`.
pub fn renderer_by_class(class: &str) -> Option<Arc<dyn TemplateRenderer>> {
    Some(match class {
        "markdown" => Arc::new(MarkdownRenderer),
        "markdown-fancy" => Arc::new(MarkdownFancyRenderer),
        "fancy" => Arc::new(FancyRenderer { escape: Escape::None }),
        "fancy-html" => Arc::new(FancyRenderer { escape: Escape::Html }),
        "fancy-locked" => Arc::new(LockedRenderer { escape: Escape::None }),
        "fancy-locked-html" => Arc::new(LockedRenderer { escape: Escape::Html }),
        _ => return None,
    })
}

/// Render `source` as a trusted fancy template with the live context.
///
/// # Errors
///
/// Returns an error when the template does not compile or throws.
pub fn render_trusted(source: &str, escape: Escape) -> Result<String, Error> {
    FancyRenderer { escape }.render(source, None)
}

/// Render `source` in the locked-down renderer with the snapshot of `data`.
///
/// # Errors
///
/// Returns an error when the template does not compile or throws.
pub fn render_locked_with(source: &str, escape: Escape, data: &Value) -> Result<String, Error> {
    LockedRenderer { escape }.render(source, Some(data))
}

/// Render `source` as a trusted fancy template on the context of a running host call.
pub(crate) fn render_trusted_here(ctx: &Ctx<'_>, source: &str, escape: Escape) -> Result<String, Error> {
    let compiled = compiled(source, escape, Scope::Live)?;
    super::runtime::run_template_function_here(ctx, &compiled)
}

/// Check that a fancy template compiles, including a parse of every expression.
///
/// # Errors
///
/// Returns the first compile error with its line.
pub fn check_template(source: &str) -> Result<(), Error> {
    compile(source, Escape::None, Scope::Live).map(|_| ())
}

/// The renderers registered in conf, by extension, and the compiled form of file templates.
pub struct TemplateRegistry {
    by_extension: Vec<(String, Arc<dyn TemplateRenderer>)>,
    /// The prepared text of a file template by location, with the stamp it was prepared at.
    files: Mutex<super::bounded::BoundedMap<String, (i64, Arc<str>)>>,
}

impl TemplateRegistry {
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|factory| Self::parse_with_native(factory.conf_text(), Some(factory.native())))
    }

    #[cfg(test)]
    fn parse(conf_text: &str) -> Self {
        Self::parse_with_native(conf_text, None)
    }

    fn parse_with_native(conf_text: &str, native: Option<&crate::native::NativeRegistry>) -> Self {
        let mut by_extension = Vec::new();
        if let Ok(root) = moqui_lib::node::MNode::parse_shared("moqui-conf", conf_text) {
            for facade in root.named_iter("resource-facade") {
                for entry in facade.named_iter("template-renderer") {
                    let (Some(extension), Some(class)) = (entry.attribute("extension"), entry.attribute("class"))
                    else {
                        continue;
                    };
                    if let Some(renderer) =
                        renderer_by_class(&class).or_else(|| native.and_then(|registry| registry.template_for(&class)))
                    {
                        by_extension.push((extension, renderer));
                    }
                }
            }
        }
        // A factory with no conf still renders `.fancy` and markdown files.
        if by_extension.is_empty() {
            for (extension, class) in [
                ("fancy", "fancy"),
                ("html.fancy", "fancy-html"),
                ("fancy-locked", "fancy-locked"),
                ("html.fancy-locked", "fancy-locked-html"),
                ("md", "markdown"),
                ("markdown", "markdown"),
                ("md.fancy", "markdown-fancy"),
                ("markdown.fancy", "markdown-fancy"),
            ] {
                if let Some(renderer) = renderer_by_class(class) {
                    by_extension.push((extension.to_string(), renderer));
                }
            }
        }
        Self {
            by_extension,
            files: Mutex::new(super::bounded::BoundedMap::new(MAX_COMPILED)),
        }
    }

    /// The registered extensions, in conf order without repeats (`ResourceFacade.getTemplateRendererExtensionSet`).
    pub fn extensions(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.by_extension
            .iter()
            .filter(|(extension, _)| seen.insert(extension.clone()))
            .map(|(extension, _)| extension.clone())
            .collect()
    }

    /// The renderer for the longest registered extension `location` ends with.
    pub fn renderer_for(&self, location: &str) -> Option<Arc<dyn TemplateRenderer>> {
        // A `#version` suffix names a version, not part of the file name.
        let location = location.split('#').next().unwrap_or(location);
        let name = location.rsplit('/').next().unwrap_or(location);
        self.by_extension
            .iter()
            .filter(|(extension, _)| name.len() > extension.len() && name.ends_with(&format!(".{extension}")))
            .max_by_key(|(extension, _)| extension.len())
            .map(|(_, renderer)| Arc::clone(renderer))
    }

    /// The renderer for `location`, else for `default_extension` used as an extension (Java's `template(location,
    /// defaultExtension)`, which a screen `<text template>` passes), else none.
    pub fn renderer_with_default(&self, location: &str, default_extension: &str) -> Option<Arc<dyn TemplateRenderer>> {
        self.renderer_for(location).or_else(|| {
            let extension = default_extension.trim().trim_start_matches('.');
            (!extension.is_empty() && extension != "true")
                .then(|| self.renderer_for(&format!("template.{extension}")))?
        })
    }

    /// Render a file template. `ec.resource.template`. The prepared text (the HTML of a markdown
    /// file) is kept by location and last-modified time, and the compiled form by its text. A
    /// location whose extension has no renderer is its text, as in Java.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is missing or the template fails.
    pub fn render_location(
        &self,
        factory: &ExecutionContextFactory,
        location: &str,
        data: Option<&Value>,
    ) -> Result<String, Error> {
        self.render_location_default(factory, location, data, "")
    }

    /// [`Self::render_location`] with Java's default extension for a location that has no renderer of its own.
    ///
    /// # Errors
    ///
    /// Same as [`Self::render_location`].
    pub fn render_location_default(
        &self,
        factory: &ExecutionContextFactory,
        location: &str,
        data: Option<&Value>,
        default_extension: &str,
    ) -> Result<String, Error> {
        let Some(renderer) = self.renderer_with_default(location, default_extension) else {
            return self.location_text(factory, location).map(|text| text.to_string());
        };
        if renderer.links_follow_base() && current_base_link_url().is_some() {
            return renderer.render(&self.location_text(factory, location)?, data);
        }
        let text = self.prepared(factory, location, &format!("t:{location}"), |source| {
            renderer.prepare(source)
        })?;
        renderer.render_prepared(&text, data)
    }

    /// Drop every kept file text: a dev reload of a changed template or text file, whose modified time a coarse file
    /// system may leave the same for two quick saves (plans/design-dev-aide.md 6.1, F6).
    pub fn forget_texts(&self) {
        lock(&self.files).clear();
    }

    /// The text of a file, kept by location and last-modified time as Java's `resource.text.location`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read.
    pub fn location_text(&self, factory: &ExecutionContextFactory, location: &str) -> Result<Arc<str>, Error> {
        self.prepared(factory, location, &format!("x:{location}"), |source| {
            Ok(source.to_string())
        })
    }

    fn prepared(
        &self,
        factory: &ExecutionContextFactory,
        location: &str,
        key: &str,
        prepare: impl FnOnce(&str) -> Result<String, Error>,
    ) -> Result<Arc<str>, Error> {
        let resources = factory.resource();
        let path = location.split('#').next().unwrap_or(location);
        // A `#versionName` suffix on storage that keeps versions renders that version, as Java's `getLocationText`.
        let versioned = resources.supports_version(path);
        let version = crate::context::split_version(location)
            .map(|(_, version)| version)
            .filter(|_| versioned);
        let resource = resources.get(path)?;
        // A file is kept by its modified time. A database or http resource, or a version, has none, and its text is
        // already in hand, so it is kept by a hash of that text: the markdown is not converted again. Storage that
        // keeps versions is kept by its text too: a git commit's time is whole seconds, the same for two quick saves.
        let modified = if versioned { None } else { resource.last_modified() };
        let (stamp, source) = match (version, modified) {
            (Some(version), _) => {
                let source = resources
                    .version_text(path, version)?
                    .ok_or_else(|| Error::new(format!("version {version} not found: {path}")))?;
                (text_stamp(&source), Some(source))
            }
            (None, Some(modified)) => (modified, None),
            (None, None) => {
                let source = resource.read_text()?;
                (text_stamp(&source), Some(source))
            }
        };
        let cached = lock(&self.files)
            .get(key)
            .filter(|(kept, _)| *kept == stamp)
            .map(|(_, text)| text);
        if let Some(text) = cached {
            return Ok(text);
        }
        let source = match source {
            Some(source) => source,
            None => resource.read_text()?,
        };
        let text: Arc<str> = prepare(&source)?.into();
        lock(&self.files).insert(key.to_string(), (stamp, Arc::clone(&text)));
        Ok(text)
    }
}

/// A stamp for text with no modified time: FNV-1a of the bytes.
fn text_stamp(text: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    i64::from_ne_bytes(hash.to_ne_bytes())
}

// ---- cache of compiled functions, by source text ----

type CompiledCache = Mutex<super::bounded::BoundedMap<(Escape, bool, String), Arc<str>>>;

fn cache() -> &'static CompiledCache {
    static CACHE: std::sync::OnceLock<CompiledCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(super::bounded::BoundedMap::new(MAX_COMPILED)))
}

fn compiled(source: &str, escape: Escape, scope: Scope) -> Result<Arc<str>, Error> {
    let key = (escape, scope == Scope::Snapshot, source.to_string());
    if let Some(found) = lock(cache()).get(&key) {
        return Ok(found);
    }
    let made: Arc<str> = Arc::from(compile(source, escape, scope)?);
    lock(cache()).insert(key, Arc::clone(&made));
    Ok(made)
}

// ---- compiler ----

#[derive(Debug, PartialEq)]
enum Token {
    Text(String),
    Interp(String),
    If(String),
    ElseIf(String),
    Else,
    EndIf,
    List(String, String),
    EndList,
    Assign(String, String),
    Comment,
}

impl Token {
    fn is_tag(&self) -> bool {
        !matches!(self, Token::Text(_) | Token::Interp(_))
    }
}

const PRELUDE: &str = r#"const $esc = (v) => String(v).replace(/[&<>"']/g, (c) => c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : c === '"' ? "&quot;" : "&#39;");
class $Raw { constructor(s) { this.s = s; } }
const $s = (v) => v instanceof $Raw ? v.s : v == null ? "" : String(v);
const $h = (v) => v instanceof $Raw ? v.s : v == null ? "" : $esc(v);
const html = (v) => new $Raw($h(v));
const raw = (v) => new $Raw($s(v));
const cssString = (v) => new $Raw('"' + $s(v).replace(/[\\"<>\r\n]/g, (c) => c === "\n" ? "\\A " : "\\" + c.charCodeAt(0).toString(16).toUpperCase() + " ") + '"');
const $list = (v) => v == null ? [] : Array.isArray(v) ? v : typeof v === "string" ? [v] : typeof v[Symbol.iterator] === "function" ? Array.from(v) : typeof v.length === "number" ? Array.from({length: v.length}, (_, i) => v[i]) : Object.values(v);
const $o = [];
"#;

/// The statements for a template, without the prelude and the function around them.
fn compile_body(source: &str, escape: Escape) -> Result<String, Error> {
    let tokens = tokenize(source)?;
    generate(&tokens, escape, source)
}

fn compile(source: &str, escape: Escape, scope: Scope) -> Result<String, Error> {
    let body = compile_body(source, escape)?;
    // An `<#assign>` is a `var` of the function, and a `var` is hoisted: without this, the name reads `undefined`
    // before the assign instead of the context's value. The live form starts each assigned name from the context.
    let mut declared = String::new();
    for token in tokenize(source)? {
        if let Token::Assign(name, _) = token {
            if !declared.contains(&format!("var {name} ")) {
                declared.push_str(&format!("var {name} = globalThis.{name};\n"));
            }
        }
    }
    let live = format!("(function(){{\n{PRELUDE}{declared}{body}return $o.join(\"\");\n}})");
    // `with` is not legal in the strict parse, so the snapshot form is checked as the live form.
    super::check_program(&live).map_err(|err| Error::new(format!("template does not compile: {}", err.message())))?;
    let text = match scope {
        Scope::Live => live,
        Scope::Snapshot => {
            format!("(function($data){{\n{PRELUDE}with ($scope($data)) {{\n{body}}}\nreturn $o.join(\"\");\n}})")
        }
    };
    Ok(text)
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

fn fail(source: &str, offset: usize, message: &str) -> Error {
    Error::new(format!("template line {}: {message}", line_of(source, offset)))
}

/// Where the expression that starts at `from` ends: the first `>` (or `}`) outside brackets and
/// strings. An arrow `=>` does not end a tag.
fn scan_expression(source: &str, from: usize, terminator: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = from;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\'' | b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != byte {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            b'`' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'`' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    } else if bytes[index] == b'$' && bytes.get(index + 1) == Some(&b'{') {
                        index = scan_expression(source, index + 2, b'}')?;
                    }
                    index += 1;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' if terminator == b'}' && depth == 0 => return Some(index),
            b'}' => depth = depth.saturating_sub(1),
            b'>' if terminator == b'>' && depth == 0 && (index == 0 || bytes[index - 1] != b'=') => {
                return Some(index);
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn tokenize(source: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut index = 0;
    macro_rules! flush {
        () => {
            if !text.is_empty() {
                tokens.push(Token::Text(std::mem::take(&mut text)));
            }
        };
    }
    while index < source.len() {
        let rest = &source[index..];
        if rest.starts_with("\\${") {
            text.push_str("${");
            index += 3;
        } else if rest.starts_with("\\<#") {
            text.push_str("<#");
            index += 3;
        } else if rest.starts_with("<#--") {
            let end = rest
                .find("-->")
                .ok_or_else(|| fail(source, index, "comment is not closed"))?;
            flush!();
            tokens.push(Token::Comment);
            index += end + 3;
        } else if rest.starts_with("${") {
            let end =
                scan_expression(source, index + 2, b'}').ok_or_else(|| fail(source, index, "${ has no closing }"))?;
            flush!();
            tokens.push(Token::Interp(source[index + 2..end].to_string()));
            index = end + 1;
        } else if rest.starts_with("<#") || rest.starts_with("</#") || rest.starts_with("<@") || rest.starts_with("</@")
        {
            flush!();
            let (token, next) = tag(source, index)?;
            tokens.push(token);
            index = next;
        } else {
            let ch = rest.chars().next().unwrap_or('\u{FFFD}');
            text.push(ch);
            index += ch.len_utf8();
        }
    }
    flush!();
    Ok(strip_tag_lines(tokens))
}

fn tag(source: &str, start: usize) -> Result<(Token, usize), Error> {
    let rest = &source[start..];
    if rest.starts_with("<@") || rest.starts_with("</@") {
        return Err(fail(
            source,
            start,
            "FreeMarker macro calls are not part of the fancy dialect",
        ));
    }
    let closing = rest.starts_with("</#");
    let name_start = start + if closing { 3 } else { 2 };
    let name_end = source[name_start..]
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .map_or(source.len(), |at| name_start + at);
    let name = &source[name_start..name_end];
    let simple = |source: &str, from: usize| -> Result<usize, Error> {
        let trimmed = source[from..].trim_start();
        if trimmed.starts_with('>') {
            Ok(source.len() - trimmed.len() + 1)
        } else {
            Err(fail(source, start, &format!("<#{name}> takes no arguments")))
        }
    };
    if closing {
        let next = simple(source, name_end)?;
        return match name {
            "if" => Ok((Token::EndIf, next)),
            "list" => Ok((Token::EndList, next)),
            _ => Err(fail(source, start, &format!("unknown tag </#{name}>"))),
        };
    }
    if name == "else" {
        return Ok((Token::Else, simple(source, name_end)?));
    }
    if !matches!(name, "if" | "elseif" | "list" | "assign") {
        return Err(fail(
            source,
            start,
            &format!("unknown tag <#{name}>; the dialect has if, elseif, else, list and assign"),
        ));
    }
    let end = scan_expression(source, name_end, b'>')
        .ok_or_else(|| fail(source, start, &format!("<#{name} has no closing >")))?;
    let argument = source[name_end..end].trim().to_string();
    if argument.is_empty() {
        return Err(fail(source, start, &format!("<#{name}> needs an expression")));
    }
    let token = match name {
        "if" => Token::If(argument),
        "elseif" => Token::ElseIf(argument),
        "list" => {
            let (items, variable) = split_last_word(&argument, "as")
                .ok_or_else(|| fail(source, start, "<#list> needs `expression as name`"))?;
            if !is_identifier(variable) {
                return Err(fail(source, start, &format!("`{variable}` is not a variable name")));
            }
            Token::List(items.to_string(), variable.to_string())
        }
        _ => {
            let (variable, value) = argument
                .split_once('=')
                .filter(|(_, value)| !value.starts_with('=') && !value.trim().is_empty())
                .ok_or_else(|| fail(source, start, "<#assign> needs `name = expression`"))?;
            let variable = variable.trim();
            if !is_identifier(variable) {
                return Err(fail(source, start, &format!("`{variable}` is not a variable name")));
            }
            Token::Assign(variable.to_string(), value.trim().to_string())
        }
    };
    Ok((token, end + 1))
}

/// `a.b as item` split at the last ` as ` word.
fn split_last_word<'a>(text: &'a str, word: &str) -> Option<(&'a str, &'a str)> {
    let (before, after) = text.rsplit_once(&format!(" {word} "))?;
    Some((before.trim(), after.trim()))
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.starts_with('$')
}

/// A line that holds only tags and white space produces no output, as in FreeMarker.
fn strip_tag_lines(tokens: Vec<Token>) -> Vec<Token> {
    // Split text so a newline ends a piece.
    let mut pieces: Vec<Token> = Vec::new();
    for token in tokens {
        match token {
            Token::Text(text) => {
                for part in text.split_inclusive('\n') {
                    pieces.push(Token::Text(part.to_string()));
                }
            }
            other => pieces.push(other),
        }
    }
    let mut out = Vec::new();
    let mut line: Vec<Token> = Vec::new();
    let flush = |line: &mut Vec<Token>, out: &mut Vec<Token>| {
        let tags_only = line.iter().any(Token::is_tag)
            && line
                .iter()
                .all(|token| token.is_tag() || matches!(token, Token::Text(text) if text.trim().is_empty()));
        for token in line.drain(..) {
            if !tags_only || token.is_tag() {
                out.push(token);
            }
        }
    };
    for piece in pieces {
        let ends = matches!(&piece, Token::Text(text) if text.ends_with('\n'));
        line.push(piece);
        if ends {
            flush(&mut line, &mut out);
        }
    }
    flush(&mut line, &mut out);
    out
}

fn js_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn generate(tokens: &[Token], escape: Escape, source: &str) -> Result<String, Error> {
    let encode = if escape == Escape::Html { "$h" } else { "$s" };
    let mut out = String::new();
    // `true` for an open list, `false` for an open if, with whether an else was seen.
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut lists = 0usize;
    for token in tokens {
        match token {
            Token::Text(text) => {
                out.push_str(&format!("$o.push({});\n", js_string(text)));
            }
            Token::Interp(expr) => {
                check_expression(expr, source)?;
                out.push_str(&format!("$o.push({encode}({expr}));\n"));
            }
            Token::If(cond) => {
                check_expression(cond, source)?;
                out.push_str(&format!("if ({cond}) {{\n"));
                stack.push((false, false));
            }
            Token::ElseIf(cond) => {
                check_expression(cond, source)?;
                match stack.last() {
                    Some((false, false)) => out.push_str(&format!("}} else if ({cond}) {{\n")),
                    _ => {
                        return Err(Error::new(
                            "template: <#elseif> without an open <#if>, or after <#else>",
                        ))
                    }
                }
            }
            Token::Else => match stack.last_mut() {
                Some((false, seen @ false)) => {
                    *seen = true;
                    out.push_str("} else {\n");
                }
                _ => return Err(Error::new("template: <#else> without an open <#if>, or twice")),
            },
            Token::EndIf => match stack.pop() {
                Some((false, _)) => out.push_str("}\n"),
                _ => return Err(Error::new("template: </#if> without an open <#if>")),
            },
            Token::List(items, name) => {
                check_expression(items, source)?;
                lists += 1;
                out.push_str(&format!(
                    "{{\nconst $l{lists} = $list({items});\nfor (let {name}_index = 0; {name}_index < $l{lists}.length; {name}_index++) {{\nconst {name} = $l{lists}[{name}_index];\nconst {name}_has_next = {name}_index < $l{lists}.length - 1;\n"
                ));
                stack.push((true, false));
            }
            Token::EndList => match stack.pop() {
                Some((true, _)) => out.push_str("}\n}\n"),
                _ => return Err(Error::new("template: </#list> without an open <#list>")),
            },
            Token::Comment => {}
            Token::Assign(name, value) => {
                check_expression(value, source)?;
                out.push_str(&format!("var {name} = {value};\n"));
            }
        }
    }
    if let Some((is_list, _)) = stack.last() {
        let open = if *is_list { "<#list>" } else { "<#if>" };
        return Err(Error::new(format!("template: {open} is not closed")));
    }
    Ok(out)
}

fn check_expression(expr: &str, _source: &str) -> Result<(), Error> {
    super::parse_js(expr).map_err(|err| Error::new(format!("template: {}", err.message())))
}

// ---- unwrap snapshot ----

/// The plain projection of a value: strings, booleans and numbers pass through, a decimal is a
/// string, timestamps are text, an entity value is its field map and an entity list is a list of
/// those. Anything else (`ec`, `sri`, finds) is dropped.
pub fn unwrap_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        Value::Decimal(number) => Json::String(number.to_string()),
        // A date and a time are their own text, as Java's `toString`, so the template formats them with no zone.
        Value::Date(date) => Json::String(date.strftime("%Y-%m-%d").to_string()),
        Value::Time(time) => Json::String(time.strftime("%H:%M:%S").to_string()),
        Value::List(items) => Json::Array(items.iter().map(unwrap_json).collect()),
        Value::Map(entries) => Json::Object(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), unwrap_json(item)))
                .collect(),
        ),
        Value::Object(_) => {
            if let Some(entries) = super::value_map(value) {
                Json::Object(
                    entries
                        .iter()
                        .map(|(key, item)| (key.clone(), unwrap_json(item)))
                        .collect(),
                )
            } else if let Some(items) = super::value_items(value) {
                Json::Array(items.iter().map(unwrap_json).collect())
            } else {
                tracing::warn!("a host object is not part of a locked-down template snapshot and was dropped");
                Json::Null
            }
        }
        other => moqui_lib::json::to_json(other),
    }
}

// ---- locked-down context ----

const SANDBOX_PRELUDE: &str = r#"
const $reserved = new Set(["html", "raw", "cssString"]);
globalThis.$scope = (data) => new Proxy(JSON.parse(data), {
    has: (t, k) => typeof k === "string" && !k.startsWith("$") && !$reserved.has(k) && !(k in globalThis),
    get: (t, k) => k === Symbol.unscopables ? undefined : (t[k] === undefined ? null : t[k]),
    set: (t, k, v) => { t[k] = v; return true; },
});
"#;

/// Freeze the global object, every object reachable from it, and the built-ins that no global names (the iterator,
/// generator and async function prototypes, `%TypedArray%`).
const SANDBOX_LOCKDOWN: &str = r#"
(() => {
    const seen = new Set();
    const freeze = (o) => {
        if (o === null || (typeof o !== "object" && typeof o !== "function") || seen.has(o)) return;
        seen.add(o);
        Object.freeze(o);
        for (const key of Reflect.ownKeys(o)) {
            const d = Object.getOwnPropertyDescriptor(o, key);
            if (!d) continue;
            if ("value" in d) freeze(d.value);
            if (d.get) freeze(d.get);
            if (d.set) freeze(d.set);
        }
        freeze(Object.getPrototypeOf(o));
    };
    const hidden = [
        [][Symbol.iterator](), new Map().entries(), new Set().values(), ""[Symbol.iterator](), /a/[Symbol.matchAll]("a"),
        function* () {}, async function () {}, async function* () {}, (function* () {})(), Int8Array,
    ];
    freeze(globalThis);
    for (const item of hidden) freeze(item);
})();
"#;

struct Sandbox {
    functions: RefCell<super::bounded::BoundedMap<String, Persistent<Function<'static>>>>,
    context: Context,
    // The context borrows this runtime. Drop order frees the context first.
    #[allow(dead_code)]
    runtime: Runtime,
}

thread_local! {
    static SANDBOX: RefCell<Option<Sandbox>> = const { RefCell::new(None) };
    static LOCKED_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

impl Sandbox {
    fn start() -> Result<Self, Error> {
        let runtime = Runtime::new().map_err(js_error)?;
        runtime.set_memory_limit(LOCKED_MEMORY_LIMIT);
        runtime.set_interrupt_handler(Some(Box::new(|| {
            LOCKED_DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
        })));
        let context = Context::full(&runtime).map_err(js_error)?;
        context.with(|ctx| -> Result<(), Error> {
            let globals = ctx.globals();
            globals
                .set("format", Function::new(ctx.clone(), locked_format).map_err(js_error)?)
                .map_err(js_error)?;
            globals
                .set(
                    "localize",
                    Function::new(ctx.clone(), locked_localize).map_err(js_error)?,
                )
                .map_err(js_error)?;
            ctx.eval::<(), _>(SANDBOX_PRELUDE)
                .catch(&ctx)
                .map_err(|err| Error::new(err.to_string()))?;
            // The context is shared by every locked-down render on this thread, so nothing a template does may last:
            // the globals and every built-in object are frozen, and a template cannot change `$esc`'s String methods,
            // JSON, or the functions the next template calls.
            ctx.eval::<(), _>(SANDBOX_LOCKDOWN)
                .catch(&ctx)
                .map_err(|err| Error::new(err.to_string()))
        })?;
        Ok(Self {
            functions: RefCell::new(super::bounded::BoundedMap::new(MAX_COMPILED)),
            context,
            runtime,
        })
    }
}

fn locked_format(ctx: Ctx<'_>, value: JsValue<'_>, pattern: Opt<String>) -> rquickjs::Result<String> {
    let host = if let Some(text) = value.as_string() {
        let text = text.to_string()?;
        // The snapshot sends decimals and dates as text, so the pattern says which it is: a date pattern reads a
        // date or time stamp, a number pattern a number. (Digits alone are also epoch milliseconds to the date
        // parser, so trying it first would print a quantity as a date.)
        match pattern.0.as_deref().filter(|pattern| !pattern.is_empty()) {
            // A date alone (`yyyy-MM-dd`) is a date, formatted with no time zone shift, as Java formats a Date.
            Some(pattern) if crate::l10n::is_date_pattern(pattern) => {
                let trimmed = text.trim();
                let date_only = matches!(trimmed.as_bytes(), [_, _, _, _, b'-', _, _, b'-', _, _]);
                date_only
                    .then(|| moqui_lib::object::parse_date(trimmed).map(Value::Date))
                    .flatten()
                    .or_else(|| crate::l10n::parse_timestamp(&text))
                    .unwrap_or(Value::String(text))
            }
            Some(_) => text
                .trim()
                .parse::<moqui_lib::format::Decimal>()
                .map(Value::Decimal)
                .unwrap_or(Value::String(text)),
            // No pattern: epoch millis text is the time stamp it was, as `ec.l10n.format` reads it.
            None => crate::l10n::epoch_millis_text(&text)
                .map(|millis| moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value)
                .unwrap_or(Value::String(text)),
        }
    } else if let Some(number) = value.as_int() {
        Value::I64(i64::from(number))
    } else if let Some(number) = value.as_float() {
        Value::F64(number)
    } else if let Some(flag) = value.as_bool() {
        Value::Bool(flag)
    } else {
        Value::Null
    };
    crate::l10n::format_value(&host, pattern.0.as_deref()).map_err(|err| Exception::throw_message(&ctx, err.message()))
}

fn locked_localize(ctx: Ctx<'_>, text: String) -> rquickjs::Result<String> {
    crate::l10n::localize(&text).map_err(|err| Exception::throw_message(&ctx, err.message()))
}

fn render_locked(compiled: &str, json: &str) -> Result<String, Error> {
    SANDBOX.with(|cell| {
        if cell.borrow().is_none() {
            *cell.borrow_mut() = Some(Sandbox::start()?);
        }
        let guard = cell.borrow();
        let sandbox = guard
            .as_ref()
            .ok_or_else(|| Error::new("template sandbox did not start"))?;
        let previous = LOCKED_DEADLINE.with(Cell::get);
        LOCKED_DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + LOCKED_TIME_LIMIT)));
        let result = sandbox.context.with(|ctx| {
            let saved = sandbox.functions.borrow_mut().get(compiled);
            let function = match saved {
                Some(saved) => saved.restore(&ctx).map_err(js_error)?,
                None => {
                    let mut options = rquickjs::context::EvalOptions::default();
                    options.strict = false;
                    options.global = true;
                    let function: Function = ctx
                        .eval_with_options(compiled, options)
                        .catch(&ctx)
                        .map_err(|err| Error::new(err.to_string()))?;
                    sandbox
                        .functions
                        .borrow_mut()
                        .insert(compiled.to_string(), Persistent::save(&ctx, function.clone()));
                    function
                }
            };
            function
                .call::<_, String>((json,))
                .catch(&ctx)
                .map_err(|err| Error::new(err.to_string()))
        });
        LOCKED_DEADLINE.with(|deadline| deadline.set(previous));
        result
    })
}

#[cfg(test)]
mod tests;
