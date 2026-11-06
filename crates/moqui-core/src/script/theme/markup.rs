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

//! The markup, URL, CSS and attribute contexts of theme HTML (design-shop §12.4). The same check reads a template's
//! source (its expressions as a mark, so literal markup is refused with its line) and each render's output (so markup
//! an expression made is refused too). Escaping HTML text is not enough for a URL, CSS or attribute, so those are read
//! as a browser's tokenizer reads them: no script, no frame or form, no event attribute, no `javascript:` or
//! HTML-bearing `data:` URL, nothing that loads from an origin the caller has not approved, and no CSS that can.
//!
//! An adapter-made fragment reaches a template as a placeholder (`\u{E000}nonce:n\u{E001}`); in output it may stand
//! only in text, never in an attribute, a comment or raw text, and only where the render's own view gave it.

use std::cell::RefCell;
use std::collections::HashSet;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

/// Opens a fragment placeholder.
pub(crate) const FRAGMENT_OPEN: char = '\u{E000}';
/// Closes a fragment placeholder.
pub(crate) const FRAGMENT_CLOSE: char = '\u{E001}';
/// Stands for a `${}` in the markup of a source.
pub(crate) const EXPRESSION_MARK: char = '\u{E002}';

/// The private-use characters the adapter reserves; stripped from view data and refused in source.
pub(crate) fn is_reserved_char(ch: char) -> bool {
    ('\u{E000}'..='\u{E003}').contains(&ch)
}

/// `text` without the reserved characters.
pub(crate) fn strip_reserved(text: &str) -> String {
    text.chars().filter(|ch| !is_reserved_char(*ch)).collect()
}

/// Rich text cleaned to the safe set (`moqui_lib::safe_html`: no script, style, event attribute or unsafe URL).
pub fn sanitize_rich_text(html: &str) -> String {
    moqui_lib::safe_html::clean(&strip_reserved(html))
}

/// A placeholder in a text: its byte range and fragment number.
pub(crate) struct FragmentRef {
    pub start: usize,
    pub end: usize,
    pub index: usize,
}

/// The placeholders of `nonce` in `text`. A reserved character that does not start a well-formed placeholder of this
/// nonce is an error.
pub(crate) fn fragment_refs(text: &str, nonce: &str) -> Result<Vec<FragmentRef>, String> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(at) = text[index..].find(is_reserved_char).map(|at| index + at) {
        let rest = &text[at..];
        let parsed = rest
            .strip_prefix(FRAGMENT_OPEN)
            .and_then(|rest| rest.strip_prefix(nonce))
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|rest| rest.split_once(FRAGMENT_CLOSE))
            .and_then(|(number, _)| {
                number
                    .parse::<usize>()
                    .ok()
                    .map(|number| (number, number_len(rest, nonce)))
            });
        let Some((number, length)) = parsed else {
            return Err("a reserved character that is not one of the adapter's fragments".to_string());
        };
        found.push(FragmentRef {
            start: at,
            end: at + length,
            index: number,
        });
        index = at + length;
    }
    Ok(found)
}

/// The byte length of the placeholder at the start of `rest`.
fn number_len(rest: &str, nonce: &str) -> usize {
    let digits = rest[FRAGMENT_OPEN.len_utf8() + nonce.len() + 1..]
        .find(FRAGMENT_CLOSE)
        .unwrap_or(0);
    FRAGMENT_OPEN.len_utf8() + nonce.len() + 1 + digits + FRAGMENT_CLOSE.len_utf8()
}

/// What the HTML is.
pub(crate) enum Mode<'a> {
    /// A template's literal markup, with [`EXPRESSION_MARK`] for each `${}`.
    Source,
    /// A render's output, which may hold these fragments.
    Output {
        nonce: &'a str,
        allowed: &'a HashSet<usize>,
    },
}

/// What a URL may reach besides the page's own origin.
pub(crate) struct Policy<'a> {
    pub origins: &'a [String],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

/// Elements a theme cannot have anywhere: scripts, frames, plugins, forms (a form is a trusted component's), and the
/// elements a browser parses differently with scripting on (`noscript`) or that end only at the document's end.
const DENIED_ELEMENTS: &[&str] = &[
    "script",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "form",
    "base",
    "portal",
    "noscript",
    "xmp",
    "plaintext",
    "noembed",
    "noframes",
    "math",
    "isindex",
    "keygen",
    "fencedframe",
];

/// Elements denied inside an `<svg>`: foreign content, animation that can set a link, and `<style>`, which an SVG parses
/// as markup rather than raw text.
const DENIED_SVG: &[&str] = &[
    "foreignobject",
    "animate",
    "animatemotion",
    "animatetransform",
    "set",
    "discard",
    "handler",
    "listener",
    "style",
    "script",
];

/// Attributes that are never safe in a theme.
const DENIED_ATTRIBUTES: &[&str] = &["srcdoc", "formaction", "action", "ping", "http-equiv", "is"];

/// `<link rel>` values a theme may use: the stylesheet and icon loads, preload, and navigation relations.
const LINK_RELS: &[&str] = &[
    "stylesheet",
    "icon",
    "apple-touch-icon",
    "preload",
    "canonical",
    "alternate",
    "next",
    "prev",
    "author",
    "license",
];

/// Check HTML. Problems are in document order with the line of the token.
pub(crate) fn check_html(text: &str, mode: &Mode<'_>, policy: &Policy<'_>) -> Vec<Problem> {
    let sink = Sink {
        state: RefCell::new(State::default()),
        mode,
        policy,
    };
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(text));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut state = tokenizer.sink.state.into_inner();
    if let Some(open) = state.raw.take() {
        state.problems.push(Problem {
            line: state.raw_line,
            code: "unsafe-markup",
            message: format!("<{open}> is not closed"),
        });
    }
    state.problems
}

#[derive(Default)]
struct State {
    problems: Vec<Problem>,
    /// The raw text element the tokenizer is in (`style`, `title`, `textarea`).
    raw: Option<String>,
    raw_line: usize,
    style_text: String,
    svg: usize,
}

struct Sink<'a> {
    state: RefCell<State>,
    mode: &'a Mode<'a>,
    policy: &'a Policy<'a>,
}

impl TokenSink for Sink<'_> {
    type Handle = ();

    fn process_token(&self, token: Token, line_number: u64) -> TokenSinkResult<()> {
        let line = usize::try_from(line_number).unwrap_or(0);
        let mut state = self.state.borrow_mut();
        match token {
            Token::TagToken(tag) => return self.tag(&mut state, &tag, line),
            Token::CharacterTokens(text) => self.characters(&mut state, &text, line),
            Token::CommentToken(text) => {
                if text.contains(FRAGMENT_OPEN) {
                    state
                        .problems
                        .push(problem(line, "unsafe-markup", "a fragment inside an HTML comment"));
                }
            }
            Token::ParseError(message) => {
                if message.starts_with("Saw EOF") || message.starts_with("Unexpected EOF") {
                    state.problems.push(problem(
                        line,
                        "unsafe-markup",
                        "the markup ends inside a tag, attribute or comment",
                    ));
                }
            }
            Token::NullCharacterToken | Token::DoctypeToken(_) | Token::EOFToken => {}
        }
        TokenSinkResult::Continue
    }
}

fn problem(line: usize, code: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        line,
        code,
        message: message.into(),
    }
}

impl Sink<'_> {
    fn characters(&self, state: &mut State, text: &str, line: usize) {
        if state.raw.as_deref() == Some("style") {
            state.style_text.push_str(text);
            return;
        }
        if let Mode::Output { nonce, allowed } = self.mode {
            if text.contains(is_reserved_char) {
                if let Some(raw) = &state.raw {
                    state
                        .problems
                        .push(problem(line, "unsafe-markup", format!("a fragment inside <{raw}>")));
                    return;
                }
                match fragment_refs(text, nonce) {
                    Ok(refs) => {
                        if refs.iter().any(|found| !allowed.contains(&found.index)) {
                            state.problems.push(problem(
                                line,
                                "unsafe-markup",
                                "a fragment that is not part of this render's view",
                            ));
                        }
                    }
                    Err(message) => state.problems.push(problem(line, "unsafe-markup", message)),
                }
            }
        }
    }

    fn tag(&self, state: &mut State, tag: &Tag, line: usize) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        if name.contains(is_reserved_char) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                "an expression or fragment in a tag name",
            ));
            return TokenSinkResult::Continue;
        }
        if tag.kind == TagKind::EndTag {
            if state.raw.as_deref() == Some(name) {
                state.raw = None;
                if name == "style" {
                    let css = std::mem::take(&mut state.style_text);
                    let start = state.raw_line;
                    self.style(state, &css, start);
                }
            }
            if name == "svg" {
                state.svg = state.svg.saturating_sub(1);
            }
            return TokenSinkResult::Continue;
        }
        if DENIED_ELEMENTS.contains(&name) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("<{name}> is not allowed in a theme"),
            ));
        }
        if state.svg > 0 && DENIED_SVG.contains(&name) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("<{name}> is not allowed inside an <svg>"),
            ));
        }
        for attribute in &tag.attrs {
            self.attribute(state, name, tag, &attribute.name.local, &attribute.value, line);
        }
        if name == "link" {
            let rel = tag
                .attrs
                .iter()
                .find(|attribute| &*attribute.name.local == "rel")
                .map(|attribute| attribute.value.to_ascii_lowercase())
                .unwrap_or_default();
            if rel.split_ascii_whitespace().any(|rel| !LINK_RELS.contains(&rel)) || rel.trim().is_empty() {
                state.problems.push(problem(
                    line,
                    "unsafe-markup",
                    format!(
                        "<link rel=\"{rel}\"> is not one a theme may use ({})",
                        LINK_RELS.join(", ")
                    ),
                ));
            }
        }
        if tag.self_closing {
            return TokenSinkResult::Continue;
        }
        if name == "svg" {
            state.svg += 1;
        }
        // The tokenizer alone does not know which elements hold raw text; the sink says so, as a tree builder would.
        if state.svg == 0 {
            let raw = match name {
                "style" => Some(RawKind::Rawtext),
                "title" | "textarea" => Some(RawKind::Rcdata),
                _ => None,
            };
            if let Some(kind) = raw {
                state.raw = Some(name.to_string());
                state.raw_line = line;
                return TokenSinkResult::RawData(kind);
            }
        }
        TokenSinkResult::Continue
    }

    fn attribute(&self, state: &mut State, element: &str, tag: &Tag, name: &str, value: &str, line: usize) {
        if name.contains(is_reserved_char) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("an expression or fragment in an attribute name of <{element}>"),
            ));
            return;
        }
        if value.contains(FRAGMENT_OPEN) || value.contains(FRAGMENT_CLOSE) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("a fragment in the `{name}` attribute of <{element}>"),
            ));
            return;
        }
        if name.starts_with("on") {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("event attribute `{name}` on <{element}>"),
            ));
            return;
        }
        if DENIED_ATTRIBUTES.contains(&name) {
            state.problems.push(problem(
                line,
                "unsafe-markup",
                format!("attribute `{name}` is not allowed in a theme"),
            ));
            return;
        }
        if name == "style" {
            for found in check_css(value, self.policy, None) {
                state.problems.push(problem(
                    line,
                    found.code,
                    format!("style attribute of <{element}>: {}", found.message),
                ));
            }
            return;
        }
        let kind = match (element, name) {
            ("a" | "area", "href") => Some(UrlKind::Navigation),
            ("link", "href") => {
                let navigation = tag.attrs.iter().any(|attribute| {
                    &*attribute.name.local == "rel"
                        && attribute
                            .value
                            .to_ascii_lowercase()
                            .split_ascii_whitespace()
                            .all(|rel| {
                                matches!(rel, "canonical" | "alternate" | "next" | "prev" | "author" | "license")
                            })
                });
                Some(if navigation {
                    UrlKind::Navigation
                } else {
                    UrlKind::Resource
                })
            }
            (_, "href" | "xlink:href") if state.svg > 0 && element == "a" => Some(UrlKind::Navigation),
            (_, "href" | "xlink:href") if state.svg > 0 => Some(UrlKind::Fragment),
            (
                _,
                "href" | "xlink:href" | "src" | "poster" | "data" | "background" | "cite" | "manifest" | "codebase",
            ) => Some(UrlKind::Resource),
            (_, "srcset" | "imagesrcset") => {
                for candidate in value.split(',') {
                    let url = candidate.split_ascii_whitespace().next().unwrap_or("");
                    self.url(state, element, name, url, UrlKind::Resource, line);
                }
                None
            }
            _ => None,
        };
        if let Some(kind) = kind {
            self.url(state, element, name, value, kind, line);
        }
    }

    fn url(&self, state: &mut State, element: &str, name: &str, value: &str, kind: UrlKind, line: usize) {
        // In source, only the part before the first expression is known; the output check sees the rest.
        let known = match self.mode {
            Mode::Source => match value.split_once(EXPRESSION_MARK) {
                Some((before, _)) if before.trim().is_empty() => return,
                Some((before, _)) => {
                    if !has_scheme_or_authority(before) {
                        return;
                    }
                    before
                }
                None => value,
            },
            Mode::Output { .. } => value,
        };
        if let Err(message) = check_url(known, kind, self.policy) {
            state.problems.push(problem(
                line,
                "unsafe-url",
                format!("`{name}` of <{element}>: {message}"),
            ));
        }
    }

    /// The CSS of a `<style>` that starts at `line`.
    fn style(&self, state: &mut State, css: &str, line: usize) {
        if matches!(self.mode, Mode::Output { .. }) && css.contains(is_reserved_char) {
            state
                .problems
                .push(problem(line, "unsafe-css", "a fragment inside <style>"));
            return;
        }
        let css = css.replace(EXPRESSION_MARK, "x");
        for found in check_css(&css, self.policy, None) {
            let at = if found.line > 0 { line + found.line - 1 } else { line };
            state
                .problems
                .push(problem(at, found.code, format!("<style>: {}", found.message)));
        }
    }
}

/// How a URL is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UrlKind {
    /// A link the visitor follows: http, https, mailto, tel and relative URLs.
    Navigation,
    /// Something the page loads: the page's own origin, an approved origin, or a raster image as `data:`.
    Resource,
    /// A reference inside an SVG: `#id` only.
    Fragment,
}

/// Whether the start of a URL already says its scheme or authority (`javascript:`, `https://`, `//host`).
fn has_scheme_or_authority(start: &str) -> bool {
    let cleaned = clean_url(start);
    cleaned.starts_with("//") || scheme_of(&cleaned).is_some()
}

/// The URL as a browser reads it: ASCII tabs and newlines removed anywhere, and leading and trailing spaces and
/// controls trimmed.
fn clean_url(value: &str) -> String {
    let removed: String = value.chars().filter(|ch| !matches!(ch, '\t' | '\n' | '\r')).collect();
    removed.trim_matches(|ch: char| ch <= ' ').to_string()
}

/// The lowercase scheme of a URL, when it has one.
fn scheme_of(url: &str) -> Option<String> {
    let (scheme, _) = url.split_once(':')?;
    let ends_before = url.find(['/', '?', '#']).is_none_or(|at| at > scheme.len());
    let valid = scheme.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
    (valid && ends_before).then(|| scheme.to_ascii_lowercase())
}

/// The `scheme://host[:port]` of an absolute URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    ))
}

const DATA_IMAGES: &[&str] = &[
    "data:image/png",
    "data:image/gif",
    "data:image/jpeg",
    "data:image/jpg",
    "data:image/webp",
    "data:image/avif",
];

/// Check one URL for how it is used.
pub(crate) fn check_url(value: &str, kind: UrlKind, policy: &Policy<'_>) -> Result<(), String> {
    let url = clean_url(value);
    if kind == UrlKind::Fragment {
        return if url.is_empty() || url.starts_with('#') {
            Ok(())
        } else {
            Err(format!("`{}` is not a reference inside the SVG (#id)", clip(&url)))
        };
    }
    let approved = |url: &str| {
        origin_of(url).is_some_and(|origin| {
            policy
                .origins
                .iter()
                .any(|allowed| allowed.trim_end_matches('/').eq_ignore_ascii_case(&origin))
        })
    };
    match scheme_of(&url).as_deref() {
        None => {
            if kind == UrlKind::Resource && url.starts_with("//") && !approved(&format!("https:{url}")) {
                return Err(format!("`{}` loads from an origin that is not approved", clip(&url)));
            }
            Ok(())
        }
        Some("http" | "https") => {
            if kind == UrlKind::Navigation || approved(&url) {
                Ok(())
            } else {
                Err(format!("`{}` loads from an origin that is not approved", clip(&url)))
            }
        }
        Some("mailto" | "tel") if kind == UrlKind::Navigation => Ok(()),
        Some("data") if kind == UrlKind::Resource => {
            let lower = url.to_ascii_lowercase();
            if DATA_IMAGES.iter().any(|prefix| {
                lower.starts_with(prefix) && matches!(lower.as_bytes().get(prefix.len()), Some(b';' | b','))
            }) {
                Ok(())
            } else {
                Err("a data: URL other than a raster image".to_string())
            }
        }
        Some(scheme) => Err(format!("a `{scheme}:` URL")),
    }
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(80).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

/// CSS functions that load or run something other than through `url()`.
const DENIED_CSS: &[(&str, &str)] = &[
    ("@import", "@import"),
    ("expression(", "expression()"),
    ("javascript:", "a javascript: URL"),
    ("vbscript:", "a vbscript: URL"),
    ("behavior:", "behavior"),
    ("behaviour:", "behavior"),
    ("-moz-binding", "-moz-binding"),
    ("</style", "</style>"),
    ("<!--", "<!--"),
    ("-->", "-->"),
    ("image-set(", "image-set()"),
    ("src(", "src()"),
];

/// Check CSS: a stylesheet asset, a `<style>` or a `style` attribute. Comments and escapes are read first, as the CSS
/// tokenizer reads them. A `url()` must be the page's own origin, an approved one, a raster `data:` image, a `#`
/// reference, or with `resolve`, a path that names a file of the package.
pub(crate) fn check_css(css: &str, policy: &Policy<'_>, resolve: Option<&dyn Fn(&str) -> bool>) -> Vec<Problem> {
    let mut found = Vec::new();
    let (text, lines) = match decode_css(css) {
        Ok(decoded) => decoded,
        Err(message) => {
            found.push(problem(0, "unsafe-css", message));
            return found;
        }
    };
    let line_at = |at: usize| lines.get(at).copied().unwrap_or(0);
    // ASCII only, so a byte offset of `lower` is one of `text`.
    let lower = text.to_ascii_lowercase();
    let squeezed: String = lower.chars().filter(|ch| !ch.is_whitespace()).collect();
    for (needle, what) in DENIED_CSS {
        if let Some(at) = lower.find(needle) {
            found.push(problem(
                line_at(at),
                "unsafe-css",
                format!("{what} is not allowed in theme CSS"),
            ));
        } else if squeezed.contains(needle) {
            found.push(problem(0, "unsafe-css", format!("{what} is not allowed in theme CSS")));
        }
    }
    let mut index = 0;
    while let Some(at) = lower[index..].find("url").map(|at| index + at) {
        let after = lower[at + 3..].trim_start();
        index = at + 3;
        let Some(argument) = after.strip_prefix('(') else {
            continue;
        };
        let start = text.len() - argument.len();
        let rest = &text[start..];
        let trimmed = rest.trim_start();
        let url = match trimmed.chars().next() {
            Some(quote @ ('"' | '\'')) => trimmed[1..].split(quote).next().unwrap_or(""),
            _ => trimmed.split(')').next().unwrap_or("").trim(),
        };
        let line = line_at(at);
        let cleaned = clean_url(url);
        if cleaned.starts_with('#') {
            continue;
        }
        // In a stylesheet asset, a relative URL names a file of the package.
        let relative = scheme_of(&cleaned).is_none() && !cleaned.starts_with('/');
        if let (Some(resolve), true) = (resolve, relative) {
            if !resolve(&cleaned) {
                found.push(problem(
                    line,
                    "missing-file",
                    format!("url({}) names no file of the package", clip(&cleaned)),
                ));
            }
            continue;
        }
        if let Err(message) = check_url(url, UrlKind::Resource, policy) {
            found.push(problem(line, "unsafe-css", format!("url(): {message}")));
        }
    }
    found
}

/// CSS with its comments removed and its escapes decoded, with the line of each byte of the result.
fn decode_css(css: &str) -> Result<(String, Vec<usize>), String> {
    let mut out = String::with_capacity(css.len());
    let mut lines = Vec::with_capacity(css.len());
    let mut line = 1;
    let mut chars = css.char_indices().peekable();
    let push = |out: &mut String, lines: &mut Vec<usize>, ch: char, line: usize| {
        out.push(ch);
        lines.extend(std::iter::repeat_n(line, ch.len_utf8()));
    };
    while let Some((at, ch)) = chars.next() {
        match ch {
            '/' if css[at..].starts_with("/*") => {
                let Some(end) = css[at + 2..].find("*/") else {
                    return Err("a CSS comment is not closed".to_string());
                };
                let comment = &css[at..at + 2 + end + 2];
                line += comment.matches('\n').count();
                while chars.peek().is_some_and(|(next, _)| *next < at + comment.len()) {
                    chars.next();
                }
                push(&mut out, &mut lines, ' ', line);
            }
            '\\' => {
                let mut hex = String::new();
                while hex.len() < 6 {
                    match chars.peek() {
                        Some((_, digit)) if digit.is_ascii_hexdigit() => {
                            hex.push(*digit);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                if hex.is_empty() {
                    match chars.next() {
                        Some((_, '\n')) => line += 1,
                        Some((_, escaped)) => push(&mut out, &mut lines, escaped, line),
                        None => {}
                    }
                } else {
                    if chars.peek().is_some_and(|(_, space)| space.is_whitespace()) {
                        if let Some((_, '\n')) = chars.next() {
                            line += 1;
                        }
                    }
                    let decoded = u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{FFFD}');
                    push(&mut out, &mut lines, decoded, line);
                }
            }
            '\n' => {
                push(&mut out, &mut lines, ch, line);
                line += 1;
            }
            _ => push(&mut out, &mut lines, ch, line),
        }
    }
    Ok((out, lines))
}
